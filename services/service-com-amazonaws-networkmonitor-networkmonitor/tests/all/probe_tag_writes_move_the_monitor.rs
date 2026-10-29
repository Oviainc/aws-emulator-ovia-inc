// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for the MONITOR record's `modifiedAt` under a TAGGING
//! call that lands in one of its probes.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rule under test: a
//! probe is part of the monitor record every read reports (`GetMonitor` lists
//! each probe with its own members and its own tags), so ANY accepted call that
//! rewrites a stored probe — a tag write or a tag removal addressed to the
//! probe's ARN as much as a create, update or delete of the probe — changed the
//! monitor too, and the monitor's `modifiedAt` moves to the same instant the
//! probe's does. A call that stored nothing (a write of pairs already stored, a
//! removal of keys the resource does not carry, an empty request) changed
//! neither record, so it moves neither `modifiedAt`; a call addressed to the
//! MONITOR's own ARN rewrites only the monitor, so the probe's `modifiedAt`
//! stays where it was; and `createdAt` never moves.
//!
//! The table spans the class rather than one recorded call: both tagging
//! operations, each addressed to a probe ARN and to a monitor ARN, each sent
//! both as a call that stores something and as one that stores nothing, over one
//! key, several keys at once and none, with an `UpdateProbe` row as the sibling
//! probe-writing path the rule is shared with.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::resource_tags::{
    call, fixture, get_monitor, only_probe, tag_resource, untag_resource, Target,
    TAG_MUTATION_STATUS,
};
use super::update_probe::timestamp;

/// A pause long enough that two instants either side of it are distinguishable
/// at the precision the wire format reports a timestamp with, and short enough
/// that the whole table stays well inside a probe's lifetime.
///
/// A blocking sleep rather than `tokio::time::sleep`: the workspace's `tokio`
/// dependency enables only `macros`, `rt` and `rt-multi-thread`, and the root
/// `Cargo.toml` that declares it is not editable from here, so the `time`
/// feature is not available. The handlers read the wall clock, so the wait has
/// to be real either way — the reason the sibling suites give on
/// `monitor_and_probe::PAST_ACTIVATION`.
const INSTANT_APART: Duration = Duration::from_millis(20);

/// One call a row makes.
#[derive(Debug, Clone)]
enum Call {
    /// `POST /tags/{resourceArn}` with these pairs, against the ARN of the
    /// named resource kind.
    Tag(Target, Value),
    /// `DELETE /tags/{resourceArn}?tagKeys=..` for these keys, against the ARN
    /// of the named resource kind.
    Untag(Target, Vec<&'static str>),
    /// `PATCH /monitors/{monitorName}/probes/{probeId}` with this body — the
    /// sibling path whose rule the tagging paths share.
    UpdateProbe(&'static str),
}

/// The `probeId` of the monitor's single probe.
fn probe_id_of(monitor: &Value) -> String {
    only_probe(monitor)
        .get("probeId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a reported probe must carry probeId, got {monitor:?}"))
        .to_string()
}

/// The monitor's `createdAt`/`modifiedAt` beside its only probe's, as one
/// `GetMonitor` reports them: the probes a read lists ARE the stored probe map,
/// so both records are read at one instant.
async fn timestamps(router: &MultiServiceRouter, name: &str) -> (f64, f64, f64, f64) {
    let (status, monitor) = get_monitor(router, name).await;
    assert_eq!(
        status, 200,
        "GetMonitor {name:?}: expected 200, got {status} {monitor:?}"
    );
    let probe = only_probe(&monitor);
    (
        timestamp(&monitor, "createdAt"),
        timestamp(&monitor, "modifiedAt"),
        timestamp(&probe, "createdAt"),
        timestamp(&probe, "modifiedAt"),
    )
}

/// A tagging call that rewrites a probe moves the containing MONITOR's
/// `modifiedAt` to the same instant as the probe's, and a call that stored
/// nothing moves neither.
#[tokio::test]
async fn a_probe_tag_write_moves_the_containing_monitors_modified_at() {
    // (case, the call the row makes, whether the monitor record changed,
    //  whether the probe record changed)
    let rows: Vec<(&str, Call, bool, bool)> = vec![
        (
            "the recorded case: TagResource on the probe ARN adds a key the probe does not carry",
            Call::Tag(Target::Probe, json!({"owner": "net"})),
            true,
            true,
        ),
        (
            "TagResource on the probe ARN assigns the value the key already has",
            Call::Tag(Target::Probe, json!({"owner": "net"})),
            false,
            false,
        ),
        (
            "TagResource on the probe ARN gives a stored key a new value",
            Call::Tag(Target::Probe, json!({"owner": "ops"})),
            true,
            true,
        ),
        (
            "TagResource on the probe ARN carries no pair at all",
            Call::Tag(Target::Probe, json!({})),
            false,
            false,
        ),
        (
            "TagResource on the probe ARN adds several keys at once",
            Call::Tag(Target::Probe, json!({"env": "prod", "tier": "edge"})),
            true,
            true,
        ),
        (
            "UntagResource on the probe ARN removes a key the probe does not carry",
            Call::Untag(Target::Probe, vec!["absent"]),
            false,
            false,
        ),
        (
            "UntagResource on the probe ARN removes a key the probe carries",
            Call::Untag(Target::Probe, vec!["owner"]),
            true,
            true,
        ),
        (
            "UntagResource on the probe ARN removes two keys, one of them absent",
            Call::Untag(Target::Probe, vec!["env", "absent"]),
            true,
            true,
        ),
        (
            "TagResource on the MONITOR ARN rewrites only the monitor",
            Call::Tag(Target::Monitor, json!({"team": "ops"})),
            true,
            false,
        ),
        (
            "UntagResource on the MONITOR ARN rewrites only the monitor",
            Call::Untag(Target::Monitor, vec!["team"]),
            true,
            false,
        ),
        (
            "UntagResource on the MONITOR ARN removes a key it does not carry",
            Call::Untag(Target::Monitor, vec!["team"]),
            false,
            false,
        ),
        (
            "the sibling path: UpdateProbe rewrites the same probe",
            Call::UpdateProbe(r#"{"packetSize":1500}"#),
            true,
            true,
        ),
        (
            "UpdateProbe naming the value already stored",
            Call::UpdateProbe(r#"{"packetSize":1500}"#),
            false,
            false,
        ),
    ];

    let (router, name, monitor_arn, probe_arn) = router_with_fixture().await;
    let (status, monitor) = get_monitor(&router, &name).await;
    assert_eq!(
        status, 200,
        "setup GetMonitor: expected 200, got {status} {monitor:?}"
    );
    let probe_id = probe_id_of(&monitor);

    let (monitor_created_at, mut monitor_modified_at, probe_created_at, mut probe_modified_at) =
        timestamps(&router, &name).await;

    for (case, row, monitor_changed, probe_changed) in rows {
        // Every row's call lands at an instant the wire format can tell from the
        // previous one's, so "moved" and "stayed" are distinguishable.
        std::thread::sleep(INSTANT_APART);
        match &row {
            Call::Tag(target, tags) => {
                let arn = match target {
                    Target::Monitor => &monitor_arn,
                    Target::Probe => &probe_arn,
                };
                let (status, body) = tag_resource(&router, arn, tags).await;
                assert_eq!(
                    status, TAG_MUTATION_STATUS,
                    "case {case:?}: expected TagResource {TAG_MUTATION_STATUS}, got {status} {body:?}"
                );
            }
            Call::Untag(target, keys) => {
                let arn = match target {
                    Target::Monitor => &monitor_arn,
                    Target::Probe => &probe_arn,
                };
                let (status, body) = untag_resource(&router, arn, keys).await;
                assert_eq!(
                    status, TAG_MUTATION_STATUS,
                    "case {case:?}: expected UntagResource {TAG_MUTATION_STATUS}, got {status} {body:?}"
                );
            }
            Call::UpdateProbe(body) => {
                let (status, answer) = call(
                    &router,
                    "PATCH",
                    &format!("/monitors/{name}/probes/{probe_id}"),
                    Some(body),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected UpdateProbe 200, got {status} {answer:?}"
                );
            }
        }

        let (now_monitor_created_at, now_monitor_modified_at, now_probe_created_at, now_probe_modified_at) =
            timestamps(&router, &name).await;
        if monitor_changed {
            assert!(
                now_monitor_modified_at > monitor_modified_at,
                "case {case:?}: the call rewrote a record the monitor reports, so the monitor's \
                 modifiedAt must move past {monitor_modified_at}, got {now_monitor_modified_at}"
            );
        } else {
            assert!(
                (now_monitor_modified_at - monitor_modified_at).abs() < f64::EPSILON,
                "case {case:?}: the call stored nothing, so the monitor's modifiedAt must stay \
                 {monitor_modified_at}, got {now_monitor_modified_at}"
            );
        }
        if probe_changed {
            assert!(
                now_probe_modified_at > probe_modified_at,
                "case {case:?}: the call rewrote the probe, so the probe's modifiedAt must move \
                 past {probe_modified_at}, got {now_probe_modified_at}"
            );
            assert!(
                (now_monitor_modified_at - now_probe_modified_at).abs() < f64::EPSILON,
                "case {case:?}: one call changed both records, so the monitor's modifiedAt must \
                 be the probe's {now_probe_modified_at}, got {now_monitor_modified_at}"
            );
        } else {
            assert!(
                (now_probe_modified_at - probe_modified_at).abs() < f64::EPSILON,
                "case {case:?}: the call rewrote no probe, so the probe's modifiedAt must stay \
                 {probe_modified_at}, got {now_probe_modified_at}"
            );
        }
        assert!(
            (now_monitor_created_at - monitor_created_at).abs() < f64::EPSILON,
            "case {case:?}: a monitor's createdAt never moves, expected {monitor_created_at}, \
             got {now_monitor_created_at}"
        );
        assert!(
            (now_probe_created_at - probe_created_at).abs() < f64::EPSILON,
            "case {case:?}: a probe's createdAt never moves, expected {probe_created_at}, got \
             {now_probe_created_at}"
        );
        monitor_modified_at = now_monitor_modified_at;
        probe_modified_at = now_probe_modified_at;
    }
}

/// A monitor carrying one tag and one probe carrying one tag, with the ARNs the
/// service minted for each.
async fn router_with_fixture() -> (MultiServiceRouter, String, String, String) {
    let router = super::resource_tags::router();
    let name = "probe-tagged".to_string();
    let (monitor_arn, probe_arn) =
        fixture(&router, &name, &json!({"team": "net"}), &json!({})).await;
    (router, name, monitor_arn, probe_arn)
}
