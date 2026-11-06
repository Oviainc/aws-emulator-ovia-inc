// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Launch Wizard deployment lifecycle: the seeded workload
//! catalogue, `CreateDeployment`, `GetDeployment`, `ListDeployments`,
//! `DeleteDeployment` and `ListDeploymentEvents`, driven over HTTP through the same
//! router the production emulator serves with.
//!
//! Every assertion here is about a RULE rather than one recorded call, so each one is
//! exercised over a table of inputs: the whole seeded catalogue (not just the `SAP`
//! entry), every page size from one to one-past-the-end, each published bound of
//! `maxResults` and one past it, and both filter keys the model declares.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_launchwizard_launchwizard::ops::types::{
    DEPLOYMENT_FILTER_KEY_VALUES, DEPLOYMENT_STATUS_VALUES, EVENT_STATUS_VALUES,
};
use service_com_amazonaws_launchwizard_launchwizard::state::{
    DRY_RUN_OPERATION_CODE, DRY_RUN_OPERATION_STATUS,
};

// The HTTP harness, the signing region and the pagination walker are shared with the
// other suites in this crate rather than copied — see `tests/all/harness.rs`.
use crate::deployment_update::conforming_specifications;
use crate::harness::{catalogue, deployment_name, walk_pages, Harness, REGION};

/// The canonical emulator account id every ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// Long enough for every clock-owed status step a lifecycle call schedules to come
/// due (the three `TRANSITION_SPAN`s a create walks, plus slack). The emulator never
/// sleeps; this is the test waiting for the clock.
const SETTLE: Duration = Duration::from_millis(2_100);

/// The settings a `CreateDeployment` for `pattern` has to carry: every field THAT
/// pattern's published schema marks `required`, filled with one of the values it allows
/// wherever it publishes any.
///
/// Read off `GetWorkloadDeploymentPattern` rather than written by hand, because the
/// schema is the PATTERN's own: two patterns of one workload require different settings,
/// so a map written out here would be a map for one of them. When the catalogue
/// publishes no such pattern (the not-found rows below), there is no schema to fill in
/// and a well-formed `DeploymentSpecifications` map is enough — the request is refused
/// for the pattern it names, before any schema is consulted.
pub(crate) async fn required_specifications(
    harness: &Harness,
    workload: &str,
    pattern: &str,
) -> Map<String, Value> {
    let case = format!("{workload}/{pattern} schema");
    let reply = harness
        .call(
            "/getWorkloadDeploymentPattern",
            &json!({"workloadName": workload, "deploymentPatternName": pattern}),
        )
        .await;
    if reply.status != 200 {
        return [("KeyPairName".to_string(), json!("key-for-no-such-pattern"))]
            .into_iter()
            .collect();
    }
    conforming_specifications(&case, &reply.body["workloadDeploymentPattern"], false)
}

/// A `CreateDeployment` request body whose `specifications` the named pattern's own
/// published schema accepts — see [`required_specifications`]. A create is judged
/// against that schema, so a body that is meant to succeed fills it in; see
/// `tests/all/create_deployment_validation.rs` for the suite that drives the judgement
/// itself.
///
/// `name` is the case's descriptive label, carried into a name the service's own name
/// rule accepts by [`deployment_name`].
pub(crate) async fn create_body(
    harness: &Harness,
    workload: &str,
    pattern: &str,
    name: &str,
    dry_run: bool,
) -> Value {
    json!({
        "workloadName": workload,
        "deploymentPatternName": pattern,
        "name": deployment_name(name),
        "specifications": Value::Object(required_specifications(harness, workload, pattern).await),
        "tags": {"Owner": "emulator-tests", "Workload": workload},
        "dryRun": dry_run,
    })
}

/// Create a deployment that must succeed, returning its `deploymentId`.
pub(crate) async fn create(
    harness: &Harness,
    case: &str,
    workload: &str,
    pattern: &str,
    name: &str,
) -> String {
    let body = harness
        .ok(
            case,
            "/createDeployment",
            create_body(harness, workload, pattern, name, false).await,
        )
        .await;
    let id = body["deploymentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateDeployment returned no deploymentId: {body}"))
        .to_string();
    // `DeploymentId`: len 2..128, pattern ^[a-zA-Z0-9-]+$.
    assert!(
        (2..=128).contains(&id.len())
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "case {case:?}: minted deploymentId must match ^[a-zA-Z0-9-]+$ with len 2..128, got {id:?}"
    );
    id
}

/// The deployment `GetDeployment` reports for `id`.
pub(crate) async fn get(harness: &Harness, case: &str, id: &str) -> Value {
    let body = harness
        .ok(case, "/getDeployment", json!({"deploymentId": id}))
        .await;
    body["deployment"].clone()
}

#[tokio::test]
async fn the_seeded_workload_catalogue_is_reported_and_paged() {
    let harness = Harness::new();
    let listed = harness.ok("all", "/listWorkloads", json!({})).await;
    let workloads = listed["workloads"]
        .as_array()
        .expect("ListWorkloads must report a workloads list");
    assert!(
        workloads.len() >= 2,
        "the catalogue must be seeded with enough workloads to create against, got {}",
        workloads.len()
    );
    assert!(
        listed.get("nextToken").is_none() || listed["nextToken"].is_null(),
        "the last page must carry no nextToken, got {}",
        listed["nextToken"]
    );

    // Each entry carries the three members `WorkloadDataSummary` reports, and a
    // `status` the model's `WorkloadStatus` enum declares.
    let workload_status_values = ["ACTIVE", "INACTIVE", "DISABLED", "DELETED"];
    for workload in workloads {
        let name = workload["workloadName"].as_str().unwrap_or_default();
        assert!(
            !name.is_empty(),
            "case {workload}: every entry must carry a workloadName"
        );
        assert!(
            workload["displayName"].as_str().is_some_and(|d| !d.is_empty()),
            "case {name:?}: every entry must carry a displayName, got {}",
            workload["displayName"]
        );
        let status = workload["status"].as_str().unwrap_or_default();
        assert!(
            workload_status_values.contains(&status),
            "case {name:?}: status {status:?} is not a WorkloadStatus value"
        );
    }

    // The issue cited `SAP`; the rule is "the whole seeded catalogue is reported", so
    // the other entries are checked too.
    let names: Vec<String> = catalogue(&harness).await;
    for expected in ["SAP", "SQLServer", "ActiveDirectory"] {
        assert!(
            names.iter().any(|name| name == expected),
            "case {expected:?}: the seeded catalogue must report it, got {names:?}"
        );
    }

    // Paging the same catalogue at every size from one item to one past the end
    // yields the same list, with a token only while items remain.
    for page_size in 1..=names.len() + 1 {
        let case = format!("listWorkloads maxResults={page_size}");
        let paged = walk_pages(
            &harness,
            &case,
            "/listWorkloads",
            &json!({}),
            "workloads",
            page_size,
        )
        .await;
        let paged_names: Vec<String> = paged
            .iter()
            .map(|w| w["workloadName"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            paged_names, names,
            "case {case:?}: paging must yield the whole catalogue in order, got {paged_names:?}"
        );
    }
}

#[tokio::test]
async fn a_created_deployment_is_read_back_from_state_and_advances_on_the_clock() {
    let harness = Harness::new();
    let case = "SAP/SapHanaSingle/demo-deployment";
    let request = create_body(&harness, "SAP", "SapHanaSingle", "demo-deployment", false).await;
    let created = harness.ok(case, "/createDeployment", request.clone()).await;
    let id = created["deploymentId"]
        .as_str()
        .expect("CreateDeployment must return a deploymentId")
        .to_string();

    // Read straight back: every member comes from the stored record.
    let deployment = get(&harness, case, &id).await;
    assert_eq!(
        deployment["id"], json!(id),
        "case {case:?}: GetDeployment must report the id it was asked for, got {}",
        deployment["id"]
    );
    for (member, expected) in [
        ("name", &request["name"]),
        ("workloadName", &request["workloadName"]),
        ("patternName", &request["deploymentPatternName"]),
        ("specifications", &request["specifications"]),
        ("tags", &request["tags"]),
    ] {
        assert_eq!(
            &deployment[member], expected,
            "case {case:?}: {member} must come back as stored, got {}",
            deployment[member]
        );
    }
    assert_eq!(
        deployment["deploymentArn"],
        json!(format!("arn:aws:launchwizard:{REGION}:{ACCOUNT}:deployment/{id}")),
        "case {case:?}: the ARN must name this request's region and the emulator account, got {}",
        deployment["deploymentArn"]
    );
    assert!(
        deployment["createdAt"].as_f64().is_some_and(|t| t > 0.0),
        "case {case:?}: createdAt must be a real timestamp, got {}",
        deployment["createdAt"]
    );
    assert!(
        deployment["deletedAt"].is_null(),
        "case {case:?}: a live deployment has no deletedAt, got {}",
        deployment["deletedAt"]
    );

    // A caller acting immediately after the create still observes the transient
    // status: the clock-owed step is never due at zero.
    assert_eq!(
        deployment["status"], json!("CREATING"),
        "case {case:?}: a just-created deployment is CREATING, got {}",
        deployment["status"]
    );

    // And the clock settles it, without the emulator ever having slept.
    std::thread::sleep(SETTLE);
    let settled = get(&harness, case, &id).await;
    assert_eq!(
        settled["status"], json!("COMPLETED"),
        "case {case:?}: the deployment must reach COMPLETED once the clock owes it, got {}",
        settled["status"]
    );
    assert_eq!(
        settled["createdAt"], deployment["createdAt"],
        "case {case:?}: createdAt must not move, got {}",
        settled["createdAt"]
    );
    assert!(
        settled["modifiedAt"].as_f64() >= deployment["modifiedAt"].as_f64(),
        "case {case:?}: modifiedAt must advance with the status, got {} then {}",
        deployment["modifiedAt"],
        settled["modifiedAt"]
    );

    // Reading does not move anything further: two reads agree.
    let again = get(&harness, case, &id).await;
    assert_eq!(
        again, settled,
        "case {case:?}: a second read must report exactly the same deployment"
    );

    // The events are a consequence of the lifecycle, and each status is a modelled one.
    let events = harness
        .ok(case, "/listDeploymentEvents", json!({"deploymentId": id}))
        .await;
    let events = events["deploymentEvents"]
        .as_array()
        .expect("ListDeploymentEvents must report a deploymentEvents list")
        .clone();
    assert_eq!(
        events.len(),
        4,
        "case {case:?}: create plus its three clock-owed steps append four events, got {}",
        events.len()
    );
    for event in &events {
        let status = event["status"].as_str().unwrap_or_default();
        assert!(
            EVENT_STATUS_VALUES.contains(&status),
            "case {case:?}: event status {status:?} is not an EventStatus value"
        );
        assert!(
            event["timestamp"].as_f64().is_some_and(|t| t > 0.0),
            "case {case:?}: every event carries a timestamp, got {}",
            event["timestamp"]
        );
        assert!(
            event["description"]
                .as_str()
                .is_some_and(|d| d.contains(&deployment_name("demo-deployment"))),
            "case {case:?}: an event describes this deployment, got {}",
            event["description"]
        );
    }
}

#[tokio::test]
async fn create_resolves_the_workload_and_pattern_against_the_catalogue() {
    let harness = Harness::new();
    let names = catalogue(&harness).await;

    // Every seeded workload is deployable, and every deployment created is stored —
    // one row per catalogue entry, not just the one the issue cited.
    let mut created: Vec<(String, String)> = Vec::new();
    for (index, workload) in names.iter().enumerate() {
        let pattern = first_pattern(workload);
        let name = format!("from-{workload}-{index}");
        let case = format!("{workload}/{pattern}");
        let id = create(&harness, &case, workload, pattern, &name).await;
        let deployment = get(&harness, &case, &id).await;
        assert_eq!(
            deployment["workloadName"], json!(workload),
            "case {case:?}: the stored workloadName must be the one created against, got {}",
            deployment["workloadName"]
        );
        assert_eq!(
            deployment["patternName"], json!(pattern),
            "case {case:?}: the stored patternName must be the one created against, got {}",
            deployment["patternName"]
        );
        created.push((name, id));
    }
    assert!(
        created.len() >= 2,
        "the catalogue must offer more than one deployable workload, got {}",
        created.len()
    );

    // A workload or pattern the catalogue does not hold is a 404 — including a
    // pattern that exists, but belongs to another workload.
    let other = names
        .iter()
        .find(|name| *name != &names[0])
        .expect("the catalogue holds more than one workload");
    let unknown = [
        ("unknown workload", "NoSuchWorkload".to_string(), first_pattern(&names[0]).to_string()),
        ("unknown pattern", names[0].clone(), "NoSuchPattern".to_string()),
        (
            "pattern of another workload",
            names[0].clone(),
            first_pattern(other).to_string(),
        ),
    ];
    for (label, workload, pattern) in unknown {
        let case = format!("{label}: {workload}/{pattern}");
        harness
            .refused(
                &case,
                "/createDeployment",
                create_body(&harness, &workload, &pattern, "should-not-exist", false).await,
                404,
                "ResourceNotFoundException",
            )
            .await;
    }

    // None of the refused creates stored anything.
    let listed = harness.ok("after refusals", "/listDeployments", json!({})).await;
    let stored = listed["deployments"]
        .as_array()
        .expect("ListDeployments must report a deployments list");
    assert_eq!(
        stored.len(),
        created.len(),
        "a refused create must store nothing: expected {} deployments, got {}",
        created.len(),
        stored.len()
    );
}

/// The first deployment pattern the seeded catalogue offers for `workload`.
///
/// Mirrors `state::seeded_workloads`, whose entries are the Launch Wizard
/// applications and deployment patterns AWS documents.
pub(crate) fn first_pattern(workload: &str) -> &'static str {
    match workload {
        "SAP" => "SapHanaSingle",
        "SQLServer" => "SqlServerSingle",
        "ActiveDirectory" => "AWSManagedAD",
        other => panic!("the test table does not know the patterns of workload {other:?}"),
    }
}

#[tokio::test]
async fn a_deployment_name_is_held_only_while_a_deployment_holds_it() {
    let harness = Harness::new();
    let name = "unique-name";
    let first = create(&harness, "first create", "SAP", "SapHanaSingle", name).await;

    // The same name again is a client error, whatever workload or pattern it is
    // submitted against.
    for (workload, pattern) in [("SAP", "SapHanaMultiNode"), ("SQLServer", "SqlServerHA")] {
        let case = format!("duplicate {name} via {workload}/{pattern}");
        harness
            .refused(
                &case,
                "/createDeployment",
                create_body(&harness, workload, pattern, name, false).await,
                400,
                "ValidationException",
            )
            .await;
    }

    // A different name is accepted, so it is the name that is held and nothing else.
    let second = create(&harness, "other name", "SAP", "SapHanaSingle", "other-name").await;
    assert_ne!(
        first, second,
        "two deployments must get distinct ids, got {first:?} twice"
    );

    // Once the deployment holding the name is gone, the name is free again.
    harness
        .ok("delete first", "/deleteDeployment", json!({"deploymentId": first}))
        .await;
    std::thread::sleep(SETTLE);
    let reused = create(&harness, "reuse after delete", "SAP", "SapHanaSingle", name).await;
    assert_ne!(
        reused, first,
        "reusing the name must mint a new deployment, got the deleted one's id {first:?}"
    );
    let old = get(&harness, "deleted deployment", &first).await;
    assert_eq!(
        old["status"], json!("DELETED"),
        "case reuse: the deleted deployment must stay deleted, got {}",
        old["status"]
    );
}

#[tokio::test]
async fn a_dry_run_validates_and_stores_nothing() {
    let harness = Harness::new();
    let before = harness.ok("before", "/listDeployments", json!({})).await;
    assert_eq!(
        before["deployments"].as_array().map(Vec::len),
        Some(0),
        "a fresh service holds no deployments, got {}",
        before["deployments"]
    );

    // A dry run that passes every check is answered with the documented pre-flight
    // ERROR response rather than success, and stores nothing — see
    // `tests/all/dry_run_preflight.rs` for the rule's own table.
    for (workload, pattern, name) in [
        ("SAP", "SapHanaSingle", "dry-one"),
        ("SQLServer", "SqlServerHA", "dry-two"),
        ("ActiveDirectory", "SelfManagedAD", "dry-three"),
    ] {
        let case = format!("dry run {workload}/{pattern}");
        let body = harness
            .refused(
                &case,
                "/createDeployment",
                create_body(&harness, workload, pattern, name, true).await,
                DRY_RUN_OPERATION_STATUS,
                DRY_RUN_OPERATION_CODE,
            )
            .await;
        assert!(
            body.get("deploymentId").is_none() || body["deploymentId"].is_null(),
            "case {case:?}: a dry run persists nothing, so it reports no deploymentId, got {}",
            body["deploymentId"]
        );
    }

    // A dry run is still validated: an unknown workload is refused the same way.
    harness
        .refused(
            "dry run of an unknown workload",
            "/createDeployment",
            create_body(&harness, "NoSuchWorkload", "SapHanaSingle", "dry-four", true).await,
            404,
            "ResourceNotFoundException",
        )
        .await;

    let after = harness.ok("after", "/listDeployments", json!({})).await;
    assert_eq!(
        after["deployments"].as_array().map(Vec::len),
        Some(0),
        "no dry run may appear in ListDeployments, got {}",
        after["deployments"]
    );
}

#[tokio::test]
async fn list_deployments_pages_and_filters_the_stored_deployments() {
    let harness = Harness::new();
    // Five deployments spread over two workloads, so a filter has something to
    // exclude and pagination has something to split.
    let rows = [
        ("SAP", "SapHanaSingle", "page-one"),
        ("SQLServer", "SqlServerSingle", "page-two"),
        ("SAP", "SapHanaMultiNode", "page-three"),
        ("ActiveDirectory", "AWSManagedAD", "page-four"),
        ("SQLServer", "SqlServerHA", "page-five"),
    ];
    let mut ids = Vec::new();
    for (workload, pattern, name) in rows {
        ids.push(create(&harness, name, workload, pattern, name).await);
    }

    // Unpaged, every stored deployment is reported in creation order.
    let all = harness.ok("unpaged", "/listDeployments", json!({})).await;
    let listed: Vec<String> = all["deployments"]
        .as_array()
        .expect("ListDeployments must report a deployments list")
        .iter()
        .map(|d| d["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed, ids,
        "unpaged: every stored deployment must be reported in creation order, got {listed:?}"
    );

    // Every page size from one to one past the end yields exactly the same list.
    for page_size in 1..=ids.len() + 1 {
        let case = format!("listDeployments maxResults={page_size}");
        let paged = walk_pages(
            &harness,
            &case,
            "/listDeployments",
            &json!({}),
            "deployments",
            page_size,
        )
        .await;
        let paged_ids: Vec<String> = paged
            .iter()
            .map(|d| d["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            paged_ids, ids,
            "case {case:?}: paging must resume where the previous page ended, got {paged_ids:?}"
        );
    }

    // `maxResults` is @range(min: 1, max: 100): both bounds are served, and one past
    // either is refused.
    for (case, max_results, expected) in [
        ("lower bound", 1, 200),
        ("upper bound", 100, 200),
        ("below the lower bound", 0, 400),
        ("above the upper bound", 101, 400),
    ] {
        let reply = harness
            .call("/listDeployments", &json!({"maxResults": max_results}))
            .await;
        assert_eq!(
            reply.status, expected,
            "case {case:?} (maxResults={max_results}): expected HTTP {expected}, got {} with body {}",
            reply.status, reply.body
        );
    }

    // A token this service did not issue is a client error, not a silent restart.
    for (case, token) in [
        ("not hex", "not-a-token"),
        ("hex of nothing we issued", "6e6f70652d686572652d6e6f7065"),
    ] {
        harness
            .refused(
                case,
                "/listDeployments",
                json!({"nextToken": token}),
                400,
                "ValidationException",
            )
            .await;
    }

    // Both `DeploymentFilterKey` values the model declares, each over one value and
    // over several (OR-joined), and the two together (AND-joined).
    let filter_cases: Vec<(&str, Value, BTreeSet<&str>)> = vec![
        (
            "one workload",
            json!([{"name": "WORKLOAD_NAME", "values": ["SAP"]}]),
            ["page-one", "page-three"].into_iter().collect(),
        ),
        (
            "two workloads, OR-joined",
            json!([{"name": "WORKLOAD_NAME", "values": ["SAP", "ActiveDirectory"]}]),
            ["page-one", "page-three", "page-four"].into_iter().collect(),
        ),
        (
            "a workload that deployed nothing",
            json!([{"name": "WORKLOAD_NAME", "values": ["NotDeployed"]}]),
            BTreeSet::new(),
        ),
        (
            "every deployment is CREATING at first",
            json!([{"name": "DEPLOYMENT_STATUS", "values": ["CREATING"]}]),
            [
                "page-one",
                "page-two",
                "page-three",
                "page-four",
                "page-five",
            ]
            .into_iter()
            .collect(),
        ),
        (
            "a status nothing is in",
            json!([{"name": "DEPLOYMENT_STATUS", "values": ["DELETE_FAILED"]}]),
            BTreeSet::new(),
        ),
        (
            "both keys, AND-joined",
            json!([
                {"name": "WORKLOAD_NAME", "values": ["SQLServer"]},
                {"name": "DEPLOYMENT_STATUS", "values": ["CREATING"]},
            ]),
            ["page-two", "page-five"].into_iter().collect(),
        ),
        (
            "both keys, excluding each other",
            json!([
                {"name": "WORKLOAD_NAME", "values": ["SQLServer"]},
                {"name": "DEPLOYMENT_STATUS", "values": ["DELETED"]},
            ]),
            BTreeSet::new(),
        ),
    ];
    for (case, filters, expected) in filter_cases {
        let body = harness
            .ok(case, "/listDeployments", json!({"filters": filters}))
            .await;
        let got: BTreeSet<String> = body["deployments"]
            .as_array()
            .expect("ListDeployments must report a deployments list")
            .iter()
            .map(|d| d["name"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: BTreeSet<String> = expected.into_iter().map(deployment_name).collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected deployments {expected:?}, got {got:?}"
        );
    }

    // A filter name outside `DeploymentFilterKey` is refused.
    assert_eq!(
        DEPLOYMENT_FILTER_KEY_VALUES.len(),
        2,
        "the model declares two DeploymentFilterKey values, got {DEPLOYMENT_FILTER_KEY_VALUES:?}"
    );
    for case in ["workload_name", "PATTERN_NAME"] {
        harness
            .refused(
                case,
                "/listDeployments",
                json!({"filters": [{"name": case, "values": ["SAP"]}]}),
                400,
                "ValidationException",
            )
            .await;
    }
}

#[tokio::test]
async fn delete_is_idempotent_and_observable_from_every_reader() {
    let harness = Harness::new();
    let case = "delete lifecycle";
    let id = create(&harness, case, "SQLServer", "SqlServerHA", "to-be-deleted").await;
    let other = create(&harness, case, "SAP", "SapHanaSingle", "to-be-kept").await;

    // An unknown deployment is a 404 from every reader and from the delete itself.
    for (path, body) in [
        ("/deleteDeployment", json!({"deploymentId": "doesnotexist"})),
        ("/getDeployment", json!({"deploymentId": "doesnotexist"})),
        (
            "/listDeploymentEvents",
            json!({"deploymentId": "doesnotexist"}),
        ),
    ] {
        harness
            .refused(path, path, body, 404, "ResourceNotFoundException")
            .await;
    }

    // The first delete starts it, and reports the status read from state. Both
    // deployments are settled first, so the event history below depends on the calls
    // made rather than on how long this test took to reach the delete.
    std::thread::sleep(SETTLE);
    for (label, settled) in [("to-be-deleted", &id), ("to-be-kept", &other)] {
        let deployment = get(&harness, label, settled).await;
        assert_eq!(
            deployment["status"], json!("COMPLETED"),
            "case {label:?}: the create must have settled before the delete, got {}",
            deployment["status"]
        );
    }
    let first = harness
        .ok(case, "/deleteDeployment", json!({"deploymentId": id}))
        .await;
    let started = first["status"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        started, "DELETE_INITIATING",
        "case {case:?}: the delete must report the status it just entered, got {started:?}"
    );
    assert!(
        DEPLOYMENT_STATUS_VALUES.contains(&started.as_str()),
        "case {case:?}: {started:?} is not a DeploymentStatus value"
    );

    // Repeating it changes nothing: no error, nothing restarted, and the status it
    // reports is the one the clock has reached rather than a restarted first step.
    let repeat = harness
        .ok(case, "/deleteDeployment", json!({"deploymentId": id}))
        .await;
    let repeated = repeat["status"].as_str().unwrap_or_default().to_string();
    assert!(
        ["DELETE_INITIATING", "DELETE_IN_PROGRESS", "DELETED"].contains(&repeated.as_str()),
        "case {case:?}: a repeated delete must report where the delete has got to, got {repeated:?}"
    );

    // Once the clock owes it, the deployment is DELETED, with deletedAt, everywhere.
    std::thread::sleep(SETTLE);
    let deleted = get(&harness, case, &id).await;
    assert_eq!(
        deleted["status"], json!("DELETED"),
        "case {case:?}: the deployment must reach DELETED, got {}",
        deleted["status"]
    );
    let deleted_at = deleted["deletedAt"].as_f64().unwrap_or_default();
    assert!(
        deleted_at > 0.0,
        "case {case:?}: a deleted deployment carries deletedAt, got {}",
        deleted["deletedAt"]
    );

    // A delete after the fact is still idempotent, and does not resurrect anything.
    let after = harness
        .ok(case, "/deleteDeployment", json!({"deploymentId": id}))
        .await;
    assert_eq!(
        after["status"], json!("DELETED"),
        "case {case:?}: deleting an already-deleted deployment reports DELETED, got {}",
        after["status"]
    );
    let again = get(&harness, case, &id).await;
    assert_eq!(
        again["deletedAt"].as_f64().unwrap_or_default(),
        deleted_at,
        "case {case:?}: deletedAt must not move on a repeated delete"
    );

    // ListDeployments reflects the deletion, and the untouched deployment is intact.
    let listed = harness.ok(case, "/listDeployments", json!({})).await;
    let statuses: Vec<(String, String)> = listed["deployments"]
        .as_array()
        .expect("ListDeployments must report a deployments list")
        .iter()
        .map(|d| {
            (
                d["id"].as_str().unwrap_or_default().to_string(),
                d["status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        statuses
            .iter()
            .find(|(listed_id, _)| listed_id == &id)
            .map(|(_, status)| status.as_str()),
        Some("DELETED"),
        "case {case:?}: ListDeployments must report the deletion, got {statuses:?}"
    );
    assert_eq!(
        statuses
            .iter()
            .find(|(listed_id, _)| listed_id == &other)
            .map(|(_, status)| status.as_str()),
        Some("COMPLETED"),
        "case {case:?}: the deployment nobody deleted must be untouched, got {statuses:?}"
    );

    // The event history grew as a consequence of the calls, and pages the same way.
    let events = harness
        .ok(case, "/listDeploymentEvents", json!({"deploymentId": id}))
        .await;
    let events = events["deploymentEvents"]
        .as_array()
        .expect("ListDeploymentEvents must report a deploymentEvents list")
        .clone();
    assert_eq!(
        events.len(),
        7,
        "case {case:?}: the create and its three clock-owed steps, then the delete's \
         synchronous DELETE_INITIATING and its two clock-owed steps, append seven events, got {}",
        events.len()
    );
    for page_size in 1..=events.len() + 1 {
        let paged = walk_pages(
            &harness,
            &format!("listDeploymentEvents maxResults={page_size}"),
            "/listDeploymentEvents",
            &json!({"deploymentId": id}),
            "deploymentEvents",
            page_size,
        )
        .await;
        assert_eq!(
            paged, events,
            "case page_size {page_size}: paging the events must yield the same history in order"
        );
    }

    // An event token belongs to the list that issued it: the same token on another
    // deployment's history is refused rather than silently resuming at that position.
    let first_page = harness
        .ok(
            case,
            "/listDeploymentEvents",
            json!({"deploymentId": id, "maxResults": 1}),
        )
        .await;
    let token = first_page["nextToken"]
        .as_str()
        .expect("a multi-event history paged at one must hand back a token")
        .to_string();
    harness
        .refused(
            "an event token from another deployment",
            "/listDeploymentEvents",
            json!({"deploymentId": other, "nextToken": token}),
            400,
            "ValidationException",
        )
        .await;
}

#[tokio::test]
async fn the_store_and_every_page_are_bounded() {
    let harness = Harness::new();
    // `maxResults` is capped at 100 by the model, so a caller that names none still
    // gets at most one page of 100 and a token for the rest, however many are stored.
    // The first id is kept: a `deploymentId` is a minted UUID, so the oldest record is
    // the one the first create handed back rather than a predictable name.
    let mut created = Vec::new();
    for index in 0..101 {
        created.push(
            create(
                &harness,
                "filling the store",
                "SAP",
                "SapHanaSingle",
                &format!("bounded-{index:03}"),
            )
            .await,
        );
    }
    let oldest = created
        .first()
        .cloned()
        .expect("the store was filled with deployments");
    let unpaged = harness.ok("unpaged", "/listDeployments", json!({})).await;
    assert_eq!(
        unpaged["deployments"].as_array().map(Vec::len),
        Some(100),
        "an unpaged read must stop at the modelled maximum page of 100, got {}",
        unpaged["deployments"]
    );
    let token = unpaged["nextToken"]
        .as_str()
        .expect("a capped first page must hand back a token")
        .to_string();
    let rest = harness
        .ok("second page", "/listDeployments", json!({"nextToken": token}))
        .await;
    assert_eq!(
        rest["deployments"].as_array().map(Vec::len),
        Some(1),
        "the remaining page must carry the one deployment left, got {}",
        rest["deployments"]
    );
    assert!(
        rest.get("nextToken").is_none() || rest["nextToken"].is_null(),
        "the last page must carry no token, got {}",
        rest["nextToken"]
    );

    // The store itself is bounded: filling it and asking for one more is the modelled
    // resource-limit error, not unbounded growth.
    for index in 101..200 {
        create(
            &harness,
            "filling the store",
            "SAP",
            "SapHanaSingle",
            &format!("bounded-{index:03}"),
        )
        .await;
    }
    harness
        .refused(
            "one deployment past the store bound",
            "/createDeployment",
            create_body(&harness, "SAP", "SapHanaSingle", "one-too-many", false).await,
            400,
            "ResourceLimitException",
        )
        .await;

    // Deleting one makes room again: the terminal record is dropped to fit the new one.
    harness
        .ok(
            "free a slot",
            "/deleteDeployment",
            json!({"deploymentId": oldest}),
        )
        .await;
    std::thread::sleep(SETTLE);
    let replacement =
        create(&harness, "after freeing a slot", "SAP", "SapHanaSingle", "room-made").await;
    assert_ne!(
        replacement, oldest,
        "the replacement must be a new record, not the freed one"
    );
    harness
        .refused(
            "the evicted record",
            "/getDeployment",
            json!({"deploymentId": oldest}),
            404,
            "ResourceNotFoundException",
        )
        .await;
}
