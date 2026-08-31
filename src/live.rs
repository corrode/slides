use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, RwLock, broadcast};

#[derive(Debug, Default)]
pub struct LiveHub {
    sessions: RwLock<HashMap<i64, Arc<SessionRuntime>>>,
}

#[derive(Debug)]
pub struct SessionRuntime {
    pub mutation: Mutex<()>,
    updates: broadcast::Sender<()>,
}

impl LiveHub {
    pub async fn runtime(&self, session_id: i64) -> Arc<SessionRuntime> {
        if let Some(runtime) = self.sessions.read().await.get(&session_id).cloned() {
            return runtime;
        }

        let mut sessions = self.sessions.write().await;
        sessions
            .entry(session_id)
            .or_insert_with(|| Arc::new(SessionRuntime::new()))
            .clone()
    }

    pub async fn subscribe(&self, session_id: i64) -> broadcast::Receiver<()> {
        self.runtime(session_id).await.updates.subscribe()
    }

    pub async fn notify(&self, session_id: i64) {
        let runtime = self.runtime(session_id).await;
        let _ = runtime.updates.send(());
    }

    pub async fn finish(&self, session_id: i64) {
        if let Some(runtime) = self.sessions.write().await.remove(&session_id) {
            let _ = runtime.updates.send(());
        }
    }
}

impl SessionRuntime {
    fn new() -> Self {
        let (updates, _) = broadcast::channel(128);
        Self {
            mutation: Mutex::new(()),
            updates,
        }
    }
}
