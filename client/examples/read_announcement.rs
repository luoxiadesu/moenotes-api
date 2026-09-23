use moenotes_client::{Client, ClientOptions, Query, SessionConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = SessionConfig {
        region: "operator-selected-region".into(),
        origin: "https://game.example.invalid".into(),
        allowed_origins: vec!["https://game.example.invalid".into()],
        platform: "android".into(),
        client_version: "1.0.1".into(),
        master_version: None,
        resource_version: None,
    };
    // Construction is offline. Explicitly adapt this example for an authorized host.
    let client = Client::new(config, None, ClientOptions::default())?;
    let _ = (client, Query::Announcements(Default::default()));
    println!("Example compiled. Configure an authorized host before executing a query.");
    Ok(())
}
