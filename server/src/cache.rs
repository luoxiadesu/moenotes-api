use moenotes_client::{CancellationToken, ClientError, ErrorKind, Generation, Query, QueryClient};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::{Mutex, watch};

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    generation: Generation,
    method: &'static str,
    request: Vec<u8>,
}

pub struct CachedResponse {
    pub json: serde_json::Value,
    pub fetched_at: SystemTime,
    size: usize,
}
type SharedResult = Result<Arc<CachedResponse>, ClientError>;
type Flight = watch::Receiver<Option<SharedResult>>;
struct Entry {
    response: Arc<CachedResponse>,
    expires: Instant,
    touched: Instant,
}
#[derive(Default)]
struct State {
    cache: HashMap<Key, Entry>,
    flights: HashMap<Key, Flight>,
    bytes: usize,
}

#[derive(Clone, Debug)]
pub struct CacheOptions {
    pub ttl: Duration,
    pub capacity: usize,
    pub max_bytes: usize,
    pub max_inflight: usize,
}
impl Default for CacheOptions {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(15),
            capacity: 1024,
            max_bytes: 64 * 1024 * 1024,
            max_inflight: 32,
        }
    }
}

impl CacheOptions {
    pub(crate) fn validate(&self) -> Result<(), ClientError> {
        if self.ttl > Duration::from_secs(3600) || self.capacity > 16384 {
            return Err(ClientError::new(ErrorKind::InvalidConfig));
        }
        Ok(())
    }
}

pub struct QueryCache {
    client: Arc<dyn QueryClient>,
    state: Mutex<State>,
    options: CacheOptions,
    stop: CancellationToken,
}

impl QueryCache {
    pub fn new(
        client: Arc<dyn QueryClient>,
        options: CacheOptions,
        stop: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            client,
            state: Mutex::new(State::default()),
            options,
            stop,
        })
    }

    pub async fn query(
        self: &Arc<Self>,
        query: Query,
    ) -> Result<(Arc<CachedResponse>, &'static str), ClientError> {
        self.options.validate()?;
        query.validate()?;
        if self.stop.is_cancelled() {
            return Err(ClientError::new(ErrorKind::Cancelled));
        }
        let generation = self.client.generation();
        let key = Key {
            generation,
            method: query.method().name,
            request: query.encode(),
        };
        let mut state = self.state.lock().await;
        let now = Instant::now();
        state
            .cache
            .retain(|k, v| k.generation == generation && v.expires > now);
        state.bytes = state.cache.values().map(|e| e.response.size).sum();
        if let Some(entry) = state.cache.get_mut(&key) {
            entry.touched = now;
            if self.client.generation() != generation {
                return Err(ClientError::new(ErrorKind::SessionChanged));
            }
            return Ok((entry.response.clone(), "HIT"));
        }
        let (mut receiver, cache_status) = if let Some(receiver) = state.flights.get(&key) {
            (receiver.clone(), "COALESCED")
        } else {
            if state.flights.len() >= self.options.max_inflight {
                return Err(ClientError::new(ErrorKind::QueueFull));
            }
            let (sender, receiver) = watch::channel(None);
            state.flights.insert(key.clone(), receiver.clone());
            let this = self.clone();
            tokio::spawn(async move {
                let result = this
                    .client
                    .execute(generation, query, this.stop.child_token())
                    .await
                    .and_then(|result| {
                        if result.generation != generation || this.client.generation() != generation
                        {
                            return Err(ClientError::new(ErrorKind::SessionChanged));
                        }
                        let json = moenotes_proto::to_json(&result.message)
                            .map_err(|_| ClientError::new(ErrorKind::Protocol))?;
                        let size = serde_json::to_vec(&json)
                            .map_err(|_| ClientError::new(ErrorKind::Protocol))?
                            .len();
                        Ok(Arc::new(CachedResponse {
                            json,
                            fetched_at: result.fetched_at,
                            size,
                        }))
                    });
                let mut state = this.state.lock().await;
                let result = if this.client.generation() != generation {
                    Err(ClientError::new(ErrorKind::SessionChanged))
                } else {
                    result
                };
                if let Ok(response) = &result
                    && this.options.capacity > 0
                    && !this.options.ttl.is_zero()
                    && response.size <= this.options.max_bytes
                {
                    while !state.cache.is_empty()
                        && (state.cache.len() >= this.options.capacity
                            || state.bytes + response.size > this.options.max_bytes)
                    {
                        let oldest = state
                            .cache
                            .iter()
                            .min_by_key(|(_, e)| e.touched)
                            .map(|(k, _)| k.clone())
                            .unwrap();
                        if let Some(removed) = state.cache.remove(&oldest) {
                            state.bytes -= removed.response.size;
                        }
                    }
                    let now = Instant::now();
                    state.bytes += response.size;
                    state.cache.insert(
                        key.clone(),
                        Entry {
                            response: response.clone(),
                            expires: now + this.options.ttl,
                            touched: now,
                        },
                    );
                }
                sender.send_replace(Some(result));
                state.flights.remove(&key);
            });
            (receiver, "MISS")
        };
        drop(state);
        loop {
            let result = receiver.borrow().clone();
            if let Some(result) = result {
                if self.client.generation() != generation {
                    return Err(ClientError::new(ErrorKind::SessionChanged));
                }
                return result.map(|response| (response, cache_status));
            }
            tokio::select! {
                _ = self.stop.cancelled() => return Err(ClientError::new(ErrorKind::Cancelled)),
                result = receiver.changed() => if result.is_err() { return Err(ClientError::new(ErrorKind::Protocol)); },
            }
        }
    }
}
