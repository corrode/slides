use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use tokio::sync::{Mutex, RwLock, broadcast};

#[derive(Debug, Default)]
pub struct LiveHub {
    sessions: RwLock<HashMap<i64, Arc<SessionRuntime>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveUpdate {
    Content,
    SlideChanged,
    Attention,
}

#[derive(Debug)]
pub struct SessionRuntime {
    pub mutation: Mutex<()>,
    revision: AtomicU64,
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

    pub async fn notify(&self, session_id: i64, update: LiveUpdate) {
        self.runtime(session_id).await.notify(update);
    }

    pub async fn finish(&self, session_id: i64) {
        if let Some(runtime) = self.sessions.write().await.remove(&session_id) {
            runtime.notify(LiveUpdate::Content);
        }
    }
}

impl SessionRuntime {
    fn new() -> Self {
        let (updates, _) = broadcast::channel(128);
        Self {
            mutation: Mutex::new(()),
            revision: AtomicU64::new(0),
            updates,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LiveUpdate> {
        self.updates.subscribe()
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    fn notify(&self, update: LiveUpdate) {
        self.revision.fetch_add(1, Ordering::Release);
        let _ = self.updates.send(update);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn distinguishes_live_update_kinds() {
        let hub = LiveHub::default();
        let runtime = hub.runtime(1).await;
        let mut updates = runtime.subscribe();

        hub.notify(1, LiveUpdate::Content).await;
        hub.notify(1, LiveUpdate::SlideChanged).await;
        hub.notify(1, LiveUpdate::Attention).await;

        assert_eq!(updates.recv().await.unwrap(), LiveUpdate::Content);
        assert_eq!(updates.recv().await.unwrap(), LiveUpdate::SlideChanged);
        assert_eq!(updates.recv().await.unwrap(), LiveUpdate::Attention);
        assert_eq!(runtime.revision(), 3);
    }

    #[tokio::test]
    async fn retains_revisions_without_subscribers() {
        let hub = LiveHub::default();

        hub.notify(1, LiveUpdate::Content).await;

        assert_eq!(hub.runtime(1).await.revision(), 1);
    }
}
