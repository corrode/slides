use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use sqlx::SqlitePool;
use tokio::sync::{Mutex, OnceCell, RwLock, broadcast};

use crate::{
    markdown::{DeckDocument, parse_deck},
    models::DeckVersion,
    store,
};

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
    viewers: AtomicU64,
    updates: broadcast::Sender<LiveUpdate>,
    deck: OnceCell<SessionDeck>,
}

#[derive(Debug)]
pub(crate) struct SessionDeck {
    pub version: DeckVersion,
    pub document: Arc<DeckDocument>,
}

#[derive(Debug)]
pub(crate) struct AudienceConnection {
    runtime: Arc<SessionRuntime>,
}

impl LiveHub {
    pub async fn runtime(
        &self,
        pool: &SqlitePool,
        session_id: i64,
    ) -> anyhow::Result<Arc<SessionRuntime>> {
        if let Some(runtime) = self.sessions.read().await.get(&session_id).cloned() {
            return Ok(runtime);
        }

        let mut sessions = self.sessions.write().await;
        if let Some(runtime) = sessions.get(&session_id) {
            return Ok(Arc::clone(runtime));
        }

        // Check the persisted state under the insertion lock: a caller's session
        // snapshot may predate finish(). If ending races this read, finish() must
        // remove our entry after insertion. If finish() won, we never retain it.
        let session = store::get_session(pool, session_id).await?;
        let runtime = Arc::new(SessionRuntime::new());
        if session.ended_at.is_none() {
            sessions.insert(session_id, Arc::clone(&runtime));
        }
        Ok(runtime)
    }

    pub async fn notify(&self, session_id: i64, update: LiveUpdate) {
        if let Some(runtime) = self.sessions.read().await.get(&session_id) {
            runtime.notify(update);
        }
    }

    /// Call after the ended state has been persisted, so later requests cannot
    /// register a new retained runtime for this session.
    pub async fn finish(&self, session_id: i64) {
        if let Some(runtime) = self.sessions.write().await.remove(&session_id) {
            runtime.notify(LiveUpdate::Content);
        }
    }

    #[cfg(test)]
    pub(crate) async fn retained_session_count(&self) -> usize {
        self.sessions.read().await.len()
    }
}

impl SessionRuntime {
    fn new() -> Self {
        let (updates, _) = broadcast::channel(128);
        Self {
            mutation: Mutex::new(()),
            revision: AtomicU64::new(0),
            viewers: AtomicU64::new(0),
            updates,
            deck: OnceCell::new(),
        }
    }

    /// The version ID is immutable for the session owning this runtime.
    /// Failed loads or parses leave the cell empty so the next request can retry.
    pub(crate) async fn deck(
        &self,
        pool: &SqlitePool,
        version_id: i64,
    ) -> anyhow::Result<&SessionDeck> {
        self.deck
            .get_or_try_init(|| async {
                let version = store::get_version(pool, version_id).await?;
                tokio::task::spawn_blocking(move || {
                    let document = Arc::new(parse_deck(&version.source)?);
                    Ok(SessionDeck { version, document })
                })
                .await?
            })
            .await
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LiveUpdate> {
        self.updates.subscribe()
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub fn viewer_count(&self) -> u64 {
        self.viewers.load(Ordering::Acquire)
    }

    pub(crate) fn track_audience(self: &Arc<Self>) -> AudienceConnection {
        self.viewers.fetch_add(1, Ordering::AcqRel);
        self.notify(LiveUpdate::Content);
        AudienceConnection {
            runtime: Arc::clone(self),
        }
    }

    fn notify(&self, update: LiveUpdate) {
        self.revision.fetch_add(1, Ordering::Release);
        let _ = self.updates.send(update);
    }
}

impl Drop for AudienceConnection {
    fn drop(&mut self) {
        let decremented = self
            .runtime
            .viewers
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_sub(1)
            })
            .is_ok();
        if decremented {
            self.runtime.notify(LiveUpdate::Content);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_version(source: &str) -> (tempfile::TempDir, SqlitePool, i64) {
        let directory = tempfile::tempdir().unwrap();
        let pool = store::connect(&format!(
            "sqlite://{}",
            directory.path().join("slides.db").display()
        ))
        .await
        .unwrap();
        let deck = store::create_deck(&pool, "cache", "Cache").await.unwrap();
        let version_id = store::save_and_publish_deck(
            &pool,
            deck.id,
            "Cache",
            source,
            source,
            &crate::models::Theme::default(),
        )
        .await
        .unwrap();
        (directory, pool, version_id)
    }

    #[tokio::test]
    async fn concurrent_deck_loads_share_one_cached_value() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SessionDeck>();
        assert_send_sync::<SessionRuntime>();

        let (_directory, pool, version_id) =
            test_version("# Cached\n\n```rust\nfn main() {}\n```").await;
        let runtime = SessionRuntime::new();
        assert!(runtime.deck.get().is_none());
        let (first, second) = tokio::join!(
            runtime.deck(&pool, version_id),
            runtime.deck(&pool, version_id),
        );
        let first = first.unwrap();
        assert!(std::ptr::eq(first, second.unwrap()));
        assert!(first.document.slides[0].html.contains("<pre"));

        // A warm hit must not need even a version query, let alone highlighting.
        pool.close().await;
        assert!(std::ptr::eq(
            first,
            runtime.deck(&pool, version_id).await.unwrap()
        ));
    }

    #[tokio::test]
    async fn failed_deck_initialization_can_retry() {
        let (_directory, pool, version_id) =
            test_version(":::poll\n- Missing closing directive").await;
        let runtime = SessionRuntime::new();
        assert!(runtime.deck(&pool, -1).await.is_err());
        assert!(runtime.deck.get().is_none());
        assert!(runtime.deck(&pool, version_id).await.is_err());
        assert!(runtime.deck.get().is_none());

        // Fault injection only: published versions are never edited in production.
        sqlx::query("UPDATE deck_versions SET source = '# Recovered' WHERE id = ?")
            .bind(version_id)
            .execute(&pool)
            .await
            .unwrap();
        let cached = runtime.deck(&pool, version_id).await.unwrap();
        assert!(cached.document.slides[0].html.contains("Recovered"));
    }

    async fn test_session() -> (tempfile::TempDir, SqlitePool, crate::models::LiveSession) {
        let (directory, pool, version_id) = test_version("# Cached").await;
        let deck = store::deck_by_slug(&pool, "cache").await.unwrap().unwrap();
        let session = store::start_session(&pool, deck.id, version_id)
            .await
            .unwrap();
        (directory, pool, session)
    }

    #[tokio::test]
    async fn finish_releases_the_hubs_cached_deck() {
        let (_directory, pool, session) = test_session().await;
        let hub = LiveHub::default();
        let runtime = hub.runtime(&pool, session.id).await.unwrap();
        let cached = runtime.deck(&pool, session.deck_version_id).await.unwrap();
        let document = Arc::downgrade(&cached.document);
        drop(runtime);
        assert!(document.upgrade().is_some());

        store::end_session(&pool, session.id, store::now_millis())
            .await
            .unwrap();
        hub.finish(session.id).await;
        assert!(document.upgrade().is_none());
        assert_eq!(hub.retained_session_count().await, 0);

        // The caller still has a pre-finish session snapshot. Its ID must not
        // resurrect a retained runtime, even if it finishes parsing later.
        assert!(session.ended_at.is_none());
        let runtime = hub.runtime(&pool, session.id).await.unwrap();
        let cached = runtime.deck(&pool, session.deck_version_id).await.unwrap();
        let document = Arc::downgrade(&cached.document);
        let weak_runtime = Arc::downgrade(&runtime);
        assert_eq!(hub.retained_session_count().await, 0);
        drop(runtime);
        hub.notify(session.id, LiveUpdate::Content).await;
        assert_eq!(hub.retained_session_count().await, 0);
        assert!(document.upgrade().is_none());
        assert!(weak_runtime.upgrade().is_none());

        // This also works after a restart, with no in-memory finish history.
        let cold_hub = LiveHub::default();
        drop(cold_hub.runtime(&pool, session.id).await.unwrap());
        assert_eq!(cold_hub.retained_session_count().await, 0);
    }

    #[tokio::test]
    async fn cold_runtime_acquisition_competing_with_finish_does_not_leak() {
        let (_directory, pool, version_id) = test_version("# Cached").await;
        let deck = store::deck_by_slug(&pool, "cache").await.unwrap().unwrap();
        let hub = LiveHub::default();
        for _ in 0..16 {
            let session = store::start_session(&pool, deck.id, version_id)
                .await
                .unwrap();
            let (runtime, ()) = tokio::join!(hub.runtime(&pool, session.id), async {
                store::end_session(&pool, session.id, store::now_millis())
                    .await
                    .unwrap();
                hub.finish(session.id).await;
            },);
            let runtime = runtime.unwrap();
            // Loading after finish must only extend the requesting task's lifetime.
            let cached = runtime.deck(&pool, version_id).await.unwrap();
            let document = Arc::downgrade(&cached.document);
            let weak_runtime = Arc::downgrade(&runtime);
            assert_eq!(hub.retained_session_count().await, 0);
            drop(runtime);
            assert!(document.upgrade().is_none());
            assert!(weak_runtime.upgrade().is_none());
        }
    }

    #[tokio::test]
    async fn distinguishes_live_update_kinds() {
        let (_directory, pool, session) = test_session().await;
        let hub = LiveHub::default();
        let runtime = hub.runtime(&pool, session.id).await.unwrap();
        let mut updates = runtime.subscribe();

        hub.notify(session.id, LiveUpdate::Content).await;
        hub.notify(session.id, LiveUpdate::SlideChanged).await;
        hub.notify(session.id, LiveUpdate::Attention).await;

        assert_eq!(updates.recv().await.unwrap(), LiveUpdate::Content);
        assert_eq!(updates.recv().await.unwrap(), LiveUpdate::SlideChanged);
        assert_eq!(updates.recv().await.unwrap(), LiveUpdate::Attention);
        assert_eq!(runtime.revision(), 3);
    }

    #[tokio::test]
    async fn retains_revisions_without_subscribers() {
        let (_directory, pool, session) = test_session().await;
        let hub = LiveHub::default();
        drop(hub.runtime(&pool, session.id).await.unwrap());
        // A retained runtime hit does not need another database read.
        pool.close().await;
        hub.notify(session.id, LiveUpdate::Content).await;
        assert_eq!(hub.runtime(&pool, session.id).await.unwrap().revision(), 1);
    }

    #[tokio::test]
    async fn notifications_do_not_create_runtimes() {
        let hub = LiveHub::default();
        hub.notify(1, LiveUpdate::Content).await;
        assert_eq!(hub.retained_session_count().await, 0);
    }

    #[tokio::test]
    async fn tracks_active_audience_connections() {
        let (_directory, pool, session) = test_session().await;
        let hub = LiveHub::default();
        let runtime = hub.runtime(&pool, session.id).await.unwrap();
        let mut updates = runtime.subscribe();

        let connection = runtime.track_audience();
        assert_eq!(runtime.viewer_count(), 1);
        assert_eq!(updates.recv().await.unwrap(), LiveUpdate::Content);

        drop(connection);
        assert_eq!(runtime.viewer_count(), 0);
        assert_eq!(updates.recv().await.unwrap(), LiveUpdate::Content);
    }
}
