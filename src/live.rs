use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, RwLock, broadcast};

#[derive(Debug, Default)]
pub struct LiveHub {
    sessions: RwLock<HashMap<i64, Arc<SessionRuntime>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveUpdate {
    Content,
    SlideChanged,
}

#[derive(Debug)]
pub struct SessionRuntime {
    pub mutation: Mutex<()>,
    updates: broadcast::Sender<LiveUpdate>,
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

    pub async fn subscribe(&self, session_id: i64) -> broadcast::Receiver<LiveUpdate> {
        self.runtime(session_id).await.updates.subscribe()
    }

    pub async fn notify(&self, session_id: i64, update: LiveUpdate) {
        let runtime = self.runtime(session_id).await;
        let _ = runtime.updates.send(update);
    }

    pub async fn finish(&self, session_id: i64) {
        if let Some(runtime) = self.sessions.write().await.remove(&session_id) {
            let _ = runtime.updates.send(LiveUpdate::Content);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn distinguishes_content_updates_from_slide_changes() {
        let hub = LiveHub::default();
        let mut updates = hub.subscribe(1).await;

        hub.notify(1, LiveUpdate::Content).await;
        hub.notify(1, LiveUpdate::SlideChanged).await;

        assert_eq!(updates.recv().await.unwrap(), LiveUpdate::Content);
        assert_eq!(updates.recv().await.unwrap(), LiveUpdate::SlideChanged);
    }
}
