use crate::{
    Result, desktop_storage, platform,
    proxy_manager::ProxyManager,
    settings::{Settings, SettingsEdit},
    shutdown,
    site_import::{AmneziaEntry, import_amnezia_sites},
};
use platform::data_directory;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{self, Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};
use tokio::sync::oneshot;

const HOST_NAME: &str = "org.vpn_ru_proxy.chrome";
const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
// Chrome limits each native response to 1 MiB.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

pub fn install(extension_id: &str, chrome_directory: Option<PathBuf>) -> Result<()> {
    if extension_id.len() != 32
        || !extension_id
            .bytes()
            .all(|byte| (b'a'..=b'p').contains(&byte))
    {
        return Err("Укажи ID расширения VPN RU Proxy из chrome://extensions.".into());
    }
    let directory = platform::manifest_directory(chrome_directory)?;
    fs::create_dir_all(&directory)?;
    let manifest = directory.join(format!("{HOST_NAME}.json"));
    desktop_storage::write_json(
        &manifest,
        &json!({
            "name": HOST_NAME,
            "description": "VPN RU Proxy",
            "path": env::current_exe()?,
            "type": "stdio",
            "allowed_origins": [format!("chrome-extension://{extension_id}/")],
        }),
    )?;
    #[cfg(windows)]
    platform::register_host(HOST_NAME, &manifest)?;
    println!("VPN RU Proxy подключён к Chrome. Открой расширение, чтобы включить прокси.");
    Ok(())
}

#[derive(Deserialize)]
#[serde(tag = "action", content = "payload", rename_all = "camelCase")]
enum Request {
    Start,
    Stop,
    Status,
    Settings,
    Save(SettingsEdit),
    Import(Vec<AmneziaEntry>),
}

impl Request {
    fn run(self, manager: &mut Option<ProxyManager>) -> Result<Value> {
        match self {
            Self::Start => {
                if let Some(manager) = manager {
                    manager.start()?;
                } else {
                    let mut new = ProxyManager::load(data_directory()?)?;
                    new.start()?;
                    *manager = Some(new);
                }
                Ok(json!({"running": true}))
            }
            Self::Stop => {
                if let Some(manager) = manager.as_mut() {
                    manager.stop()?;
                }
                *manager = None;
                Ok(json!({"running": false}))
            }
            Self::Status => Ok(json!({"running": match manager {
                Some(manager) => manager.running()?,
                None => false,
            }})),
            Self::Settings => Ok(if let Some(manager) = manager {
                json!({"settings": manager.settings()})
            } else {
                let directory = data_directory()?;
                desktop_storage::prepare(&directory)?;
                json!({"settings": Settings::load(&directory)?})
            }),
            Self::Save(edit) => {
                let settings = edit.into_settings()?;
                let response = json!({"settings": settings});
                encode_response(&response)?;
                match manager {
                    Some(manager) => manager.save(settings)?,
                    None => ProxyManager::load(data_directory()?)?.save(settings)?,
                }
                Ok(response)
            }
            Self::Import(entries) => Ok(json!({"sites": import_amnezia_sites(entries)?})),
        }
    }
}

enum Session {
    Open(Option<ProxyManager>),
    Closed,
}

pub async fn run() -> Result<()> {
    let session = Arc::new(Mutex::new(Session::Open(None)));
    let (finished, connection) = oneshot::channel();
    let worker_session = Arc::clone(&session);
    thread::Builder::new().spawn(move || {
        let _ = finished.send(serve_messages(&worker_session));
    })?;

    let result: Result<()> = tokio::select! {
        result = connection => result.unwrap_or_else(|error| Err(error.into())),
        () = shutdown() => Ok(()),
    };
    let previous_session = {
        let mut session = session
            .lock()
            .map_err(|_| "Не удалось получить состояние прокси")?;
        // Reject new requests before releasing the lock to stop the proxy.
        std::mem::replace(&mut *session, Session::Closed)
    };
    if let Session::Open(Some(mut manager)) = previous_session {
        manager.stop()?;
    }
    result
}

fn serve_messages(session: &Mutex<Session>) -> Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    while let Some(payload) = read_message(&mut input)? {
        let result = (|| {
            let request = serde_json::from_slice::<Request>(&payload)?;
            let mut session = session
                .lock()
                .map_err(|_| "Не удалось получить состояние прокси")?;
            let Session::Open(manager) = &mut *session else {
                return Err("Помощник завершает работу.".into());
            };
            encode_response(&request.run(manager)?)
        })();
        let payload = match result {
            Ok(payload) => payload,
            Err(error) => {
                let message: String = error.to_string().chars().take(4000).collect();
                encode_response(&json!({"error": message}))?
            }
        };
        output.write_all(&u32::try_from(payload.len())?.to_ne_bytes())?;
        output.write_all(&payload)?;
        output.flush()?;
    }
    Ok(())
}

fn read_message(input: &mut impl Read) -> Result<Option<Vec<u8>>> {
    let mut length = [0; 4];
    match input.read_exact(&mut length[..1]) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    input.read_exact(&mut length[1..])?;
    let length = u32::from_ne_bytes(length) as usize;
    if length > MAX_REQUEST_BYTES {
        return Err("Слишком большой запрос от расширения.".into());
    }
    let mut payload = vec![0; length];
    input.read_exact(&mut payload)?;
    Ok(Some(payload))
}

fn encode_response(response: &Value) -> Result<Vec<u8>> {
    let payload = serde_json::to_vec(response)?;
    if payload.len() > MAX_RESPONSE_BYTES {
        return Err("Слишком большой список сайтов. Сократи список и попробуй снова.".into());
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::read_message;

    #[test]
    fn distinguishes_end_of_stream_from_truncated_messages() -> crate::Result<()> {
        let mut frame = 2_u32.to_ne_bytes().to_vec();
        frame.extend_from_slice(b"{}");

        assert!(read_message(&mut &frame[..1]).is_err());
        assert!(read_message(&mut &frame[..5]).is_err());

        let mut input = frame.as_slice();
        assert_eq!(read_message(&mut input)?.as_deref(), Some(b"{}".as_slice()));
        assert_eq!(read_message(&mut input)?, None);
        Ok(())
    }
}
