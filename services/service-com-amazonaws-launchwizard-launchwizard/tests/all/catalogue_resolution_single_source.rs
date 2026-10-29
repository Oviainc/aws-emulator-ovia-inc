// SPDX-License-Identifier: CC0-1.0

//! The workload-then-pattern resolution has ONE statement in this crate —
//! `state::resolve_workload` and the `state::resolve_pattern` built on top of it — and
//! every operation that names a catalogue resource reaches it through that one path.
//!
//! RULE: a deployment pattern exists only inside a workload, so a request that names
//! both resolves them in the order they scope each other and is refused with the
//! modelled `ResourceNotFoundException` (HTTP 404) carrying the fixed, identifier-free
//! message of the OUTERMOST kind it named that the catalogue does not hold — the
//! workload's whenever the workload is absent, whatever pattern name accompanies it, and
//! the pattern's only once the workload has resolved. A request that names a workload
//! alone takes just the first step of that same rule.
//!
//! The suite exists because that two-step lookup and its two message constants used to
//! be written out once per handler — four full copies, two partial ones, and a seventh
//! formulation that folded the two steps together and answered the PATTERN's message for
//! an absent workload. The rows below drive the one statement end-to-end through an
//! operation from each former copy, so they can no longer answer different refusals, and
//! they assert no response byte changed: the admitting half requires every reader to
//! report the resolved record, and the deployment the update rows run against is read
//! back afterwards.
//!
//! The rows differ along what the rule generalises over: WHICH operation (and so which
//! modelled error enum and request shape) resolves the names, WHICH step of the rule is
//! the one that fails (the workload, the pattern, or both at once), and the FORM of the
//! absent name — spanning both published length bounds and the character forms of the
//! shapes that carry it — over every workload the catalogue holds.

use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_launchwizard_launchwizard::state::{
    service_state, CATALOGUE_PATTERN_VERSIONS, DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE,
    WORKLOAD_NOT_FOUND_MESSAGE,
};

use crate::deployment_lifecycle::{create, get};
use crate::deployment_update::{
    conforming_specifications, error_message, one_pattern_per_workload, pattern_data, update_body,
};
use crate::harness::Harness;

/// One workload's update row: the deployment the refusals run against, the catalogue
/// pair it was created against, and settings that pattern's schema accepts (so a refusal
/// is about the name that is missing and not about the settings).
struct Row {
    case: String,
    workload: String,
    pattern: String,
    id: String,
    specifications: Map<String, Value>,
}

/// The HTTP status and wire error code a catalogue lookup that resolves nothing carries
/// (`ResourceNotFoundException`, `errorKind: client`, `httpError: 404`).
const STATUS: u16 = 404;
/// See [`STATUS`].
const CODE: &str = "ResourceNotFoundException";

/// Long enough for every clock-owed step a CREATE schedules to come due (the three
/// `state::TRANSITION_SPAN`s it walks, plus slack): the update rows below need a
/// deployment at rest, since one whose create is still in flight refuses an update.
const CREATE_SETTLE: Duration = Duration::from_millis(2_100);

/// Well-formed `workloadName`s the seeded catalogue does not hold.
///
/// `WorkloadName` is @length(min: 1, max: 100) with @pattern `^[A-Za-z][a-zA-Z0-9-_]*$`
/// — from smithy_query(query="shape", shape="WorkloadName") — so the rows span both
/// published bounds and the character forms it allows.
fn absent_workloads() -> Vec<(&'static str, String)> {
    vec![
        ("plain unknown", "AbsentWorkloadName".to_string()),
        ("minimum length 1", "Y".to_string()),
        ("maximum length 100", format!("A{}", "b".repeat(99))),
        ("underscore and hyphen", "Absent_Work-load7".to_string()),
    ]
}

/// Well-formed `deploymentPatternName`s no workload of the catalogue holds.
///
/// `DeploymentPatternName` is @length(min: 1, max: 256) with @pattern
/// `^[A-Za-z0-9][a-zA-Z0-9-]*$` — from smithy_query(query="shape",
/// shape="DeploymentPatternName") — so the rows span both published bounds and the
/// character forms it allows (including the digit a pattern name may start with, which a
/// workload name may not).
fn absent_patterns() -> Vec<(&'static str, String)> {
    vec![
        ("plain unknown", "AbsentPatternName".to_string()),
        ("minimum length 1", "X".to_string()),
        ("maximum length 256", format!("A{}", "b".repeat(255))),
        ("digit-led", "7AbsentPatternName".to_string()),
    ]
}

/// One request that reaches the shared resolution: the operation, the route the model
/// binds it to, the body naming `workload` and `pattern`, and whether this request names
/// a deployment pattern at all (a workload-only reader takes the first step alone).
///
/// `CreateDeployment` carries a well-formed `specifications` map rather than one of the
/// named pattern's schema: a create resolves the catalogue BEFORE it judges the settings,
/// so a row whose workload or pattern is absent is refused for that name and never for
/// its settings.
fn readers(
    workload: &str,
    pattern: &str,
    version: &str,
) -> Vec<(&'static str, &'static str, Value, bool)> {
    vec![
        (
            "GetWorkload",
            "/getWorkload",
            json!({"workloadName": workload}),
            false,
        ),
        (
            "ListWorkloadDeploymentPatterns",
            "/listWorkloadDeploymentPatterns",
            json!({"workloadName": workload}),
            false,
        ),
        (
            "GetWorkloadDeploymentPattern",
            "/getWorkloadDeploymentPattern",
            json!({"workloadName": workload, "deploymentPatternName": pattern}),
            true,
        ),
        (
            "ListDeploymentPatternVersions",
            "/listDeploymentPatternVersions",
            json!({"workloadName": workload, "deploymentPatternName": pattern}),
            true,
        ),
        (
            "GetDeploymentPatternVersion",
            "/getDeploymentPatternVersion",
            json!({
                "workloadName": workload,
                "deploymentPatternName": pattern,
                "deploymentPatternVersionName": version,
            }),
            true,
        ),
        (
            "CreateDeployment",
            "/createDeployment",
            json!({
                "workloadName": workload,
                "deploymentPatternName": pattern,
                "name": "resolution-rule-row",
                "specifications": {"KeyPairName": "resolution-rule"},
            }),
            true,
        ),
    ]
}

/// The message a refused POST of `body` to `path` carries, requiring the modelled 404
/// `ResourceNotFoundException` and that the message render neither name the caller asked
/// about.
async fn refusal_message(
    harness: &Harness,
    case: &str,
    path: &str,
    body: Value,
    probes: &[&str],
) -> String {
    let answered = harness.refused(case, path, body, STATUS, CODE).await;
    let message = error_message(&answered);
    for probe in probes {
        // A one- or two-character name can occur in ordinary English by accident, so the
        // echo probe is only meaningful for a longer one.
        assert!(
            probe.len() < 3 || !message.contains(probe),
            "case {case:?}: the message must not render the requested name {probe:?}, got \
             {message:?}"
        );
    }
    message
}

/// Point the stored deployment `id` at `workload`/`pattern`.
///
/// `UpdateDeployment` reads the names it resolves off the deployment's own record rather
/// than from the request, so the first step of the rule is reachable from it only with a
/// record whose workload the catalogue does not hold — a state no sequence of requests
/// can produce, since a create resolved those names before storing them. It is therefore
/// set up on the stored record through the service's OWN store (the one every request
/// reaches, see `tests/all/state_roundtrip.rs`), and the refusal is then observed over
/// the wire like any other.
fn point_record_at(harness: &Harness, case: &str, id: &str, workload: &str, pattern: &str) {
    let state = service_state(&**harness.engine()).expect("the service's own state store");
    let mut store = state.lock();
    let deployment = store
        .deployments
        .get_mut(id)
        .unwrap_or_else(|| panic!("case {case:?}: the created deployment must be stored"));
    deployment.workload_name = workload.to_string();
    deployment.pattern_name = pattern.to_string();
}

#[tokio::test]
async fn every_catalogue_lookup_resolves_the_workload_then_the_pattern_by_the_one_rule() {
    let harness = Harness::new();
    let published = CATALOGUE_PATTERN_VERSIONS
        .last()
        .copied()
        .expect("the catalogue publishes at least one version per pattern");

    for (index, (workload, pattern)) in one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .enumerate()
    {
        let case = format!("{workload}/{pattern}");

        // The admitting half: every reader resolves names the catalogue DOES hold and
        // reports the record, so the rows below are about which name is missing and not
        // about the lookup having stopped working.
        for (operation, path, body, _) in readers(&workload, &pattern, published) {
            if operation == "CreateDeployment" {
                // Driven through the shared create helper, which asserts the minted id:
                // the row above carries settings of no particular pattern's schema, and a
                // create that is meant to SUCCEED is judged against that schema.
                create(
                    &harness,
                    &format!("{case} CreateDeployment"),
                    &workload,
                    &pattern,
                    // A live deployment holds its name against another create, so each
                    // row's name is its own.
                    &format!("resolution-rule-created-{index}"),
                )
                .await;
                continue;
            }
            harness
                .ok(&format!("{case} {operation}"), path, body)
                .await;
        }

        // Step one fails: the workload is absent, so EVERY reader answers the workload's
        // message — including the ones that also name a pattern the catalogue does hold.
        for (label, absent) in absent_workloads() {
            for (operation, path, body, _) in readers(&absent, &pattern, published) {
                let message = refusal_message(
                    &harness,
                    &format!("{case} {operation} absent workload ({label})"),
                    path,
                    body,
                    &[absent.as_str(), pattern.as_str()],
                )
                .await;
                assert_eq!(
                    message, WORKLOAD_NOT_FOUND_MESSAGE,
                    "case {case:?} {operation} absent workload ({label}): expected the \
                     workload's message {WORKLOAD_NOT_FOUND_MESSAGE:?}, got {message:?}"
                );
            }
        }

        // Step two fails: the workload resolves and the pattern inside it does not, so
        // the readers that name a pattern answer the PATTERN's message.
        for (label, absent) in absent_patterns() {
            for (operation, path, body, names_pattern) in readers(&workload, &absent, published) {
                if !names_pattern {
                    continue;
                }
                let message = refusal_message(
                    &harness,
                    &format!("{case} {operation} absent pattern ({label})"),
                    path,
                    body,
                    &[absent.as_str(), workload.as_str()],
                )
                .await;
                assert_eq!(
                    message, DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE,
                    "case {case:?} {operation} absent pattern ({label}): expected the \
                     pattern's message {DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE:?}, got {message:?}"
                );
            }
        }

        // Both names absent: the ENCLOSING kind is the one reported, so folding the two
        // steps into one lookup — which can only name the inner kind — is observable.
        for ((workload_label, absent_workload), (pattern_label, absent_pattern)) in
            absent_workloads().into_iter().zip(absent_patterns())
        {
            let label = format!("{workload_label} workload, {pattern_label} pattern");
            for (operation, path, body, _) in
                readers(&absent_workload, &absent_pattern, published)
            {
                let message = refusal_message(
                    &harness,
                    &format!("{case} {operation} neither held ({label})"),
                    path,
                    body,
                    &[absent_workload.as_str(), absent_pattern.as_str()],
                )
                .await;
                assert_eq!(
                    message, WORKLOAD_NOT_FOUND_MESSAGE,
                    "case {case:?} {operation} neither held ({label}): the outermost missing \
                     kind is the one reported, expected {WORKLOAD_NOT_FOUND_MESSAGE:?}, got \
                     {message:?}"
                );
            }
        }

        // One kind, one message: the two steps cannot answer the same sentence, or a
        // caller could not tell which of the names it sent is the unheld one.
        assert_ne!(
            WORKLOAD_NOT_FOUND_MESSAGE, DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE,
            "case {case:?}: each kind of missing resource must carry its OWN message"
        );
    }
}

#[tokio::test]
async fn an_update_resolves_its_pattern_through_the_same_two_step_rule() {
    let harness = Harness::new();

    // One deployment per workload, created FIRST and settled with ONE sleep: an update is
    // taken only by a deployment at rest, and a refused update leaves it where it was, so
    // one deployment serves every refusal row of its workload.
    let mut rows: Vec<Row> = Vec::new();
    for (index, (workload, pattern)) in one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .enumerate()
    {
        let case = format!("{workload}/{pattern}");
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        let specifications = conforming_specifications(&case, &data, false);
        let id = create(
            &harness,
            &case,
            &workload,
            &pattern,
            &format!("resolution-rule-{index}"),
        )
        .await;
        rows.push(Row {
            case,
            workload,
            pattern,
            id,
            specifications,
        });
    }
    std::thread::sleep(CREATE_SETTLE);

    for Row {
        case,
        workload,
        pattern,
        id,
        specifications,
    } in rows
    {
        let stored = get(&harness, &case, &id).await;
        let update = update_body(&id, &specifications);

        // Step one, reached from the update: a record whose WORKLOAD the catalogue does
        // not hold is refused for the workload, never for the pattern inside it.
        for (label, absent) in absent_workloads() {
            point_record_at(&harness, &case, &id, &absent, &pattern);
            let message = refusal_message(
                &harness,
                &format!("{case} UpdateDeployment absent workload ({label})"),
                "/updateDeployment",
                update.clone(),
                &[absent.as_str(), pattern.as_str()],
            )
            .await;
            assert_eq!(
                message, WORKLOAD_NOT_FOUND_MESSAGE,
                "case {case:?} absent workload ({label}): expected the workload's message \
                 {WORKLOAD_NOT_FOUND_MESSAGE:?}, got {message:?}"
            );
        }

        // Step two, reached from the update: the workload resolves and the pattern does
        // not.
        for (label, absent) in absent_patterns() {
            point_record_at(&harness, &case, &id, &workload, &absent);
            let message = refusal_message(
                &harness,
                &format!("{case} UpdateDeployment absent pattern ({label})"),
                "/updateDeployment",
                update.clone(),
                &[absent.as_str(), workload.as_str()],
            )
            .await;
            assert_eq!(
                message, DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE,
                "case {case:?} absent pattern ({label}): expected the pattern's message \
                 {DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE:?}, got {message:?}"
            );
        }

        // The record restored to the pair it was created against, the update is taken and
        // the settings it carries land — so the resolution hands the update the pattern's
        // own schema record, and none of the refusals above changed anything else about
        // the deployment.
        point_record_at(&harness, &case, &id, &workload, &pattern);
        let answered = harness
            .ok(&format!("{case} UpdateDeployment"), "/updateDeployment", update)
            .await;
        let summary = answered["deployment"].clone();
        for (member, expected) in [
            ("id", json!(id)),
            ("name", stored["name"].clone()),
            ("workloadName", json!(workload)),
            ("patternName", json!(pattern)),
            ("createdAt", stored["createdAt"].clone()),
        ] {
            assert_eq!(
                summary[member], expected,
                "case {case:?}: the summary's {member} must come from the record, got {}",
                summary[member]
            );
        }
        let read = get(&harness, &case, &id).await;
        assert_eq!(
            read["specifications"],
            Value::Object(specifications.clone()),
            "case {case:?}: the stored settings must be the submitted map, got {}",
            read["specifications"]
        );
    }
}
