use moenotes_client::{CancellationToken, Client, CredentialProvider, StaticCredentials};
use moenotes_server::{
    RouterOptions,
    cache::CacheOptions,
    config::{Config, read_api_key},
    managed::{GameRecovery, ManagedClient, Recovery},
    operator, router_with_options,
};
use std::{
    io::{self, IsTerminal, Read},
    path::Path,
    sync::Arc,
    time::Duration,
};
use zeroize::Zeroizing;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    if command == "--version" || command == "-V" {
        println!("moenotes-server {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if matches!(command.as_str(), "help" | "--help") {
        println!(
            "moenotes-server <serve|check-config|sdk-login|game-login|auth-status> <config.toml>\nsdk-login: --password-stdin reads one private JSON object, otherwise prompts without password echo\ngame-login: --confirm-sdk-ready [--allow-create]\nSIGHUP: reload current saved game session, never log in.\nmoenotes-server --version"
        );
        return Ok(());
    }
    let path = args.next().ok_or("configuration path required")?;
    let flags: Vec<_> = args.collect();
    let config = Config::read(Path::new(&path))?;
    match command.as_str() {
        "sdk-login" => {
            if flags.iter().any(|f| f != "--password-stdin") {
                return Err("unknown flag".into());
            }
            let login = config
                .login
                .as_ref()
                .ok_or("login configuration required")?;
            let (email, password) = if flags.iter().any(|f| f == "--password-stdin") {
                if io::stdin().is_terminal() {
                    return Err("--password-stdin requires a pipe, not a terminal".into());
                }
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    email: String,
                    password: String,
                }
                let mut bytes = Zeroizing::new(Vec::new());
                io::stdin().take(16385).read_to_end(&mut bytes)?;
                if bytes.len() > 16384 {
                    return Err("secret input too large".into());
                }
                let i: Input =
                    serde_json::from_slice(&bytes).map_err(|_| "invalid secret JSON input")?;
                (i.email, Zeroizing::new(i.password))
            } else {
                let email = rpassword::prompt_password("Email: ")?;
                let password = Zeroizing::new(rpassword::prompt_password("Password: ")?);
                (email, password)
            };
            operator::sdk_login(&config.session, login, email, password).await?;
            println!(
                "SDK authorization saved as pending. Complete required SDK consent/checks, then run game-login --confirm-sdk-ready."
            );
            return Ok(());
        }
        "game-login" => {
            if !flags.iter().any(|f| f == "--confirm-sdk-ready")
                || flags
                    .iter()
                    .any(|f| f != "--confirm-sdk-ready" && f != "--allow-create")
            {
                return Err("requires --confirm-sdk-ready; optional --allow-create".into());
            }
            operator::game_login(
                &config.session,
                config
                    .login
                    .as_ref()
                    .ok_or("login configuration required")?,
                config.client_options(),
                flags.iter().any(|f| f == "--allow-create"),
            )
            .await?;
            println!(
                "Game session saved. Reload a running service with SIGHUP; do not log in again merely to restart it."
            );
            return Ok(());
        }
        "serve" | "check-config" | "auth-status" => {
            if !flags.is_empty() {
                return Err("unexpected argument".into());
            }
        }
        _ => return Err("unknown command".into()),
    }
    if let Some(login) = &config.login {
        login.validate(&config.session)?;
    }
    let provider = if let Some(login) = &config.login {
        login.credentials(&config.session)?
    } else {
        config
            .credentials_file
            .as_ref()
            .map(|p| StaticCredentials::from_file(p))
            .transpose()?
    };
    let client = Arc::new(Client::new(
        config.session.clone(),
        provider.as_ref().map(|p| p as &dyn CredentialProvider),
        config.client_options(),
    )?);
    if config.recovery.enabled {
        let login = config.login.as_ref().unwrap();
        if provider.is_some() {
            let _ = login.approved_sdk(&config.session)?;
        }
    }
    if command == "auth-status" {
        println!(
            "{}",
            serde_json::json!({"session":format!("{:?}",client.session_status()),"recovery_enabled":config.recovery.enabled,"network_checked":false})
        );
        return Ok(());
    }
    let stop = CancellationToken::new();
    let recovery: Option<Arc<dyn Recovery>> = if config.recovery.enabled {
        Some(Arc::new(GameRecovery {
            client: client.clone(),
            login: config.login.clone().unwrap(),
            config: config.session.clone(),
        }))
    } else {
        None
    };
    let managed = Arc::new(ManagedClient::new(
        client.clone(),
        recovery,
        Duration::from_secs(config.recovery.cooldown_seconds),
        stop.clone(),
    ));
    let app = router_with_options(
        managed.clone(),
        read_api_key(&config.api_key_file)?,
        RouterOptions {
            mode: config.mode(),
            managed: Some(managed.clone()),
            access_log: config.access_log,
        },
        CacheOptions {
            ttl: Duration::from_secs(config.cache_ttl_seconds),
            capacity: config.cache_capacity,
            ..Default::default()
        },
        stop.clone(),
    )?;
    if command == "check-config" {
        println!("Configuration valid; no upstream requests made; session validity not verified.");
        return Ok(());
    }
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    println!(
        "moenotes-api listening on {} (response mode: {:?})",
        listener.local_addr()?,
        config.mode()
    );
    #[cfg(unix)]
    {
        let stop_reload = stop.clone();
        let managed = managed.clone();
        let client = client.clone();
        tokio::spawn(async move {
            let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                .expect("SIGHUP handler");
            loop {
                tokio::select! {_=stop_reload.cancelled()=>break,_=hup.recv()=>{
                    let success=managed.reload(||{
                        let loaded=if let Some(login)=&config.login{login.credentials(&config.session)}else{config.credentials_file.as_ref().map(|p|StaticCredentials::from_file(p)).transpose()};
                        loaded.and_then(|p|client.replace_session(config.session.clone(),p.as_ref().map(|p|p as &dyn CredentialProvider)))
                    }).is_ok();
                    eprintln!("{}",serde_json::json!({"event":"session_reload","success":success}));
                }}
            }
        });
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM handler");
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
            stop.cancel();
            managed.wait_idle().await;
        })
        .await?;
    Ok(())
}
