// SPDX-License-Identifier: CC0-1.0

//! What an idempotency token replays, driven end-to-end through BOTH creates.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The one rule under test,
//! which `CreateMonitor` and `CreateProbe` now share: a repeat carrying a
//! `clientToken` is a retry of exactly the resource that token already minted —
//! the same kind of resource, under the same monitor name, and still stored —
//! and every other repeat is judged as if it carried no token at all (a second
//! create of a stored monitor name conflicts; a second `CreateProbe` mints a
//! second probe).
//!
//! The tables span the class rather than one token: four spellings of a token,
//! the same token under another name, a token of the other create's kind, and a
//! token whose monitor has since been deleted — which is the case where a
//! per-monitor copy of the token would disagree with the records that decide.

use serde_json::Value;

use super::monitor_and_probe::{
    create_monitor, create_probe, error_code, get_monitor, probe_json, probes_of, router, string_at,
};
use super::resource_tags::teardown_monitor;

/// A `CreateMonitor` body carrying a token and an explicit `aggregationPeriod`,
/// so a replayed answer is distinguishable from a fresh create's.
///
/// `pub(crate)` so the suite that covers which create a token's record belongs
/// to (`client_token_record_is_the_first_creates`) drives the same bodies.
pub(crate) fn monitor_body(name: &str, token: &str, aggregation_period: i64) -> String {
    format!(
        r#"{{"monitorName":"{name}","clientToken":"{token}","aggregationPeriod":{aggregation_period}}}"#
    )
}

/// A `CreateProbe` body carrying a token. `pub(crate)` for the same reason as
/// [`monitor_body`].
pub(crate) fn probe_body(token: &str, destination: &str) -> String {
    format!(
        r#"{{"clientToken":"{token}","probe":{}}}"#,
        probe_json(destination, "ICMP", r#","packetSize":56"#)
    )
}

/// The `aggregationPeriod` an answer reports.
fn period_of(body: &Value) -> Option<i64> {
    body.get("aggregationPeriod").and_then(Value::as_i64)
}

/// Spellings a caller may pick for a token: the model types `clientToken` as a
/// plain `String` with `@idempotencyToken` and no length or pattern constraint
/// (`smithy_query(query="operation",operation="CreateMonitor")`), so the rule
/// holds for any of them — none is the SDK's generated UUID in particular.
fn token_spellings() -> Vec<(&'static str, String)> {
    vec![
        (
            "uuid",
            "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0".to_string(),
        ),
        ("one character", "t".to_string()),
        (
            "mixed case and punctuation",
            "Retry_Token.42+/=".to_string(),
        ),
        ("200 characters", "a".repeat(200)),
    ]
}

/// A monitor create's token replays the monitor it minted, for any spelling of
/// the token — and replays nothing for a create of a DIFFERENT name, which is a
/// create of its own.
#[tokio::test]
async fn a_monitor_token_replays_the_monitor_it_minted_whatever_its_spelling() {
    let router = router();

    for (index, (case, token)) in token_spellings().into_iter().enumerate() {
        let name = format!("tok-{index}");
        let body = monitor_body(&name, &token, 30);

        let (status, first) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first CreateMonitor 200, got {status} {first:?}"
        );
        let (status, replayed) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the retry 200, got {status} {replayed:?}"
        );
        assert_eq!(
            replayed, first,
            "case {case:?}: expected the retry to replay {first:?}, got {replayed:?}"
        );

        // The same token under ANOTHER name replays nothing: it minted the
        // monitor above, not whatever the caller asks for next.
        let other = format!("tok-{index}-other");
        let (status, fresh) = create_monitor(&router, &monitor_body(&other, &token, 60)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a create under another name 200, got {status} {fresh:?}"
        );
        assert_eq!(
            string_at(&fresh, "monitorName"),
            other,
            "case {case:?}: expected the new name {other:?}, got {fresh:?}"
        );
        assert_eq!(
            period_of(&fresh),
            Some(60),
            "case {case:?}: expected the new request's aggregationPeriod 60, got {fresh:?}"
        );
    }
}

/// A repeat of a stored monitor name that does NOT replay a monitor token is
/// refused, whatever token it carries.
#[tokio::test]
async fn a_repeat_create_that_replays_no_monitor_token_conflicts() {
    let router = router();

    let (status, created) = create_monitor(&router, &monitor_body("taken", "minting", 30)).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateMonitor 200, got {status} {created:?}"
    );
    // A token this region never saw, and a token that minted a PROBE rather
    // than a monitor: neither is a retry of the create above.
    let probe = probe_body("probe-minting", "10.0.0.4");
    let (status, minted) = create_probe(&router, "taken", &probe).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateProbe 200, got {status} {minted:?}"
    );

    let cases: [(&str, String); 3] = [
        ("no token", r#"{"monitorName":"taken"}"#.to_string()),
        (
            "a token nothing minted",
            monitor_body("taken", "never-sent", 30),
        ),
        (
            "a token that minted a probe",
            monitor_body("taken", "probe-minting", 30),
        ),
    ];
    for (case, body) in cases {
        let (status, refused) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused:?}"
        );
    }
}

/// A probe create's token replays the probe it minted on THAT monitor; carried
/// on another monitor, or standing for a monitor create, it replays nothing and
/// a probe is minted.
#[tokio::test]
async fn a_probe_token_replays_the_probe_it_minted_and_nothing_else() {
    let router = router();

    for name in ["host", "other"] {
        let (status, created) =
            create_monitor(&router, &format!(r#"{{"monitorName":"{name}"}}"#)).await;
        assert_eq!(
            status, 200,
            "setup {name:?}: expected CreateMonitor 200, got {status} {created:?}"
        );
    }

    let body = probe_body("probe-token", "10.0.0.1");
    let (status, first) = create_probe(&router, "host", &body).await;
    assert_eq!(
        status, 200,
        "expected the first CreateProbe 200, got {status} {first:?}"
    );
    let (status, replayed) = create_probe(&router, "host", &body).await;
    assert_eq!(
        status, 200,
        "case \"same monitor\": expected the retry 200, got {status} {replayed:?}"
    );
    assert_eq!(
        string_at(&replayed, "probeId"),
        string_at(&first, "probeId"),
        "case \"same monitor\": expected the retry to replay probe {:?}, got {replayed:?}",
        string_at(&first, "probeId")
    );
    let (_, host) = get_monitor(&router, "host").await;
    assert_eq!(
        probes_of(&host).len(),
        1,
        "case \"same monitor\": the retry must mint no second probe, got {host:?}"
    );

    // The same token on ANOTHER monitor: the record names a probe of `host`, so
    // there is nothing to replay here and a probe is minted.
    let (status, elsewhere) = create_probe(&router, "other", &body).await;
    assert_eq!(
        status, 200,
        "case \"another monitor\": expected 200, got {status} {elsewhere:?}"
    );
    assert_ne!(
        string_at(&elsewhere, "probeId"),
        string_at(&first, "probeId"),
        "case \"another monitor\": expected a probe of its own, got {elsewhere:?}"
    );
    let (_, other) = get_monitor(&router, "other").await;
    assert_eq!(
        probes_of(&other).len(),
        1,
        "case \"another monitor\": expected one probe minted there, got {other:?}"
    );

    // A token that minted the MONITOR itself replays no probe: the record it
    // left names no probe, so a `CreateProbe` carrying it mints one.
    let (status, created) =
        create_monitor(&router, &monitor_body("mixed", "monitor-token", 30)).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateMonitor 200, got {status} {created:?}"
    );
    let (status, minted) =
        create_probe(&router, "mixed", &probe_body("monitor-token", "10.0.0.2")).await;
    assert_eq!(
        status, 200,
        "case \"a token that minted a monitor\": expected 200, got {status} {minted:?}"
    );
    let (_, mixed) = get_monitor(&router, "mixed").await;
    assert_eq!(
        probes_of(&mixed).len(),
        1,
        "case \"a token that minted a monitor\": expected the probe to be minted, got {mixed:?}"
    );
}

/// A token whose monitor has been DELETED replays nothing: the records that
/// decide a retry are forgotten with the monitor, so a repeat of the same
/// request mints a new monitor and is answered from THAT one.
#[tokio::test]
async fn a_token_whose_monitor_was_deleted_replays_nothing() {
    let router = router();

    let (status, first) = create_monitor(&router, &monitor_body("recreated", "kept", 30)).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateMonitor 200, got {status} {first:?}"
    );
    let (status, minted) =
        create_probe(&router, "recreated", &probe_body("kept-probe", "10.0.0.3")).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateProbe 200, got {status} {minted:?}"
    );

    // A monitor that still holds a probe in service is not deleted, so the probe
    // is switched off first ([`teardown_monitor`]).
    let (status, deleted) = teardown_monitor(&router, "recreated").await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteMonitor 200, got {status} {deleted:?}"
    );

    // Same token, same name, a different aggregationPeriod: a replay of the
    // deleted monitor would answer 30, a fresh create answers 60.
    let (status, again) = create_monitor(&router, &monitor_body("recreated", "kept", 60)).await;
    assert_eq!(
        status, 200,
        "case \"re-create after a delete\": expected 200, got {status} {again:?}"
    );
    assert_eq!(
        period_of(&again),
        Some(60),
        "case \"re-create after a delete\": expected the new request's aggregationPeriod 60, got {again:?}"
    );
    let (status, got) = get_monitor(&router, "recreated").await;
    assert_eq!(
        status, 200,
        "case \"re-create after a delete\": expected GetMonitor 200, got {status} {got:?}"
    );
    assert_eq!(
        period_of(&got),
        Some(60),
        "case \"re-create after a delete\": the stored monitor must be the new one, got {got:?}"
    );
    assert!(
        probes_of(&got).is_empty(),
        "case \"re-create after a delete\": the new monitor holds no probe, got {got:?}"
    );

    // And the token now replays the monitor it has just minted.
    let (status, replayed) = create_monitor(&router, &monitor_body("recreated", "kept", 60)).await;
    assert_eq!(
        status, 200,
        "case \"retry of the re-create\": expected 200, got {status} {replayed:?}"
    );
    assert_eq!(
        replayed, again,
        "case \"retry of the re-create\": expected the retry to replay {again:?}, got {replayed:?}"
    );

    // The probe token of the deleted monitor replays nothing either: its probe
    // went with the monitor, so a repeat on the re-created monitor mints one.
    let (status, probe) =
        create_probe(&router, "recreated", &probe_body("kept-probe", "10.0.0.3")).await;
    assert_eq!(
        status, 200,
        "case \"probe token after the delete\": expected 200, got {status} {probe:?}"
    );
    assert_ne!(
        string_at(&probe, "probeId"),
        string_at(&minted, "probeId"),
        "case \"probe token after the delete\": expected a newly minted probe, got {probe:?}"
    );
}
