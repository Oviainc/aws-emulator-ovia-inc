// SPDX-License-Identifier: CC0-1.0

//! `UpdateScope`, and the two refusals the rest of this service's control plane
//! shares with it, driven over the service's own `restJson1` wire protocol through
//! the one harness in [`crate::harness`].
//!
//! The rules these suites pin, each asserted over a TABLE whose rows differ along
//! what the rule generalizes over:
//!
//! * RULE A (an update edits a list as a SET): `UpdateScope` applies
//!   `resourcesToDelete` and then `resourcesToAdd` to the scope's `targets` keyed
//!   by Region + target type + account, so the next `GetScope` reports the result,
//!   a deletion of what the scope does not hold is a no-op, and nothing else about
//!   the record moves. (An ADD of a pair the scope already holds is refused
//!   instead — `scope_quota.rs`.)
//! * RULE B (a target an update adds belongs to the Region the request is made in):
//!   `resourcesToAdd` naming any other Region is the modelled `ValidationException`
//!   (400), and the scope is left as it was.
//! * RULE C (a scope keeps its owner and at least one target): a `resourcesToDelete`
//!   that would drop the owner account's target, or every target, is the modelled
//!   `ValidationException` (400).
//! * RULE D (an add list and its remove list cannot name one item): any update of
//!   this service — `UpdateScope`'s two target lists and `UpdateMonitor`'s four
//!   resource lists — refuses that contradiction with the modelled
//!   `ValidationException` (400), leaving the record untouched.
//! * RULE E (two refusals whose text is a constant for the condition): a second
//!   create of one monitor name, and a `nextToken` no listing of this service
//!   issued.

use serde_json::{json, Value};

use crate::harness::{extra_region, targets, Answer, Emulator, ACCOUNT_ID, REGION, SIGNING_NAME};

/// Accounts that are NOT the emulator's own, so a row that removes a target is
/// about the target and not about the scope's owner.
const OTHER_ACCOUNTS: [&str; 4] = [
    "111111111111",
    "222222222222",
    "333333333333",
    "444444444444",
];

/// The local resource types `MonitorLocalResourceType` declares, from the model
/// (`smithy_query(query="shape",shape="MonitorLocalResourceType")`).
const LOCAL_RESOURCE_TYPES: &[&str] = &[
    "AWS::EC2::VPC",
    "AWS::AvailabilityZone",
    "AWS::EC2::Subnet",
    "AWS::EKS::Cluster",
    "AWS::Region",
];

/// The remote resource types `MonitorRemoteResourceType` declares, from the model
/// (`smithy_query(query="shape",shape="MonitorRemoteResourceType")`).
const REMOTE_RESOURCE_TYPES: &[&str] = &[
    "AWS::EC2::VPC",
    "AWS::AvailabilityZone",
    "AWS::EC2::Subnet",
    "AWS::AWSService",
    "AWS::Region",
];

/// A monitor resource list as the wire carries it: a `type` + `identifier` pair
/// per entry.
fn resources(entries: &[(&str, &str)]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|(resource_type, identifier)| {
                json!({ "type": resource_type, "identifier": identifier })
            })
            .collect(),
    )
}

/// `text` hex-encoded through the same crate `state::page_token` encodes a cursor
/// with — so a test can hand a listing a token that decodes cleanly and still was
/// never issued.
fn hex_of(text: &str) -> String {
    hex::encode(text)
}

/// A scope holding exactly `pairs` as its targets, and the `scopeId` it was minted
/// with.
async fn scope_with(emulator: &Emulator, pairs: &[(&str, &str)]) -> String {
    let created = emulator
        .create_scope(json!({ "targets": targets(pairs) }))
        .await;
    assert_eq!(
        created.status, 200,
        "the scope for {pairs:?} must be created, got {} with body {}",
        created.status, created.body
    );
    created.text("scopeId")
}

/// A monitor named `name` holding `local` and `remote`, under a fresh scope.
async fn monitor_with(emulator: &Emulator, name: &str, local: Value, remote: Value) -> Answer {
    let scope_arn = emulator.scope_for_monitors().await;
    let created = emulator
        .create_monitor(json!({
            "monitorName": name,
            "scopeArn": scope_arn,
            "localResources": local,
            "remoteResources": remote
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "the monitor {name:?} must be created, got {} with body {}",
        created.status, created.body
    );
    created
}

/// A scope's `targets` as order-independent keys, so a row asserts WHICH targets a
/// scope holds without pinning the order the store keeps them in.
fn target_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|target| {
            format!(
                "{}@{}",
                target
                    .pointer("/targetIdentifier/targetId/accountId")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                target
                    .get("region")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            )
        })
        .collect();
    keys.sort();
    keys
}

/// The same keys for a row's (account, Region) pairs.
fn expected_keys(pairs: &[(&str, &str)]) -> Vec<String> {
    let mut keys: Vec<String> = pairs
        .iter()
        .map(|(account, region)| format!("{account}@{region}"))
        .collect();
    keys.sort();
    keys
}

/// A monitor resource list as order-independent `type/identifier` keys.
fn resource_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|resource| {
            format!(
                "{}/{}",
                resource
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                resource
                    .get("identifier")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            )
        })
        .collect();
    keys.sort();
    keys
}

/// Assert that `answer` is the modelled 400 `ValidationException`.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        answer.is_error("ValidationException"),
        "case {case:?}: expected ValidationException, got {:?} / body {}",
        answer.error_type,
        answer.body
    );
}

/// One row of the `UpdateScope` target-editing table (RULE A).
struct ScopeUpdate {
    case: &'static str,
    initial: Vec<(&'static str, &'static str)>,
    body: Value,
    expected: Vec<(&'static str, &'static str)>,
}

fn scope_update_rows() -> Vec<ScopeUpdate> {
    let [first, second, third, fourth] = OTHER_ACCOUNTS;
    vec![
        ScopeUpdate {
            case: "add one target",
            initial: vec![(first, REGION)],
            body: json!({ "resourcesToAdd": targets(&[(second, REGION)]) }),
            expected: vec![(first, REGION), (second, REGION)],
        },
        ScopeUpdate {
            case: "add three targets in one call: the count dimension",
            initial: vec![(first, REGION)],
            body: json!({
                "resourcesToAdd": targets(&[(second, REGION), (third, REGION), (fourth, REGION)])
            }),
            expected: vec![
                (first, REGION),
                (second, REGION),
                (third, REGION),
                (fourth, REGION),
            ],
        },
        ScopeUpdate {
            case: "remove one of two targets",
            initial: vec![(first, REGION), (second, REGION)],
            body: json!({ "resourcesToDelete": targets(&[(second, REGION)]) }),
            expected: vec![(first, REGION)],
        },
        ScopeUpdate {
            case: "remove a target the scope does not hold: nothing to remove",
            initial: vec![(first, REGION)],
            body: json!({ "resourcesToDelete": targets(&[(third, REGION)]) }),
            expected: vec![(first, REGION)],
        },
        ScopeUpdate {
            case: "remove a target the scope holds in another Region",
            initial: vec![(first, REGION), (second, "eu-west-1")],
            body: json!({ "resourcesToDelete": targets(&[(second, "eu-west-1")]) }),
            expected: vec![(first, REGION)],
        },
        ScopeUpdate {
            case: "add and remove different targets in one call",
            initial: vec![(first, REGION), (second, REGION)],
            body: json!({
                "resourcesToAdd": targets(&[(third, REGION)]),
                "resourcesToDelete": targets(&[(first, REGION)])
            }),
            expected: vec![(second, REGION), (third, REGION)],
        },
        ScopeUpdate {
            case: "neither list: an update that asks for nothing changes nothing",
            initial: vec![(first, REGION), (second, REGION)],
            body: json!({}),
            expected: vec![(first, REGION), (second, REGION)],
        },
    ]
}

#[tokio::test]
async fn update_scope_edits_the_targets_and_get_scope_reflects_it() {
    for row in scope_update_rows() {
        // A store per row: the account holds one scope per Region, so each row's
        // scope is the only one its store needs.
        let emulator = Emulator::new();
        let scope_id = scope_with(&emulator, &row.initial).await;
        let before = emulator.get_scope(&scope_id).await;

        let updated = emulator.update_scope(&scope_id, row.body.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected 200, got {} with body {}",
            row.case, updated.status, updated.body
        );
        assert_eq!(
            updated.text("scopeId"),
            scope_id,
            "case {:?}: expected the scopeId that was addressed, got {:?}",
            row.case,
            updated.text("scopeId")
        );
        assert_eq!(
            updated.text("scopeArn"),
            before.text("scopeArn"),
            "case {:?}: expected the stored scopeArn, got {:?}",
            row.case,
            updated.text("scopeArn")
        );
        // Editing which resources the topology covers does not re-provision the
        // scope, so the status is the one the record already held.
        assert_eq!(
            updated.text("status"),
            before.text("status"),
            "case {:?}: expected the stored status {:?}, got {:?}",
            row.case,
            before.text("status"),
            updated.text("status")
        );

        let read = emulator.get_scope(&scope_id).await;
        assert_eq!(
            target_keys(read.member("targets")),
            expected_keys(&row.expected),
            "case {:?}: expected the scope to hold {:?}, got {:?}",
            row.case,
            expected_keys(&row.expected),
            target_keys(read.member("targets"))
        );

        // Replaying the edit: a deletion the scope no longer holds is a no-op, so
        // an update that only deletes (or asks for nothing) lands on the same
        // targets again — while re-adding a target the first call put IN scope is
        // refused, because the scope already holds it. Either way the targets are
        // what the first call left.
        let replay = emulator.update_scope(&scope_id, row.body.clone()).await;
        let adds_again = row
            .body
            .get("resourcesToAdd")
            .and_then(Value::as_array)
            .is_some_and(|list| !list.is_empty());
        if adds_again {
            assert_refused(&format!("{}: the replayed add", row.case), &replay);
        } else {
            assert_eq!(
                replay.status, 200,
                "case {:?}: the replay must succeed, got {} with body {}",
                row.case, replay.status, replay.body
            );
        }
        let reread = emulator.get_scope(&scope_id).await;
        assert_eq!(
            target_keys(reread.member("targets")),
            expected_keys(&row.expected),
            "case {:?}: a replayed update must not change the targets again, got {:?}",
            row.case,
            target_keys(reread.member("targets"))
        );
    }
}

#[tokio::test]
async fn update_scope_refuses_a_target_from_another_region() {
    // RULE B is about the Region the REQUEST is made in, not about a constant: the
    // same target is accepted under one credential scope and refused under
    // another.
    for signed in ["us-west-2", "eu-central-1", "ap-northeast-1"] {
        let emulator = Emulator::new_in(signed);
        let held = OTHER_ACCOUNTS[0];
        let scope_id = scope_with(&emulator, &[(held, signed)]).await;
        let before = emulator.get_scope(&scope_id).await;

        let foreign: Vec<&str> = ["us-west-2", "eu-central-1", "us-east-1", "sa-east-1"]
            .into_iter()
            .filter(|region| *region != signed)
            .collect();
        let mut rows: Vec<(String, Value)> = foreign
            .iter()
            .map(|region| {
                (
                    format!("a single target in {region}"),
                    json!({ "resourcesToAdd": targets(&[(OTHER_ACCOUNTS[1], region)]) }),
                )
            })
            .collect();
        // Position in the list must not matter: one bad entry refuses the request.
        rows.push((
            "a valid target first, a foreign one second".to_string(),
            json!({
                "resourcesToAdd": targets(&[
                    (OTHER_ACCOUNTS[1], signed),
                    (OTHER_ACCOUNTS[2], foreign[0]),
                ])
            }),
        ));
        rows.push((
            "a foreign target first, a valid one second".to_string(),
            json!({
                "resourcesToAdd": targets(&[
                    (OTHER_ACCOUNTS[2], foreign[0]),
                    (OTHER_ACCOUNTS[1], signed),
                ])
            }),
        ));

        for (case, body) in rows {
            let case = format!("signed {signed}: {case}");
            let answer = emulator.update_scope(&scope_id, body).await;
            assert_refused(&case, &answer);
            // A refused update leaves the scope exactly as it was.
            let read = emulator.get_scope(&scope_id).await;
            assert_eq!(
                target_keys(read.member("targets")),
                expected_keys(&[(held, signed)]),
                "case {case:?}: a refused update must not edit the targets, got {:?}",
                target_keys(read.member("targets"))
            );
            assert_eq!(
                read.text("status"),
                before.text("status"),
                "case {case:?}: a refused update must not move the status, got {:?}",
                read.text("status")
            );
        }

        // The same request with a target in the signed Region is accepted, so the
        // refusals above are about the Region and nothing else.
        let accepted = emulator
            .update_scope(
                &scope_id,
                json!({ "resourcesToAdd": targets(&[(OTHER_ACCOUNTS[1], signed)]) }),
            )
            .await;
        assert_eq!(
            accepted.status, 200,
            "signed {signed:?}: a target in the request's Region must be accepted, got {} with body {}",
            accepted.status, accepted.body
        );
    }
}

/// One row of the `UpdateScope` removal-refusal table (RULE C).
struct RemovalRefusal {
    case: &'static str,
    initial: Vec<(&'static str, &'static str)>,
    body: Value,
}

#[tokio::test]
async fn update_scope_keeps_the_owner_and_at_least_one_target() {
    let [first, second, third, _] = OTHER_ACCOUNTS;

    let rows = vec![
        RemovalRefusal {
            case: "removing the owner account's own target",
            initial: vec![(ACCOUNT_ID, REGION), (first, REGION)],
            body: json!({ "resourcesToDelete": targets(&[(ACCOUNT_ID, REGION)]) }),
        },
        RemovalRefusal {
            case: "removing the owner's target among several",
            initial: vec![(ACCOUNT_ID, REGION), (first, REGION), (second, REGION)],
            body: json!({
                "resourcesToDelete": targets(&[(first, REGION), (ACCOUNT_ID, REGION)])
            }),
        },
        RemovalRefusal {
            case: "removing the scope's only target",
            initial: vec![(first, REGION)],
            body: json!({ "resourcesToDelete": targets(&[(first, REGION)]) }),
        },
        RemovalRefusal {
            case: "removing every target of a three-target scope",
            initial: vec![(first, REGION), (second, REGION), (third, REGION)],
            body: json!({
                "resourcesToDelete": targets(&[
                    (first, REGION),
                    (second, REGION),
                    (third, REGION),
                ])
            }),
        },
    ];

    for row in rows {
        let RemovalRefusal {
            case,
            initial,
            body,
        } = row;
        // A store per row: one scope per account and Region.
        let emulator = Emulator::new();
        let scope_id = scope_with(&emulator, &initial).await;
        let answer = emulator.update_scope(&scope_id, body).await;
        assert_refused(case, &answer);
        let read = emulator.get_scope(&scope_id).await;
        assert_eq!(
            target_keys(read.member("targets")),
            expected_keys(&initial),
            "case {case:?}: a refused update must leave the targets alone, got {:?}",
            target_keys(read.member("targets"))
        );
    }

    // Removing all but one target IS accepted, so the refusals above are about
    // what the scope would be left with and not about removing at all.
    let emulator = Emulator::new();
    let scope_id = scope_with(&emulator, &[(first, REGION), (second, REGION)]).await;
    let accepted = emulator
        .update_scope(
            &scope_id,
            json!({ "resourcesToDelete": targets(&[(second, REGION)]) }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "a scope left with one target must accept the removal, got {} with body {}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn update_scope_refuses_a_target_in_both_lists() {
    // RULE D over `UpdateScope`'s two target lists: the contradiction is refused
    // whichever entry carries it and wherever in the lists it sits.
    let [first, second, third, _] = OTHER_ACCOUNTS;
    let initial = [(first, REGION), (second, REGION)];

    let rows: Vec<(&str, Value)> = vec![
        (
            "a target the scope holds, in both lists",
            json!({
                "resourcesToAdd": targets(&[(first, REGION)]),
                "resourcesToDelete": targets(&[(first, REGION)])
            }),
        ),
        (
            "a target the scope does not hold, in both lists",
            json!({
                "resourcesToAdd": targets(&[(third, REGION)]),
                "resourcesToDelete": targets(&[(third, REGION)])
            }),
        ),
        (
            "the contradicting entry last in a longer add list",
            json!({
                "resourcesToAdd": targets(&[(third, REGION), (second, REGION)]),
                "resourcesToDelete": targets(&[(second, REGION)])
            }),
        ),
        (
            "the contradicting entry last in a longer delete list",
            json!({
                "resourcesToAdd": targets(&[(third, REGION)]),
                "resourcesToDelete": targets(&[(first, REGION), (third, REGION)])
            }),
        ),
    ];

    for (case, body) in rows {
        // A store per row: one scope per account and Region.
        let emulator = Emulator::new();
        let scope_id = scope_with(&emulator, &initial).await;
        let answer = emulator.update_scope(&scope_id, body).await;
        assert_refused(case, &answer);
        let read = emulator.get_scope(&scope_id).await;
        assert_eq!(
            target_keys(read.member("targets")),
            expected_keys(&initial),
            "case {case:?}: a refused update must leave the targets alone, got {:?}",
            target_keys(read.member("targets"))
        );
    }

    // The same edit with the two lists disjoint is accepted.
    let emulator = Emulator::new();
    let scope_id = scope_with(&emulator, &initial).await;
    let accepted = emulator
        .update_scope(
            &scope_id,
            json!({
                "resourcesToAdd": targets(&[(third, REGION)]),
                "resourcesToDelete": targets(&[(first, REGION)])
            }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "disjoint add/delete lists must be accepted, got {} with body {}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn update_monitor_refuses_a_resource_in_both_add_and_remove() {
    // RULE D over `UpdateMonitor`'s four resource lists.
    let emulator = Emulator::new();
    let local = resources(&[("AWS::EC2::VPC", "vpc-1"), ("AWS::EC2::Subnet", "subnet-1")]);
    let remote = resources(&[("AWS::AWSService", "S3")]);

    let mut rows: Vec<(String, Value)> = Vec::new();
    // Every local resource type the model publishes: the rule is about the pair of
    // lists, not about a type.
    for (index, resource_type) in LOCAL_RESOURCE_TYPES.iter().enumerate() {
        let entry = resources(&[(resource_type, &format!("both-{index}"))]);
        rows.push((
            format!("a {resource_type} local resource in both local lists"),
            json!({
                "localResourcesToAdd": entry.clone(),
                "localResourcesToRemove": entry
            }),
        ));
    }
    // Every remote resource type, likewise.
    for (index, resource_type) in REMOTE_RESOURCE_TYPES.iter().enumerate() {
        let entry = resources(&[(resource_type, &format!("both-{index}"))]);
        rows.push((
            format!("a {resource_type} remote resource in both remote lists"),
            json!({
                "remoteResourcesToAdd": entry.clone(),
                "remoteResourcesToRemove": entry
            }),
        ));
    }
    rows.push((
        "the contradicting entry last in a longer add list".to_string(),
        json!({
            "localResourcesToAdd": resources(&[
                ("AWS::EC2::VPC", "vpc-new"),
                ("AWS::Region", "us-west-2"),
            ]),
            "localResourcesToRemove": resources(&[("AWS::Region", "us-west-2")])
        }),
    ));
    rows.push((
        "a resource the monitor already holds, in both lists".to_string(),
        json!({
            "localResourcesToAdd": resources(&[("AWS::EC2::VPC", "vpc-1")]),
            "localResourcesToRemove": resources(&[("AWS::EC2::VPC", "vpc-1")])
        }),
    ));
    rows.push((
        "an overlap in the remote lists while the local lists are fine".to_string(),
        json!({
            "localResourcesToAdd": resources(&[("AWS::EC2::VPC", "vpc-new")]),
            "remoteResourcesToAdd": resources(&[("AWS::AWSService", "S3")]),
            "remoteResourcesToRemove": resources(&[("AWS::AWSService", "S3")])
        }),
    ));
    rows.push((
        "one entry of each pair contradicted at once".to_string(),
        json!({
            "localResourcesToAdd": resources(&[("AWS::EC2::Subnet", "subnet-1")]),
            "localResourcesToRemove": resources(&[("AWS::EC2::Subnet", "subnet-1")]),
            "remoteResourcesToAdd": resources(&[("AWS::Region", "us-east-1")]),
            "remoteResourcesToRemove": resources(&[("AWS::Region", "us-east-1")])
        }),
    ));

    for (index, (case, body)) in rows.into_iter().enumerate() {
        let name = format!("contradicted-{index}");
        monitor_with(&emulator, &name, local.clone(), remote.clone()).await;
        let before = emulator.get_monitor(&name).await;

        let answer = emulator.update_monitor(&name, body).await;
        assert_refused(&case, &answer);

        // A refused update leaves the monitor exactly as it was.
        let after = emulator.get_monitor(&name).await;
        for member in ["localResources", "remoteResources", "modifiedAt"] {
            assert_eq!(
                after.member(member),
                before.member(member),
                "case {case:?}: a refused update must not change {member}, got {:?} after {:?}",
                after.member(member),
                before.member(member)
            );
        }
    }

    // The same edit with the overlap removed is accepted, so the refusals above
    // are about the contradiction and nothing else.
    let name = "contradiction-free";
    monitor_with(&emulator, name, local, remote).await;
    let accepted = emulator
        .update_monitor(
            name,
            json!({
                "localResourcesToAdd": resources(&[("AWS::Region", "us-west-2")]),
                "localResourcesToRemove": resources(&[("AWS::EC2::Subnet", "subnet-1")])
            }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "disjoint add/remove lists must be accepted, got {} with body {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        resource_keys(accepted.member("localResources")),
        vec![
            "AWS::EC2::VPC/vpc-1".to_string(),
            "AWS::Region/us-west-2".to_string()
        ],
        "the accepted edit must land, got {:?}",
        resource_keys(accepted.member("localResources"))
    );
}

#[tokio::test]
async fn update_scope_refuses_more_targets_than_one_scope_holds() {
    // The store holds a bounded number of targets per scope
    // (`state::MAX_SCOPE_TARGETS`), so repeated updates naming fresh accounts
    // cannot grow one record without limit. The ceiling itself is accepted; one
    // target past it is the modelled 400 and leaves the scope as it was.
    let emulator = Emulator::new();
    let ceiling = service_com_amazonaws_networkflowmonitor_networkflowmonitor::state::MAX_SCOPE_TARGETS;
    let accounts: Vec<String> = (0..=ceiling)
        .map(|index| format!("{:012}", 100_000_000_000u64 + index as u64))
        .collect();
    let pairs: Vec<(&str, &str)> = accounts
        .iter()
        .map(|account| (account.as_str(), REGION))
        .collect();

    let scope_id = scope_with(&emulator, &pairs[..1]).await;
    // Filling the scope exactly to the ceiling is accepted.
    let to_the_ceiling = emulator
        .update_scope(
            &scope_id,
            json!({ "resourcesToAdd": targets(&pairs[1..ceiling]) }),
        )
        .await;
    assert_eq!(
        to_the_ceiling.status, 200,
        "a scope filled exactly to the ceiling must be accepted, got {} with body {}",
        to_the_ceiling.status, to_the_ceiling.body
    );
    let read = emulator.get_scope(&scope_id).await;
    assert_eq!(
        target_keys(read.member("targets")).len(),
        ceiling,
        "the scope must hold the ceiling's worth of targets, got {:?}",
        target_keys(read.member("targets")).len()
    );

    // One more target is refused, and the scope keeps exactly what it held.
    let past_the_ceiling = emulator
        .update_scope(
            &scope_id,
            json!({ "resourcesToAdd": targets(&pairs[ceiling..]) }),
        )
        .await;
    assert_refused("one target past the ceiling", &past_the_ceiling);
    let after = emulator.get_scope(&scope_id).await;
    assert_eq!(
        target_keys(after.member("targets")),
        target_keys(read.member("targets")),
        "a refused update must leave the targets alone, got {:?}",
        target_keys(after.member("targets"))
    );
}

#[tokio::test]
async fn a_tagged_resource_the_store_does_not_hold_is_named_in_the_refusal() {
    // The service's GENERIC not-found renderer (`state::not_found`) serves what it
    // mints no ARN of its own for: here the tagging operations, which address a
    // resource BY its ARN. Its text follows the same form as the monitor and scope
    // renderers — the resource label, the identifier the request named in square
    // brackets, then `doesn't exist` — so one client-side matcher reads every
    // not-found this service answers.
    let emulator = Emulator::new();
    let unknown = [
        (
            "a scope id nothing was minted for",
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:scope/00000000-0000-4000-8000-ffffffffffff"
            ),
        ),
        (
            "a monitor name nothing was minted for",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:monitor/never-created"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT_ID}:vpc/vpc-123"),
        ),
    ];

    for (case, arn) in unknown {
        let expected = format!("Resource [{arn}] doesn't exist");
        for (operation, answer) in [
            ("ListTagsForResource", emulator.list_tags(&arn).await),
            (
                "TagResource",
                emulator.tag_resource(&arn, &json!({ "env": "prod" })).await,
            ),
            (
                "UntagResource",
                emulator.untag_resource(&arn, &["env"]).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}/{operation}: expected 404, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.text("message"),
                expected,
                "case {case:?}/{operation}: expected {expected:?}, got {:?}",
                answer.text("message")
            );
        }
    }
}

#[tokio::test]
async fn a_second_create_of_one_monitor_name_carries_the_constant_conflict_text() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    // RULE E: the text is a constant for the condition — it carries no value from
    // the request, so it is the same for every name and every resource list.
    let expected = "Cannot create a monitor which already exists";

    for (case, name, second_local) in [
        (
            "the same request twice",
            "duplicated",
            resources(&[("AWS::EC2::VPC", "vpc-1")]),
        ),
        (
            "a one-character name",
            "d",
            resources(&[("AWS::Region", "us-east-1")]),
        ),
        (
            "a name with every character class ResourceName allows",
            "Dup.monitor_1-x",
            resources(&[
                ("AWS::EC2::Subnet", "subnet-9"),
                ("AWS::EKS::Cluster", "cluster-9"),
            ]),
        ),
    ] {
        let first = emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": resources(&[("AWS::EC2::VPC", "vpc-first")])
            }))
            .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create must succeed, got {} with body {}",
            first.status, first.body
        );

        let second = emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": second_local
            }))
            .await;
        assert_eq!(
            second.status, 409,
            "case {case:?}: expected 409, got {} with body {}",
            second.status, second.body
        );
        assert!(
            second.is_error("ConflictException"),
            "case {case:?}: expected ConflictException, got {:?} / body {}",
            second.error_type,
            second.body
        );
        assert_eq!(
            second.text("message"),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            second.text("message")
        );
        // The refused create did not overwrite the monitor that exists.
        let read = emulator.get_monitor(name).await;
        assert_eq!(
            resource_keys(read.member("localResources")),
            vec!["AWS::EC2::VPC/vpc-first".to_string()],
            "case {case:?}: the stored monitor must keep its own localResources, got {:?}",
            resource_keys(read.member("localResources"))
        );
    }
}

#[tokio::test]
async fn an_unissued_next_token_carries_the_constant_invalid_token_text() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    for index in 1..=3 {
        let created = emulator
            .create_monitor(json!({
                "monitorName": format!("paged-{index}"),
                "scopeArn": scope_arn,
                "localResources": resources(&[("AWS::EC2::VPC", "vpc-1")])
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "the paged monitors must be created, got {} with body {}",
            created.status, created.body
        );
        let scope = emulator
            .create_scope_in(
                extra_region(index),
                json!({ "targets": targets(&[(OTHER_ACCOUNTS[0], extra_region(index))]) }),
            )
            .await;
        assert_eq!(
            scope.status, 200,
            "the paged scopes must be created, got {} with body {}",
            scope.status, scope.body
        );
    }

    // RULE E again: the text renders nothing of the rejected token, so every
    // unissued shape carries it — on every paginated listing of this service.
    let expected = "NextToken provided isn't valid.";
    for listing in ["monitors", "scopes"] {
        // Each listing's cursors carry its OWN tag (`state::MONITORS_PAGE_TAG` /
        // `SCOPES_PAGE_TAG`), so the other listing's tag is a token this one never
        // issued, and its own tag with no cursor after it names no position.
        let (own_tag, foreign_tag) = if listing == "monitors" {
            ("monitors/1:", "scopes/1:")
        } else {
            ("scopes/1:", "monitors/1:")
        };
        let tokens = [
            ("a plain word", "not-a-token".to_string()),
            ("a resource name", "paged-1".to_string()),
            ("the hex of a bare resource name", hex_of("paged-1")),
            (
                "the hex of this listing's tag with no cursor after it",
                hex_of(own_tag),
            ),
            (
                "the hex of the other listing's tag and cursor",
                hex_of(&format!("{foreign_tag}paged-1")),
            ),
            ("the hex of an unrelated sentence", hex_of("start at the top")),
        ];

        for (case, token) in tokens {
            let case = format!("{listing}: {case}");
            let answer = if listing == "monitors" {
                emulator
                    .list_monitors(&format!("?nextToken={token}"))
                    .await
            } else {
                emulator.list_scopes(&format!("?nextToken={token}")).await
            };
            assert_refused(&case, &answer);
            assert_eq!(
                answer.text("message"),
                expected,
                "case {case:?} ({token:?}): expected {expected:?}, got {:?}",
                answer.text("message")
            );
        }
    }

    // A token a listing DID issue still resumes its walk, so the refusals above
    // are about the token and not about paginating at all.
    for listing in ["monitors", "scopes"] {
        let first = if listing == "monitors" {
            emulator.list_monitors("?maxResults=1").await
        } else {
            emulator.list_scopes("?maxResults=1").await
        };
        let issued = first.text("nextToken");
        assert!(
            !issued.is_empty(),
            "{listing}: the first page must carry a nextToken, got body {}",
            first.body
        );
        let resumed = if listing == "monitors" {
            emulator
                .list_monitors(&format!("?maxResults=1&nextToken={issued}"))
                .await
        } else {
            emulator
                .list_scopes(&format!("?maxResults=1&nextToken={issued}"))
                .await
        };
        assert_eq!(
            resumed.status, 200,
            "{listing}: an issued token must be accepted, got {} with body {}",
            resumed.status, resumed.body
        );
    }
}
