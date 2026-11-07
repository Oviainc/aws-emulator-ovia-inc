// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for what the TAGGING mutations do to a resource whose
//! teardown has already been accepted.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rule under test: a
//! resource whose asynchronous teardown has been accepted is no longer a target
//! for any call that would change what it still reports, so every such call —
//! `TagResource` and `UntagResource` beside the `UpdateProbe` already pinned by
//! `probe_teardown.rs` — is refused with the 404 the tag APIs state
//! (`NotFoundException`) and stores nothing, while the reporting calls
//! (`GetProbe`, `GetMonitor`, `ListTagsForResource`) keep reporting the record,
//! in the teardown state it has reached and with the tags and `modifiedAt` the
//! accepted delete left on it, until the clock has carried the teardown through.
//!
//! The refusal belongs to the resource on its way out and not to the tagging
//! route: a live monitor and a live probe beside a draining one are still
//! tagged and untagged, which `a_live_resource_beside_a_draining_probe_is_still_tagged`
//! pins.
//!
//! The tables span the class the rule covers rather than one recorded call:
//! every shape of tag mutation (a key added, a key overwritten, a write that
//! would change nothing, an empty write, several keys at once, a key removed, a
//! key removed that is not carried, several keys removed at once), both address
//! families and both protocols, `packetSize` at the ceiling of its published
//! range, and both phases of a teardown a record is readable in
//! (`DELETING` and `DELETED`) beside the phase after it, where the record is
//! gone.

use std::time::Instant;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    create_monitor, create_probe, error_code, get_monitor, probe_json, router, string_at,
    PAST_ACTIVATION, PAST_TEARDOWN,
};
use super::probe_teardown::{delete_probe, get_probe, probe_fixture, PAST_FIRST_TEARDOWN_STAGE};
use super::resource_tags::{
    list_tags, tag_resource, untag_resource, TAG_MUTATION_STATUS, TAG_NOT_FOUND_CODE,
};
use super::update_probe::timestamp;

/// The tags every fixture's probe carries before its teardown is accepted,
/// written through `TagResource` while the probe is still a target for one.
fn baseline_tags() -> Value {
    json!({"stage": "live", "team": "net"})
}

/// One tagging MUTATION, as the wire call that performs it.
#[derive(Debug, Clone)]
enum Mutation {
    /// `POST /tags/{resourceArn}` carrying these pairs.
    Tag(Value),
    /// `DELETE /tags/{resourceArn}?tagKeys=…` naming these keys.
    Untag(&'static [&'static str]),
}

/// Perform one tagging mutation against `resource_arn`.
async fn mutate(
    router: &MultiServiceRouter,
    resource_arn: &str,
    mutation: &Mutation,
) -> (u16, Value) {
    match mutation {
        Mutation::Tag(tags) => tag_resource(router, resource_arn, tags).await,
        Mutation::Untag(keys) => untag_resource(router, resource_arn, keys).await,
    }
}

/// Every shape of tag mutation, against the probe's [`baseline_tags`]: what the
/// rule generalises over is the pairs or keys the call carries, so the table
/// holds a row for each way a mutation can relate to the stored map — including
/// the two that would change nothing at all.
fn mutations() -> Vec<(&'static str, Mutation)> {
    vec![
        (
            "TagResource adds a key the probe does not carry",
            Mutation::Tag(json!({"owner": "net-eng"})),
        ),
        (
            "TagResource overwrites a key the probe carries",
            Mutation::Tag(json!({"stage": "draining"})),
        ),
        (
            "TagResource assigns the value a carried key already has",
            Mutation::Tag(json!({"stage": "live"})),
        ),
        ("TagResource carries an empty tag map", Mutation::Tag(json!({}))),
        (
            "TagResource writes several keys at once",
            Mutation::Tag(json!({"a": "1", "b": "2", "c": "3"})),
        ),
        (
            "UntagResource removes a key the probe carries",
            Mutation::Untag(&["stage"]),
        ),
        (
            "UntagResource removes a key the probe does not carry",
            Mutation::Untag(&["absent"]),
        ),
        (
            "UntagResource removes several keys at once",
            Mutation::Untag(&["stage", "team", "absent"]),
        ),
    ]
}

/// Assert one tagging mutation of `resource_arn` is refused with the 404 the tag
/// APIs state: the ARN reaches no resource this call may change.
async fn assert_refused(
    router: &MultiServiceRouter,
    resource_arn: &str,
    mutation: &Mutation,
    case: &str,
) {
    let (status, refused) = mutate(router, resource_arn, mutation).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the tag mutation of a resource on its way out to be 404, \
         got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        TAG_NOT_FOUND_CODE,
        "case {case:?}: expected {TAG_NOT_FOUND_CODE}, got {refused:?}"
    );
}

/// Assert the draining probe is still reported, with exactly `want_tags` and
/// with its `modifiedAt` still at `want_modified_at` — the two values a refused
/// mutation must have left alone.
async fn assert_probe_unchanged(
    router: &MultiServiceRouter,
    monitor_name: &str,
    probe_id: &str,
    probe_arn: &str,
    want_tags: &Value,
    want_modified_at: f64,
    case: &str,
) {
    let (status, got) = get_probe(router, monitor_name, probe_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the draining probe is still readable, got {status} {got:?}"
    );
    assert_eq!(
        got.get("tags"),
        Some(want_tags),
        "case {case:?}: expected the stored tags to be {want_tags:?}, got {got:?}"
    );
    let modified_at = timestamp(&got, "modifiedAt");
    assert!(
        (modified_at - want_modified_at).abs() < f64::EPSILON,
        "case {case:?}: a refused call changed nothing, so modifiedAt must stay \
         {want_modified_at}, got {modified_at} in {got:?}"
    );
    let (status, tags) = list_tags(router, probe_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a draining probe's ARN still resolves for a read, got {status} {tags:?}"
    );
    assert_eq!(
        tags.get("tags"),
        Some(want_tags),
        "case {case:?}: ListTagsForResource reports the same stored map, expected \
         {want_tags:?}, got {tags:?}"
    );
}

/// Create a monitor holding one probe, wait for that probe to leave `PENDING` —
/// a probe still being provisioned is no target for a delete at all — tag it
/// with [`baseline_tags`] while it is still a target for a tag write, accept its
/// delete, and answer the probe's id and ARN beside the `modifiedAt` the
/// accepted delete left on it.
async fn draining_probe_fixture(
    router: &MultiServiceRouter,
    monitor_name: &str,
    probe: &str,
    case: &str,
) -> (String, String, f64) {
    let (probe_id, probe_arn) = probe_fixture(router, monitor_name, probe).await;
    std::thread::sleep(PAST_ACTIVATION);
    let (_, settled) = get_probe(router, monitor_name, &probe_id).await;
    assert_eq!(
        string_at(&settled, "state"),
        "ACTIVE",
        "case {case:?}: the probe must be live before the delete, got {settled:?}"
    );
    let (status, written) = tag_resource(router, &probe_arn, &baseline_tags()).await;
    assert_eq!(
        status, TAG_MUTATION_STATUS,
        "case {case:?}: a probe still in service is tagged, got {status} {written:?}"
    );

    let (status, accepted) = delete_probe(router, monitor_name, &probe_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteProbe 200, got {status} {accepted:?}"
    );
    let (status, draining) = get_probe(router, monitor_name, &probe_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a probe whose delete was accepted is still readable, \
         got {status} {draining:?}"
    );
    assert_eq!(
        string_at(&draining, "state"),
        "DELETING",
        "case {case:?}: expected the accepted delete to start the teardown, got {draining:?}"
    );
    (probe_id, probe_arn, timestamp(&draining, "modifiedAt"))
}

/// A tag mutation of a probe whose teardown has been accepted is refused with
/// the tag APIs' 404 and stores nothing: the probe keeps the tags and the
/// `modifiedAt` the accepted delete left on it.
///
/// The table spans every shape of tag mutation, since what the rule generalises
/// over is the pairs or keys the call carries.
#[tokio::test]
async fn a_tag_mutation_of_a_draining_probe_is_refused_and_stores_nothing() {
    for (case, mutation) in mutations() {
        let router = router();
        let (probe_id, probe_arn, torn_down_at) = draining_probe_fixture(
            &router,
            "draining-tags",
            &probe_json("10.0.4.1", "ICMP", r#","packetSize":56"#),
            case,
        )
        .await;

        assert_refused(&router, &probe_arn, &mutation, case).await;
        assert_probe_unchanged(
            &router,
            "draining-tags",
            &probe_id,
            &probe_arn,
            &baseline_tags(),
            torn_down_at,
            case,
        )
        .await;

        // The monitor reports the same record as `GetProbe`, so the refused
        // mutation is invisible there too.
        let (status, monitor) = get_monitor(&router, "draining-tags").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {monitor:?}"
        );
        let listed = monitor
            .get("probes")
            .and_then(Value::as_array)
            .and_then(|probes| {
                probes
                    .iter()
                    .find(|probe| string_at(probe, "probeId") == probe_id)
                    .cloned()
            })
            .unwrap_or_else(|| {
                panic!("case {case:?}: the monitor must still list probe {probe_id}, got {monitor:?}")
            });
        assert_eq!(
            listed.get("tags"),
            Some(&baseline_tags()),
            "case {case:?}: GetMonitor reports the tags the delete left, got {listed:?}"
        );
    }
}

/// The refusal holds through every phase of a teardown and for every shape of
/// probe: while the record reports `DELETING`, once it reports `DELETED`, and —
/// as for any ARN no stored resource carries — once the record is gone.
///
/// The table spans both address families and both protocols (so a probe with a
/// `destinationPort` and one without) and `packetSize` at the ceiling of its
/// published `@range(56..8500)`.
#[tokio::test]
async fn the_refusal_holds_through_every_phase_of_the_teardown() {
    // (case, probe body)
    let cases: Vec<(&str, String)> = vec![
        (
            "the recorded case: a live ICMP IPv4 probe",
            probe_json("10.0.5.1", "ICMP", r#","packetSize":56"#),
        ),
        (
            "a TCP IPv6 probe with a destinationPort",
            probe_json("2001:db8::1", "TCP", r#","destinationPort":443,"packetSize":56"#),
        ),
        (
            "a live ICMP probe at the packetSize ceiling",
            probe_json("10.0.5.2", "ICMP", r#","packetSize":8500"#),
        ),
    ];
    // One row of each kind of mutation is enough here: the mutation dimension is
    // spanned by `a_tag_mutation_of_a_draining_probe_is_refused_and_stores_nothing`.
    let tag = Mutation::Tag(json!({"phase": "late"}));
    let untag = Mutation::Untag(&["team"]);

    for (case, probe) in cases {
        let router = router();
        let (probe_id, probe_arn, torn_down_at) =
            draining_probe_fixture(&router, "teardown-phases", &probe, case).await;
        run_phases(
            &router, "teardown-phases", &probe_id, &probe_arn, torn_down_at, &tag, &untag, case,
        )
        .await;
    }
}

/// Walk one accepted teardown through its phases, refusing both mutations in
/// each: the record reporting `DELETING`, the record reporting `DELETED`, and
/// the record gone.
#[allow(clippy::too_many_arguments)]
async fn run_phases(
    router: &MultiServiceRouter,
    monitor_name: &str,
    probe_id: &str,
    probe_arn: &str,
    torn_down_at: f64,
    tag: &Mutation,
    untag: &Mutation,
    case: &str,
) {
    let accepted_at = Instant::now();
    for mutation in [tag, untag] {
        assert_refused(router, probe_arn, mutation, case).await;
    }
    assert_probe_unchanged(
        router,
        monitor_name,
        probe_id,
        probe_arn,
        &baseline_tags(),
        torn_down_at,
        case,
    )
    .await;

    // The second teardown phase: the record reports `DELETED` and is still
    // readable, and is still no target for a mutation.
    if let Some(remaining) = PAST_FIRST_TEARDOWN_STAGE.checked_sub(accepted_at.elapsed()) {
        std::thread::sleep(remaining);
    }
    let (status, drained) = get_probe(router, monitor_name, probe_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a probe whose teardown completed is still readable, got {status} {drained:?}"
    );
    assert_eq!(
        string_at(&drained, "state"),
        "DELETED",
        "case {case:?}: expected the teardown to have completed, got {drained:?}"
    );
    for mutation in [tag, untag] {
        assert_refused(router, probe_arn, mutation, case).await;
    }
    assert_probe_unchanged(
        router,
        monitor_name,
        probe_id,
        probe_arn,
        &baseline_tags(),
        torn_down_at,
        case,
    )
    .await;

    // Past the whole teardown the record is gone, so the ARN carries nothing at
    // all — the same 404 for a read as for a mutation.
    std::thread::sleep(PAST_TEARDOWN);
    let (status, gone) = get_probe(router, monitor_name, probe_id).await;
    assert_eq!(
        status, 404,
        "case {case:?}: the teardown still finishes, got {status} {gone:?}"
    );
    let (status, tags) = list_tags(router, probe_arn).await;
    assert_eq!(
        status, 404,
        "case {case:?}: a torn-down probe's ARN resolves to nothing, got {status} {tags:?}"
    );
    for mutation in [tag, untag] {
        assert_refused(router, probe_arn, mutation, case).await;
    }
}

/// The refusal belongs to the resource on its way out, not to the tagging
/// route: the monitor that held the draining probe, and a probe of that monitor
/// nobody deleted, are both still tagged and untagged, and what those calls
/// wrote is what every later read reports.
///
/// The table spans both resource kinds the model's `resourceArn` references and
/// both mutations.
#[tokio::test]
async fn a_live_resource_beside_a_draining_probe_is_still_tagged() {
    let router = router();
    let (status, created) =
        create_monitor(&router, r#"{"monitorName":"mixed","aggregationPeriod":60}"#).await;
    assert_eq!(
        status, 200,
        "setup CreateMonitor expected 200, got {status} {created:?}"
    );
    let monitor_arn = string_at(&created, "monitorArn");

    let (status, doomed) = create_probe(
        &router,
        "mixed",
        &format!(
            r#"{{"probe":{}}}"#,
            probe_json("10.0.6.1", "ICMP", r#","packetSize":56"#)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateProbe expected 200, got {status} {doomed:?}"
    );
    let doomed_id = string_at(&doomed, "probeId");
    let doomed_arn = string_at(&doomed, "probeArn");

    let (status, kept) = create_probe(
        &router,
        "mixed",
        &format!(
            r#"{{"probe":{}}}"#,
            probe_json("10.0.6.2", "ICMP", r#","packetSize":56"#)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateProbe expected 200, got {status} {kept:?}"
    );
    let kept_id = string_at(&kept, "probeId");
    let kept_arn = string_at(&kept, "probeArn");

    let (status, tagged) = tag_resource(&router, &doomed_arn, &baseline_tags()).await;
    assert_eq!(
        status, TAG_MUTATION_STATUS,
        "the probe is tagged while it is still in service, got {status} {tagged:?}"
    );
    // A probe is torn down only once it has left `PENDING`, so the delete below
    // waits for the clock to carry it into service first.
    std::thread::sleep(PAST_ACTIVATION);
    let (status, accepted) = delete_probe(&router, "mixed", &doomed_id).await;
    assert_eq!(
        status, 200,
        "expected DeleteProbe 200, got {status} {accepted:?}"
    );

    // (case, the ARN the mutation addresses, the mutation)
    let live: Vec<(&str, &str, Mutation)> = vec![
        (
            "the monitor that holds the draining probe",
            &monitor_arn,
            Mutation::Tag(json!({"keep": "me"})),
        ),
        (
            "a probe of that monitor nobody deleted",
            &kept_arn,
            Mutation::Tag(json!({"keep": "me", "drop": "later"})),
        ),
        (
            "the same probe, one key removed again",
            &kept_arn,
            Mutation::Untag(&["drop"]),
        ),
    ];
    for (case, resource_arn, mutation) in live {
        let (status, body) = mutate(&router, resource_arn, &mutation).await;
        assert_eq!(
            status, TAG_MUTATION_STATUS,
            "case {case:?}: a resource nothing is tearing down is still mutable, \
             got {status} {body:?}"
        );
    }
    let (status, monitor_tags) = list_tags(&router, &monitor_arn).await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource 200 for the monitor, got {status} {monitor_tags:?}"
    );
    assert_eq!(
        monitor_tags.get("tags"),
        Some(&json!({"keep": "me"})),
        "the monitor reports what the tag write stored, got {monitor_tags:?}"
    );
    let (status, kept_tags) = list_tags(&router, &kept_arn).await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource 200 for the live probe, got {status} {kept_tags:?}"
    );
    assert_eq!(
        kept_tags.get("tags"),
        Some(&json!({"keep": "me"})),
        "the live probe reports what the two writes left, got {kept_tags:?}"
    );

    // The draining probe beside them is refused, and still reports what the
    // accepted delete left on it.
    for mutation in [
        Mutation::Tag(json!({"keep": "me"})),
        Mutation::Untag(&["stage"]),
    ] {
        assert_refused(&router, &doomed_arn, &mutation, "the draining probe").await;
    }
    let (status, doomed_tags) = list_tags(&router, &doomed_arn).await;
    assert_eq!(
        status, 200,
        "the draining probe's ARN still resolves for a read, got {status} {doomed_tags:?}"
    );
    assert_eq!(
        doomed_tags.get("tags"),
        Some(&baseline_tags()),
        "the draining probe reports the tags the delete left, got {doomed_tags:?}"
    );
    let (status, still_there) = get_probe(&router, "mixed", &kept_id).await;
    assert_eq!(
        status, 200,
        "the probe nobody deleted is untouched, got {status} {still_there:?}"
    );
    assert_eq!(
        still_there.get("tags"),
        Some(&json!({"keep": "me"})),
        "the live probe's own read reports the same stored map, got {still_there:?}"
    );
}
