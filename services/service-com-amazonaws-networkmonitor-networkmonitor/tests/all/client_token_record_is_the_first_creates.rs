// SPDX-License-Identifier: CC0-1.0

//! Which create a `clientToken`'s record belongs to, driven end-to-end through
//! both creates.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The one rule under test: a
//! token's record is the record of the create that used it FIRST, and stays that
//! create's for as long as the resource it minted is stored — a later create
//! carrying the same token is a request of its own and does NOT take the record
//! over, so a retry of the first create keeps replaying the resource that create
//! minted rather than being judged a fresh request (which would conflict, or
//! mint a duplicate). Once the recorded resource is gone the record replays
//! nothing, so the next create using that token records what IT minted. A token
//! already spoken for therefore buys the later create no idempotency of its own:
//! that create, and every repeat of it, is judged exactly as a request carrying
//! no token is.
//!
//! The tables span the class rather than one sequence: either kind of create
//! recording first, one or several later creates reusing the token, of either
//! kind, and token spellings from one character to two hundred.

use super::client_token_replay::{monitor_body, probe_body};
use super::monitor_and_probe::{
    create_monitor, create_probe, error_code, get_monitor, probes_of, router, string_at,
    PAST_ACTIVATION, PAST_TEARDOWN,
};
use super::probe_teardown::delete_probe;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// A later create that reuses a token an earlier create already recorded. Each
/// one succeeds on its own terms (it names a resource of its own), and none of
/// them is a retry of the create that recorded the token.
#[derive(Debug, Clone, Copy)]
enum Reuse {
    /// `CreateMonitor` under a monitor name of its own.
    Monitor,
    /// `CreateProbe` on a monitor of its own.
    Probe,
}

/// The sequences of later creates each table drives, spanning how many reuse the
/// token and of which kind.
fn reuse_sequences() -> Vec<(&'static str, String, Vec<Reuse>)> {
    vec![
        (
            "one later monitor create",
            "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0".to_string(),
            vec![Reuse::Monitor],
        ),
        (
            "one later probe create",
            "Retry_Token.42+/=".to_string(),
            vec![Reuse::Probe],
        ),
        ("a one-character token", "t".to_string(), vec![Reuse::Probe]),
        (
            "three later creates of both kinds",
            "reused-three-times".to_string(),
            vec![Reuse::Monitor, Reuse::Probe, Reuse::Monitor],
        ),
        (
            "a 200-character token",
            "a".repeat(200),
            vec![Reuse::Monitor, Reuse::Monitor],
        ),
    ]
}

/// Create a monitor carrying no token, as a host for the probes a case mints.
async fn host_monitor(router: &MultiServiceRouter, name: &str, case: &str) {
    let (status, created) =
        create_monitor(router, &format!(r#"{{"monitorName":"{name}"}}"#)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the host monitor {name:?} to be created, got {status} {created:?}"
    );
}

/// Drive one sequence of later creates, each reusing `token` for a resource of
/// its own; `label` keeps the names each case mints apart.
async fn drive_reuses(
    router: &MultiServiceRouter,
    case: &str,
    label: &str,
    token: &str,
    reuses: &[Reuse],
) {
    for (position, reuse) in reuses.iter().enumerate() {
        match reuse {
            Reuse::Monitor => {
                let name = format!("{label}-later-{position}");
                let (status, later) =
                    create_monitor(router, &monitor_body(&name, token, 60)).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the later CreateMonitor {name:?} 200, got {status} {later:?}"
                );
            }
            Reuse::Probe => {
                let host = format!("{label}-host-{position}");
                host_monitor(router, &host, case).await;
                let (status, later) =
                    create_probe(router, &host, &probe_body(token, "10.0.0.9")).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the later CreateProbe on {host:?} 200, got {status} {later:?}"
                );
            }
        }
    }
}

/// The number of probes `GetMonitor` reports for a monitor.
async fn probe_count(router: &MultiServiceRouter, name: &str) -> usize {
    let (status, body) = get_monitor(router, name).await;
    assert_eq!(
        status, 200,
        "expected GetMonitor {name:?} 200, got {status} {body:?}"
    );
    probes_of(&body).len()
}

/// A monitor create's token keeps replaying the monitor it minted, however many
/// later creates reuse that token.
#[tokio::test]
async fn a_monitor_token_still_replays_its_monitor_after_later_creates_reuse_it() {
    let router = router();

    for (index, (case, token, reuses)) in reuse_sequences().into_iter().enumerate() {
        let label = format!("mon-{index}");
        let name = format!("{label}-first");
        let body = monitor_body(&name, &token, 30);

        let (status, first) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first CreateMonitor 200, got {status} {first:?}"
        );

        drive_reuses(&router, case, &label, &token, &reuses).await;

        // The retry of the FIRST create: byte-identical, same token, so it
        // replays the monitor that token minted rather than conflicting with it.
        let (status, replayed) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the retry of the first create 200, got {status} {replayed:?}"
        );
        assert_eq!(
            replayed, first,
            "case {case:?}: expected the retry to replay {first:?}, got {replayed:?}"
        );
    }
}

/// A probe create's token keeps replaying the probe it minted, however many
/// later creates reuse that token — so the retry mints no duplicate.
#[tokio::test]
async fn a_probe_token_still_replays_its_probe_after_later_creates_reuse_it() {
    let router = router();

    for (index, (case, token, reuses)) in reuse_sequences().into_iter().enumerate() {
        let label = format!("prb-{index}");
        let owner = format!("{label}-owner");
        host_monitor(&router, &owner, case).await;

        let body = probe_body(&token, "10.0.0.1");
        let (status, first) = create_probe(&router, &owner, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first CreateProbe 200, got {status} {first:?}"
        );

        drive_reuses(&router, case, &label, &token, &reuses).await;

        let (status, replayed) = create_probe(&router, &owner, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the retry of the first CreateProbe 200, got {status} {replayed:?}"
        );
        assert_eq!(
            replayed, first,
            "case {case:?}: expected the retry to replay {first:?}, got {replayed:?}"
        );
        let held = probe_count(&router, &owner).await;
        assert_eq!(
            held, 1,
            "case {case:?}: expected the retry to mint no second probe on {owner:?}, got {held} probes"
        );
    }
}

/// A later create reusing a token is still a create of its own: it is answered
/// from the resource IT asks for, not from the record the token holds.
#[tokio::test]
async fn a_later_create_reusing_a_token_is_answered_from_its_own_request() {
    let router = router();

    let (status, first) = create_monitor(&router, &monitor_body("recorded", "shared", 30)).await;
    assert_eq!(
        status, 200,
        "setup: expected the first CreateMonitor 200, got {status} {first:?}"
    );

    let (status, later) = create_monitor(&router, &monitor_body("borrower", "shared", 60)).await;
    assert_eq!(
        status, 200,
        "case \"a later monitor create\": expected 200, got {status} {later:?}"
    );
    assert_eq!(
        string_at(&later, "monitorName"),
        "borrower",
        "case \"a later monitor create\": expected the name it asked for, got {later:?}"
    );
    assert_eq!(
        later.get("aggregationPeriod").and_then(Value::as_i64),
        Some(60),
        "case \"a later monitor create\": expected the period it asked for, got {later:?}"
    );

    let (status, probe) = create_probe(&router, "borrower", &probe_body("shared", "10.0.0.2")).await;
    assert_eq!(
        status, 200,
        "case \"a later probe create\": expected 200, got {status} {probe:?}"
    );
    let held = probe_count(&router, "borrower").await;
    assert_eq!(
        held, 1,
        "case \"a later probe create\": expected the probe it asked for, got {held} probes"
    );

    // And the record is still the first create's, so its retry replays it.
    let (status, replayed) = create_monitor(&router, &monitor_body("recorded", "shared", 30)).await;
    assert_eq!(
        status, 200,
        "case \"the retry of the first create\": expected 200, got {status} {replayed:?}"
    );
    assert_eq!(
        replayed, first,
        "case \"the retry of the first create\": expected {first:?}, got {replayed:?}"
    );
}

/// A token already spoken for buys the LATER create no idempotency of its own:
/// that create keeps the record it did not get, so each repeat of it is judged
/// on its own — exactly as a repeat carrying no token is. The record this leaves
/// intact is the first create's, which is what the token identifies.
#[tokio::test]
async fn a_later_create_reusing_a_token_is_judged_on_its_own_each_time() {
    let router = router();

    let (status, first) = create_monitor(&router, &monitor_body("holder", "spoken-for", 30)).await;
    assert_eq!(
        status, 200,
        "setup: expected the first CreateMonitor 200, got {status} {first:?}"
    );

    // A later monitor create reusing the token stores a monitor of its own, so a
    // repeat of THAT create meets its own stored name and conflicts, as a
    // tokenless repeat does.
    let later = monitor_body("latecomer", "spoken-for", 60);
    let (status, created) = create_monitor(&router, &later).await;
    assert_eq!(
        status, 200,
        "case \"a later monitor create\": expected 200, got {status} {created:?}"
    );
    let (status, refused) = create_monitor(&router, &later).await;
    assert_eq!(
        status, 409,
        "case \"a repeat of the later monitor create\": expected 409, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictException",
        "case \"a repeat of the later monitor create\": expected ConflictException, got {refused:?}"
    );

    // And a later probe create reusing it mints a probe of its own each time,
    // again as a tokenless probe create does.
    host_monitor(&router, "latehost", "a later probe create").await;
    let probe = probe_body("spoken-for", "10.0.0.3");
    let (status, minted) = create_probe(&router, "latehost", &probe).await;
    assert_eq!(
        status, 200,
        "case \"a later probe create\": expected 200, got {status} {minted:?}"
    );
    let (status, again) = create_probe(&router, "latehost", &probe).await;
    assert_eq!(
        status, 200,
        "case \"a repeat of the later probe create\": expected 200, got {status} {again:?}"
    );
    assert_ne!(
        string_at(&again, "probeId"),
        string_at(&minted, "probeId"),
        "case \"a repeat of the later probe create\": expected a probe of its own, got {again:?}"
    );
    let held = probe_count(&router, "latehost").await;
    assert_eq!(
        held, 2,
        "case \"a repeat of the later probe create\": expected both minted probes, got {held} probes"
    );

    // Through all of it the token still identifies the FIRST create.
    let (status, replayed) =
        create_monitor(&router, &monitor_body("holder", "spoken-for", 30)).await;
    assert_eq!(
        status, 200,
        "case \"the retry of the first create\": expected 200, got {status} {replayed:?}"
    );
    assert_eq!(
        replayed, first,
        "case \"the retry of the first create\": expected {first:?}, got {replayed:?}"
    );
}

/// A record that replays nothing any more is spent: once the probe a token
/// recorded has been torn down and forgotten, the next create carrying that
/// token records the probe IT mints, so the retry after it replays that probe
/// instead of minting a third.
#[tokio::test]
async fn a_token_whose_probe_is_gone_records_the_probe_the_next_create_mints() {
    let router = router();

    host_monitor(&router, "respawn", "a spent record").await;
    let body = probe_body("spent", "10.0.0.7");
    let (status, first) = create_probe(&router, "respawn", &body).await;
    assert_eq!(
        status, 200,
        "setup: expected the first CreateProbe 200, got {status} {first:?}"
    );
    // A probe is given up only once it has left `PENDING`, so the delete below
    // waits for the clock to carry it into service first.
    std::thread::sleep(PAST_ACTIVATION);
    let (status, deleted) = delete_probe(&router, "respawn", &string_at(&first, "probeId")).await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteProbe 200, got {status} {deleted:?}"
    );
    // Past the whole teardown, so the probe the token recorded is not stored any
    // more and that record replays nothing.
    std::thread::sleep(PAST_TEARDOWN);

    let (status, minted) = create_probe(&router, "respawn", &body).await;
    assert_eq!(
        status, 200,
        "case \"a spent record\": expected the repeat to mint a probe, got {status} {minted:?}"
    );
    assert_ne!(
        string_at(&minted, "probeId"),
        string_at(&first, "probeId"),
        "case \"a spent record\": expected a newly minted probe, got {minted:?}"
    );

    let (status, replayed) = create_probe(&router, "respawn", &body).await;
    assert_eq!(
        status, 200,
        "case \"a retry after the spent record was replaced\": expected 200, got {status} {replayed:?}"
    );
    assert_eq!(
        replayed, minted,
        "case \"a retry after the spent record was replaced\": expected {minted:?}, got {replayed:?}"
    );
    let held = probe_count(&router, "respawn").await;
    assert_eq!(
        held, 1,
        "case \"a retry after the spent record was replaced\": expected the one minted probe, got {held} probes"
    );
}
