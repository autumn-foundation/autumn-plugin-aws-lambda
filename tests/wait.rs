//! Tests for the readiness wait.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use autumn_plugin_aws_lambda::wait_until;

const MS: Duration = Duration::from_millis(1);

#[tokio::test(start_paused = true)]
async fn returns_true_at_once_when_ready() {
    let calls = Arc::new(AtomicU32::new(0));
    let c = calls.clone();
    let ready = wait_until(
        move || {
            c.fetch_add(1, Ordering::SeqCst);
            true
        },
        MS * 1000,
        MS * 10,
        MS * 100,
    )
    .await;
    assert!(ready);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn returns_true_when_ready_later() {
    let calls = Arc::new(AtomicU32::new(0));
    let c = calls.clone();
    let ready = wait_until(
        move || c.fetch_add(1, Ordering::SeqCst) >= 3,
        MS * 1000,
        MS * 10,
        MS * 100,
    )
    .await;
    assert!(ready);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test(start_paused = true)]
async fn returns_false_after_timeout_with_bounded_probes() {
    let calls = Arc::new(AtomicU32::new(0));
    let c = calls.clone();
    let start = tokio::time::Instant::now();
    let ready = wait_until(
        move || {
            c.fetch_add(1, Ordering::SeqCst);
            false
        },
        MS * 1000,
        MS * 10,
        MS * 100,
    )
    .await;
    assert!(!ready);
    // Verus bound: probes <= timeout / initial + 2.
    assert!(calls.load(Ordering::SeqCst) <= 1000 / 10 + 2);
    assert!(start.elapsed() >= MS * 1000);
    assert!(start.elapsed() <= MS * 1100);
}

#[tokio::test(start_paused = true)]
async fn zero_timeout_probes_once() {
    let calls = Arc::new(AtomicU32::new(0));
    let c = calls.clone();
    let ready = wait_until(
        move || {
            c.fetch_add(1, Ordering::SeqCst);
            false
        },
        Duration::ZERO,
        MS * 10,
        MS * 100,
    )
    .await;
    assert!(!ready);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn zero_initial_delay_does_not_spin() {
    let calls = Arc::new(AtomicU32::new(0));
    let c = calls.clone();
    let ready = wait_until(
        move || {
            c.fetch_add(1, Ordering::SeqCst);
            false
        },
        MS * 100,
        Duration::ZERO,
        Duration::ZERO,
    )
    .await;
    assert!(!ready);
    assert!(calls.load(Ordering::SeqCst) <= 102);
}
