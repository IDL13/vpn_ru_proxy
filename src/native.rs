use crate::{amnezia, platform, shutdown, write_json, Edit, Manager, Result, Settings};
use platform::data_directory;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    env, fs,
    io::{self, Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};
use tokio::sync::oneshot;

const HOST_NAME: &str = "org.vpn_ru_proxy.chrome";

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
    write_json(
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
    Save(Edit),
    Import(Vec<amnezia::Entry>),
}

impl Request {
    fn run(self, manager: &mut Option<Manager>) -> Result<Value> {
        match self {
            Self::Start => {
                if let Some(manager) = manager {
                    manager.start()?;
                } else {
                    let mut new = Manager::load(data_directory()?)?;
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
            Self::Settings => {
                let settings = match manager {
                    Some(manager) => manager.settings.clone(),
                    None => Settings::load(&data_directory()?)?,
                };
                Ok(json!({"settings": settings}))
            }
            Self::Save(edit) => {
                let settings = edit.settings()?;
                let response = json!({"settings": settings});
                encode_response(&response)?;
                match manager {
                    Some(manager) => manager.save(settings)?,
                    None => Manager::load(data_directory()?)?.save(settings)?,
                }
                Ok(response)
            }
            Self::Import(entries) => Ok(json!({"sites": amnezia::sites(entries)?})),
        }
    }
}

pub async fn run() -> Result<()> {
    let manager = Arc::new(Mutex::new(None));
    let (finished, connection) = oneshot::channel();
    let client_manager = manager.clone();
    thread::spawn(move || {
        let _ = finished.send(serve_messages(&client_manager));
    });

    let result: Result<()> = tokio::select! {
        result = connection => result?,
        _ = shutdown() => Ok(()),
    };
    if let Some(mut manager) = manager
        .lock()
        .map_err(|_| "Не удалось получить состояние прокси")?
        .take()
    {
        manager.stop()?;
    }
    result
}

fn serve_messages(manager: &Mutex<Option<Manager>>) -> Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    while let Some(payload) = read_message(&mut input)? {
        let result = (|| {
            let request = serde_json::from_slice::<Request>(&payload)?;
            let mut manager = manager
                .lock()
                .map_err(|_| "Не удалось получить состояние прокси")?;
            encode_response(&request.run(&mut manager)?)
        })();
        let payload = match result {
            Ok(payload) => payload,
            Err(error) => {
                let message: String = error.to_string().chars().take(4000).collect();
                encode_response(&json!({"error": message}))?
            }
        };
        output.write_all(&(payload.len() as u32).to_ne_bytes())?;
        output.write_all(&payload)?;
        output.flush()?;
    }
    Ok(())
}

fn read_message(input: &mut impl Read) -> Result<Option<Vec<u8>>> {
    let mut length = [0; 4];
    if input.read(&mut length[..1])? == 0 {
        return Ok(None);
    }
    input.read_exact(&mut length[1..])?;
    let length = u32::from_ne_bytes(length) as usize;
    if length > 4 * 1024 * 1024 {
        return Err("Слишком большой запрос от расширения.".into());
    }
    let mut payload = vec![0; length];
    input.read_exact(&mut payload)?;
    Ok(Some(payload))
}

fn encode_response(response: &Value) -> Result<Vec<u8>> {
    let payload = serde_json::to_vec(response)?;
    // Chrome limits each native response to 1 MiB.
    if payload.len() > 1024 * 1024 {
        return Err("Слишком большой список сайтов. Сократи список и попробуй снова.".into());
    }
    Ok(payload)
}
