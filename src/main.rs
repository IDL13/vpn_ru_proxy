mod chrome_host;
mod desktop_storage;
mod mobile_profile;
mod platform;
mod proxy_manager;
mod settings;
mod site_import;
mod site_rules;

use std::{env, path::PathBuf};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut arguments = env::args_os().skip(1);
    let argument = arguments.next().unwrap_or_default();
    if argument == "export-mobile" {
        return mobile_profile::export(arguments);
    }
    if argument == "--install-chrome-host" {
        let extension_id = arguments.next().unwrap_or_default();
        return chrome_host::install(
            &extension_id.to_string_lossy(),
            arguments.next().map(PathBuf::from),
        );
    }
    if argument
        .to_string_lossy()
        .starts_with("chrome-extension://")
    {
        return chrome_host::run().await;
    }
    Err(concat!(
        "Открой VPN RU Proxy в Chrome. Для установки помощника используй ",
        "--install-chrome-host ID_РАСШИРЕНИЯ, для экспорта профиля телефона — export-mobile."
    )
    .into())
}

async fn shutdown() {
    #[cfg(unix)]
    if let Ok(mut terminate) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
        return;
    }
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}
