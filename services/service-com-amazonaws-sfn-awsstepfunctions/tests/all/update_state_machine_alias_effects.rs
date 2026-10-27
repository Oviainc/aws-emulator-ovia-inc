// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two effects an `UpdateStateMachineAlias` has that no
//! caller can see from the alias record alone, driven over this service's own wire
//! protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rules under test:
//!
//! 1. an update MODIFIES the one record its create stored, so concurrent updates of
//!    one alias are serialized rather than refused: every one of them answers, the
//!    record converges on the parameters of one of them, nothing is minted beside it,
//!    and callers naming the same parameters all get the same `updateDate` back —
//!    this operation's modelled `ConflictException` is for concurrent requests for
//!    `DeleteStateMachineVersion`, `PublishStateMachineVersion` or
//!    `UpdateStateMachine` with `publish` set, which an alias update is none of
//!    (`smithy_query(query="operation",operation="UpdateStateMachineAlias")`);
//! 2. an alias is the reference `StartExecution` resolves at the instant it is
//!    called, so an update of its routing configuration shifts which versions later
//!    executions run: a version the update dropped runs none of them, and every
//!    version it names at a non-zero weight runs some.
//!
//! The harness (`Emulator`, `string`, `start_execution`, `describe_execution`) is the
//! one `state_machine_lifecycle` builds and the published versions come from
//! `state_machine_versions`' `machine_with_versions`; `tests/all.rs` is this binary's
//! crate root, so both are reached through `crate::`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{
    describe_execution, start_execution, string, Emulator,
};
use crate::state_machine_versions::machine_with_versions;

/// One entry of a routing configuration.
fn route(version_arn: &str, weight: i64) -> Value {
    json!({ "stateMachineVersionArn": version_arn, "weight": weight })
}

/// The parameters one update names: its description and its routing configuration as
/// `(version ARN, weight)` pairs, which is the form the record reports them in.
type Parameters = (String, Vec<(String, i64)>);

/// `DescribeStateMachineAlias` on `arn`, which must succeed.
async fn describe_alias(emulator: &Emulator, arn: &str) -> Value {
    emulator
        .ok(
            "DescribeStateMachineAlias",
            &json!({ "stateMachineAliasArn": arn }),
        )
        .await
}

/// The numeric member `member` of `body`, as the wire carries it.
fn number(body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a numeric '{member}' in {body:?}"))
}

/// The parameters a record reports: its description and its routing configuration, in
/// the order the response carries them.
fn reported(body: &Value) -> Parameters {
    let routing = body
        .get("routingConfiguration")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a routingConfiguration list in {body:?}"))
        .iter()
        .map(|item| {
            (
                string(item, "stateMachineVersionArn"),
                item.get("weight")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| panic!("expected a weight in {item:?}")),
            )
        })
        .collect();
    (string(body, "description"), routing)
}

/// The `UpdateStateMachineAlias` request body for `arn` and `parameters`.
fn update_request(arn: &str, (description, routing): &Parameters) -> Value {
    let entries: Vec<Value> = routing
        .iter()
        .map(|(version_arn, weight)| route(version_arn, *weight))
        .collect();
    json!({
        "stateMachineAliasArn": arn,
        "description": description,
        "routingConfiguration": entries,
    })
}

/// Create an alias over `routing` with the description `description`, and return its
/// ARN.
async fn create_alias(
    emulator: &Emulator,
    name: &str,
    description: &str,
    routing: &[Value],
) -> String {
    let created = emulator
        .ok(
            "CreateStateMachineAlias",
            &json!({
                "name": name,
                "description": description,
                "routingConfiguration": routing,
            }),
        )
        .await;
    string(&created, "stateMachineAliasArn")
}

/// Every alias ARN `ListStateMachineAliases` reports for the state machine `arn`.
async fn listed_aliases(emulator: &Emulator, arn: &str) -> Vec<String> {
    let listed = emulator
        .ok("ListStateMachineAliases", &json!({ "stateMachineArn": arn }))
        .await;
    listed
        .get("stateMachineAliases")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an alias list in {listed:?}"))
        .iter()
        .map(|item| string(item, "stateMachineAliasArn"))
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: concurrent updates of one alias are serialized, not refused
// ---------------------------------------------------------------------------

/// The parameter set of the caller at `index` in a race: a description that no other caller names
/// and a routing configuration whose SHAPE alternates between one entry and two, so
/// the racing requests differ along both members the operation writes.
fn racing_parameters(index: usize, versions: &[String]) -> Parameters {
    let description = format!("caller-{index}");
    let primary = &versions[index % versions.len()];
    let routing = if index.is_multiple_of(2) {
        vec![(primary.clone(), 100)]
    } else {
        let secondary = &versions[(index + 1) % versions.len()];
        let weight = i64::try_from(index % 10).unwrap_or(0) * 10;
        vec![(primary.clone(), 100 - weight), (secondary.clone(), weight)]
    };
    (description, routing)
}

/// Concurrent updates of ONE alias all answer, and the record converges on the
/// parameters of one of them — over a table whose rows differ along the NUMBER of
/// racing callers and along whether they name the SAME parameters or different ones,
/// which is what the rule generalises over.
///
/// The callers run on their own tasks over a multi-threaded runtime, so the requests
/// are in flight together rather than one after another.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_updates_of_one_alias_all_answer_and_converge() {
    let emulator = Arc::new(Emulator::new());
    let machine = machine_with_versions(&emulator, "alias-update-race", 3).await;
    let versions: Vec<String> = (1..=3).map(|number| format!("{machine}:{number}")).collect();

    // (case, how many callers race, whether they all name the same parameters).
    let cases: Vec<(&str, usize, bool)> = vec![
        ("two callers naming the same parameters", 2, true),
        ("two callers naming different parameters", 2, false),
        ("four callers naming different parameters", 4, false),
        ("eight callers naming the same parameters", 8, true),
        ("eight callers naming different parameters", 8, false),
    ];

    for (index, (case, callers, identical)) in cases.iter().enumerate() {
        let name = format!("racy-{index}");
        let arn = create_alias(
            &emulator,
            &name,
            "baseline",
            &[route(&versions[0], 100)],
        )
        .await;
        let created = describe_alias(&emulator, &arn).await;

        let candidates: Vec<Parameters> = (0..*callers)
            .map(|caller| racing_parameters(if *identical { 0 } else { caller }, &versions))
            .collect();

        let mut racing = Vec::with_capacity(*callers);
        for candidate in &candidates {
            let emulator = Arc::clone(&emulator);
            let request = update_request(&arn, candidate);
            racing.push(tokio::spawn(async move {
                emulator.call("UpdateStateMachineAlias", &request).await
            }));
        }

        let mut update_dates: Vec<f64> = Vec::with_capacity(*callers);
        for (caller, task) in racing.into_iter().enumerate() {
            let (status, body) = task.await.expect("the racing caller ran to completion");
            assert_eq!(
                status, 200,
                "case {case:?}: caller {caller} must be served rather than refused, \
                 got {status} with body {body:?}"
            );
            update_dates.push(number(&body, "updateDate"));
        }

        // The record holds the parameters of ONE of the callers, whole: no update
        // left a description from one request beside a routing configuration from
        // another.
        let described = describe_alias(&emulator, &arn).await;
        let settled = reported(&described);
        assert!(
            candidates.contains(&settled),
            "case {case:?}: expected one caller's parameters out of {candidates:?}, \
             got {settled:?}"
        );
        // The instant the record reports is one a caller was answered with.
        let reported_date = number(&described, "updateDate");
        assert!(
            update_dates.contains(&reported_date),
            "case {case:?}: expected an updateDate out of {update_dates:?}, \
             got {reported_date:?}"
        );
        assert!(
            reported_date >= number(&created, "creationDate"),
            "case {case:?}: an update cannot report an instant before the create, \
             got {reported_date:?}"
        );
        // An update modifies the record its create stored and mints nothing: the ARN,
        // the name and the creation date stand, and the state machine holds exactly
        // the aliases that were created on it.
        assert_eq!(
            string(&described, "name"),
            name,
            "case {case:?}: the alias keeps its name, got {described:?}"
        );
        assert_eq!(
            number(&described, "creationDate"),
            number(&created, "creationDate"),
            "case {case:?}: the alias keeps its creation date, got {described:?}"
        );
        let listed = listed_aliases(&emulator, &machine).await;
        assert_eq!(
            listed.iter().filter(|listed| **listed == arn).count(),
            1,
            "case {case:?}: the race mints no second record, got {listed:?}"
        );
        // Converged: a second read of an unchanged record answers the first.
        let again = describe_alias(&emulator, &arn).await;
        assert_eq!(
            again, described,
            "case {case:?}: two reads of a settled record must agree, got {again:?}"
        );

        if *identical {
            // "Requests with the same parameters return an idempotent response": only
            // the first of them wrote, so every caller was answered the same instant.
            let distinct: BTreeSet<String> =
                update_dates.iter().map(|date| date.to_string()).collect();
            assert_eq!(
                distinct.len(),
                1,
                "case {case:?}: callers naming the same parameters must share one \
                 updateDate, got {update_dates:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an update shifts the versions a later execution runs
// ---------------------------------------------------------------------------

/// One row of the shift table: the case, the `(version index, weight)` entries the
/// alias is created with, and the entries it is then updated to.
type ShiftCase<'a> = (&'a str, Vec<(usize, i64)>, Vec<(usize, i64)>);

/// How many executions each half of a row starts. Enough that a version at a non-zero
/// weight running none of them is beyond chance.
const STARTS: usize = 40;

/// Start `STARTS` executions through `alias` and count, per version ARN, how many of
/// them that version ran.
async fn routed_versions(
    emulator: &Emulator,
    alias: &str,
    tag: &str,
) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for index in 0..STARTS {
        let execution = start_execution(emulator, alias, &format!("{tag}-{index}"), "{}").await;
        let described = describe_execution(emulator, &execution).await;
        let version = described
            .get("stateMachineVersionArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("expected a routed version in {described:?}"))
            .to_string();
        *counts.entry(version).or_default() += 1;
    }
    counts
}

/// Assert that the executions `counts` records were drawn from `entries` alone, and
/// that every entry at a non-zero weight ran some of them.
fn assert_drawn_from(case: &str, phase: &str, counts: &BTreeMap<String, usize>, entries: &[(String, i64)]) {
    for (version, count) in counts {
        assert!(
            entries.iter().any(|(entry, _)| entry == version),
            "case {case:?} ({phase}): {version} is not in the routing configuration \
             {entries:?}, yet it ran {count} executions"
        );
    }
    for (version, weight) in entries {
        let count = counts.get(version).copied().unwrap_or(0);
        if *weight == 0 {
            assert_eq!(
                count, 0,
                "case {case:?} ({phase}): a version at weight 0 runs nothing, \
                 got {count} of {STARTS}"
            );
        } else {
            assert!(
                count > 0,
                "case {case:?} ({phase}): a version at weight {weight} must run some of \
                 the {STARTS} executions, got {count}"
            );
        }
    }
}

/// An update of the routing configuration shifts which versions later executions run —
/// over a table whose rows differ along the shift itself: onto another version, onto a
/// split, off a split onto one version, and onto a version held at weight 0 beside the
/// one that carries the whole weight.
#[tokio::test]
async fn an_update_shifts_the_versions_a_later_execution_runs() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "alias-update-shift", 3).await;
    let versions: Vec<String> = (1..=3).map(|number| format!("{machine}:{number}")).collect();

    // (case, the entries created, the entries updated to), as (version index, weight).
    let cases: Vec<ShiftCase<'_>> = vec![
        (
            "the whole weight moved onto another version",
            vec![(0, 100)],
            vec![(1, 100)],
        ),
        (
            "one version split into two",
            vec![(0, 100)],
            vec![(0, 50), (1, 50)],
        ),
        (
            "a split collapsed onto a version neither entry named",
            vec![(0, 50), (1, 50)],
            vec![(2, 100)],
        ),
        (
            "a second version named at weight 0",
            vec![(1, 100)],
            vec![(0, 0), (2, 100)],
        ),
    ];

    for (index, (case, before, after)) in cases.iter().enumerate() {
        let entries = |spec: &Vec<(usize, i64)>| -> Vec<(String, i64)> {
            spec.iter()
                .map(|(version, weight)| (versions[*version].clone(), *weight))
                .collect()
        };
        let (before, after) = (entries(before), entries(after));
        let name = format!("shifted-{index}");
        let created: Vec<Value> = before
            .iter()
            .map(|(version, weight)| route(version, *weight))
            .collect();
        let arn = create_alias(&emulator, &name, "before", &created).await;

        let drawn = routed_versions(&emulator, &arn, &format!("{name}-before")).await;
        assert_drawn_from(case, "before the update", &drawn, &before);

        emulator
            .ok(
                "UpdateStateMachineAlias",
                &update_request(&arn, &("after".to_string(), after.clone())),
            )
            .await;
        // The record reports the configuration the update wrote, entry for entry and
        // in the order it was given.
        let described = describe_alias(&emulator, &arn).await;
        assert_eq!(
            reported(&described),
            ("after".to_string(), after.clone()),
            "case {case:?}: expected the updated parameters, got {described:?}"
        );

        let drawn = routed_versions(&emulator, &arn, &format!("{name}-after")).await;
        assert_drawn_from(case, "after the update", &drawn, &after);
        // Every version the update dropped runs nothing at all afterwards.
        for (version, _) in &before {
            if !after.iter().any(|(entry, _)| entry == version) {
                assert_eq!(
                    drawn.get(version).copied().unwrap_or(0),
                    0,
                    "case {case:?}: {version} was dropped by the update, so it runs none \
                     of the {STARTS} later executions, got {drawn:?}"
                );
            }
        }
    }
}
