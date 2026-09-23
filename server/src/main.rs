use moenotes_client::{CancellationToken, Client, CredentialProvider, StaticCredentials};
use moenotes_server::{
    cache::CacheOptions,
    config::{Config, read_api_key},
    router,
};
use std::{path::Path, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    if command == "help" || command == "--help" {
        println!(
            "moenotes-server <serve|check-config> <config.toml>\nThis CLI uses static credentials only. No game calls are made by check-config."
        );
        return Ok(());
    }
    if command != "serve" && command != "check-config" {
        return Err("unknown command".into());
    }
    let path = args.next().ok_or("configuration path required")?;
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let config = Config::read(Path::new(&path))?;
    let provider = config
        .credentials_file
        .as_ref()
        .map(|p| StaticCredentials::from_file(p))
        .transpose()?;
    let client = Arc::new(Client::new(
        config.session.clone(),
        provider.as_ref().map(|p| p as &dyn CredentialProvider),
        config.client_options(),
    )?);
    let stop = CancellationToken::new();
    let cache = CacheOptions {
        ttl: Duration::from_secs(config.cache_ttl_seconds),
        capacity: config.cache_capacity,
        ..CacheOptions::default()
    };
    let app = router(
        client,
        read_api_key(&config.api_key_file)?,
        config.enable_experimental_raw,
        cache,
        stop.clone(),
    )?;
    if command == "check-config" {
        println!("Configuration valid; no upstream requests made; session validity not verified.");
        return Ok(());
    }
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    println!(
        "moenotes-api listening on {} (experimental raw routes: {})",
        listener.local_addr()?,
        config.enable_experimental_raw
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM handler");
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
            stop.cancel();
        })
        .await?;
    Ok(())
}
