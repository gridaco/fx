//! Per-route concurrency and pacing of provider requests.
//!
//! - Concurrency: one semaphore per route id per invocation, created with the first limit asked
//!   for. The limit of a call is the project's `fx.yaml` `routes.<capability>.concurrency` when set
//!   (the long form `{route, concurrency}`), else the route table's `concurrency`, else none.
//!   A slot is taken **per send** (each send, submit or collect of an attempt), never across a
//!   whole body, so one instance using one route for two capabilities cannot deadlock itself.
//! - Pacing: a route's `requests_per_minute` spaces the **starts of provider requests** on it by
//!   `60 / rpm` seconds, across the invocation. Cache hits are never paced.
//!
//! Both wait cancellably: a cancelled wait returns `None` and holds no slot. A start it had
//! already been given under `requests_per_minute` stays taken, so the next request on the route
//! waits as if it had been sent.

use crate::engine::Cancel;
use grida_fx_providers::BoxFuture;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

/// The pacing state of one invocation.
#[derive(Debug, Default)]
pub struct Pacing {
    slots: Mutex<HashMap<String, Arc<Semaphore>>>,
    next_start: Mutex<HashMap<String, Instant>>,
}

/// A held route slot; dropping it frees the slot.
#[derive(Debug)]
pub struct Slot(
    /// Held for its drop, never read.
    #[allow(dead_code)]
    Option<OwnedSemaphorePermit>,
);

impl Slot {
    /// A slot that holds nothing (a route with no limit; fakes in tests).
    pub fn free() -> Slot {
        Slot(None)
    }
}

/// What the retry owner waits on before each send: [`Pacing`] in a run, a fake in tests.
pub trait Admission: Send + Sync {
    fn admit<'a>(
        &'a self,
        route_id: &'a str,
        limit: Option<u32>,
        rpm: Option<u32>,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Option<Slot>>;
}

impl Admission for Pacing {
    fn admit<'a>(
        &'a self,
        route_id: &'a str,
        limit: Option<u32>,
        rpm: Option<u32>,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Option<Slot>> {
        Box::pin(Pacing::admit(self, route_id, limit, rpm, cancel))
    }
}

impl Pacing {
    pub fn new() -> Pacing {
        Pacing::default()
    }

    /// Waits for a slot of `route_id` (no limit: at once) and then for its next start under
    /// `rpm`. `None` when `cancel` fired first.
    pub async fn admit(
        &self,
        route_id: &str,
        limit: Option<u32>,
        rpm: Option<u32>,
        cancel: &Cancel,
    ) -> Option<Slot> {
        if cancel.is_cancelled() {
            return None;
        }
        let slot = match limit {
            None => Slot::free(),
            Some(limit) => {
                let semaphore = self.semaphore(route_id, limit);
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return None,
                    permit = semaphore.acquire_owned() => Slot(Some(permit.ok()?)),
                }
            }
        };
        if let Some(rpm) = rpm.filter(|rpm| *rpm > 0) {
            let start = self.next_start(route_id, rpm);
            tokio::select! {
                biased;
                // The slot taken is dropped with this branch: a cancelled wait holds nothing.
                _ = cancel.cancelled() => return None,
                _ = tokio::time::sleep_until(start) => {}
            }
        }
        Some(slot)
    }

    /// The route's semaphore, made with the first limit asked for (a limit of 0 counts as 1, so
    /// a route can never block forever).
    fn semaphore(&self, route_id: &str, limit: u32) -> Arc<Semaphore> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let semaphore = slots
            .entry(route_id.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(limit.max(1) as usize)));
        Arc::clone(semaphore)
    }

    /// Takes the route's next start: `max(now, next)`, and moves `next` on by `60 / rpm` seconds.
    fn next_start(&self, route_id: &str, rpm: u32) -> Instant {
        let interval = Duration::from_nanos(60_000_000_000 / u64::from(rpm));
        let mut next = self.next_start.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let start = next
            .get(route_id)
            .map_or(now, |planned| (*planned).max(now));
        next.insert(route_id.to_string(), start + interval);
        start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn requests_per_minute_space_the_starts() {
        let pacing = Pacing::new();
        let cancel = Cancel::new();
        let begin = Instant::now();
        let mut starts = Vec::new();
        for _ in 0..3 {
            let slot = pacing.admit("img-a@acme", None, Some(60), &cancel).await;
            assert!(slot.is_some());
            starts.push(Instant::now() - begin);
        }
        assert_eq!(
            starts,
            [
                Duration::ZERO,
                Duration::from_secs(1),
                Duration::from_secs(2)
            ]
        );
        // Another route is paced on its own.
        let other = Instant::now();
        assert!(
            pacing
                .admit("llm-a@acme", None, Some(60), &cancel)
                .await
                .is_some()
        );
        assert_eq!(Instant::now(), other);
        // After a quiet spell, the next request starts at once.
        tokio::time::advance(Duration::from_secs(10)).await;
        let quiet = Instant::now();
        assert!(
            pacing
                .admit("img-a@acme", None, Some(60), &cancel)
                .await
                .is_some()
        );
        assert_eq!(Instant::now(), quiet);
    }

    #[tokio::test(start_paused = true)]
    async fn a_limit_of_one_serializes_sends() {
        let pacing = Arc::new(Pacing::new());
        let cancel = Cancel::new();
        let first = pacing.admit("img-a@acme", Some(1), None, &cancel).await;
        assert!(first.is_some());
        let waiting = {
            let pacing = Arc::clone(&pacing);
            let cancel = cancel.clone();
            tokio::spawn(async move {
                pacing
                    .admit("img-a@acme", Some(1), None, &cancel)
                    .await
                    .is_some()
            })
        };
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(!waiting.is_finished(), "the second send waits for the slot");
        // A later, larger limit does not change the route's semaphore.
        let third = {
            let pacing = Arc::clone(&pacing);
            let cancel = cancel.clone();
            tokio::spawn(async move {
                pacing
                    .admit("img-a@acme", Some(4), None, &cancel)
                    .await
                    .is_some()
            })
        };
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(!third.is_finished());
        // A route with no limit is never held up.
        assert!(
            pacing
                .admit("free@acme", None, None, &cancel)
                .await
                .is_some()
        );
        drop(first);
        assert!(waiting.await.unwrap());
        assert!(third.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_wait_returns_none_and_holds_nothing() {
        let pacing = Arc::new(Pacing::new());
        let cancel = Cancel::new();
        let held = pacing.admit("img-a@acme", Some(1), None, &cancel).await;
        assert!(held.is_some());
        let stopped = Cancel::new();
        let waiting = {
            let pacing = Arc::clone(&pacing);
            let stopped = stopped.clone();
            tokio::spawn(async move {
                pacing
                    .admit("img-a@acme", Some(1), None, &stopped)
                    .await
                    .is_none()
            })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        stopped.cancel();
        assert!(waiting.await.unwrap());
        // Cancelled already: nothing is waited for.
        assert!(
            pacing
                .admit("other@acme", None, None, &stopped)
                .await
                .is_none()
        );
        // Cancelled during the pacing wait: the slot it took is given back.
        drop(held);
        let _first = pacing
            .admit("paced@acme", Some(1), Some(1), &cancel)
            .await
            .unwrap();
        drop(_first);
        let late = Cancel::new();
        let paced = {
            let pacing = Arc::clone(&pacing);
            let late = late.clone();
            tokio::spawn(async move {
                pacing
                    .admit("paced@acme", Some(1), Some(1), &late)
                    .await
                    .is_none()
            })
        };
        tokio::time::sleep(Duration::from_secs(1)).await;
        late.cancel();
        assert!(paced.await.unwrap());
        let free = tokio::time::timeout(
            Duration::from_secs(120),
            pacing.admit("paced@acme", Some(1), None, &cancel),
        )
        .await;
        assert!(
            matches!(free, Ok(Some(_))),
            "the paced wait gave its slot back"
        );
    }
}
