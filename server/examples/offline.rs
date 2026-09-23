//! Local demonstration only. It never creates an upstream transport or game session.
use async_trait::async_trait;
use moenotes_client::{
    CancellationToken, ClientError, Generation, Query, QueryClient, QueryResponse,
};
use moenotes_server::{cache::CacheOptions, router};
use prost_reflect::DynamicMessage;
use std::{sync::Arc, time::SystemTime};
use zeroize::Zeroizing;

struct Offline(Generation);
#[async_trait]
impl QueryClient for Offline {
    fn generation(&self) -> Generation {
        self.0
    }
    async fn execute(
        &self,
        generation: Generation,
        query: Query,
        _: CancellationToken,
    ) -> Result<QueryResponse, ClientError> {
        let message = DynamicMessage::new(
            moenotes_proto::pool()
                .get_message_by_name(query.method().output)
                .unwrap(),
        );
        Ok(QueryResponse {
            message,
            generation,
            fetched_at: SystemTime::now(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "8081".into())
        .parse::<u16>()?;
    let key = Zeroizing::new("offline-demo-key-not-for-production-123456789".into());
    let stop = CancellationToken::new();
    let app = router(
        Arc::new(Offline(Generation::new_v4())),
        key,
        true,
        CacheOptions::default(),
        stop.clone(),
    )?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    println!(
        "Offline synthetic demo at http://{}; fixed demo key documented in README",
        listener.local_addr()?
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            stop.cancel();
        })
        .await?;
    Ok(())
}
