// SPDX-License-Identifier: CC0-1.0

//! The `dryRun` pre-flight contract, driven over HTTP through the same router the
//! production emulator serves with.
//!
//! The rule under test, from the `dryRun` member's own modelled documentation
//! (`smithy_query(query="doc",shape="CreateDeploymentInput",member="dryRun")`, and the
//! identical sentence on `UpdateDeploymentInput$dryRun`): a request that sets `dryRun`
//! checks the action "without actually making the request, and provides an error
//! response" — never a success — and the error response for a caller who holds the
//! required permissions is `DryRunOperation`. So, on EVERY operation this service
//! models `dryRun` on:
//!
//! 1. a pre-flight that passes every check the real request would be judged by is
//!    answered with that error code and its HTTP status, carries no resource member,
//!    and leaves the stored state byte-for-byte as it was;
//! 2. a pre-flight of a request the service would REFUSE reports that refusal instead
//!    — the pre-flight answers the outcome the real request would have had;
//! 3. dropping `dryRun` from the very same body performs the action, so the flag is
//!    the only difference between a check and a call.
//!
//! Every assertion is made over a TABLE spanning the class rather than over one call:
//! both operations that model the member, every workload and deployment pattern the
//! catalogue holds, settings maps of two different sizes, a request with and without
//! `tags`, and an update with and without `force`. No workload, pattern, field name or
//! allowed value is written by hand — each is read out of what the service itself
//! publishes for the pattern.

use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_launchwizard_launchwizard::state::{
    DRY_RUN_OPERATION_CODE, DRY_RUN_OPERATION_STATUS,
};

use crate::deployment_lifecycle::get;
use crate::deployment_update::{
    conforming_specifications, constrained_field, error_message, every_pattern,
    one_pattern_per_workload, pattern_data, required_field, update_body, with_member,
};
use crate::harness::{deployment_name, walk_pages, Harness};

/// Long enough for every clock-owed step a CREATE schedules to come due (the three
/// `state::TRANSITION_SPAN`s a create walks, plus slack). A deployment whose create is
/// still in flight refuses an update, so the update rows wait this out once. The
/// emulator never sleeps; this is the test waiting for the clock.
const CREATE_SETTLE: Duration = Duration::from_millis(2_100);

/// A value no published schema allows, used to make a row the service would refuse.
const OUTSIDE_ALLOWED: &str = "no-such-allowed-value";

/// One row of the create table: a body that would be served, and what it varies.
struct CreateRow {
    case: String,
    name: String,
    specifications: Map<String, Value>,
    body: Value,
}

/// A `CreateDeployment` request body over an explicit settings map, with `tags` only
/// when `tags` is `Some` — a dry run must answer the same way either way. `name` is the
/// case's descriptive label, carried into a name the service's own name rule accepts by
/// [`deployment_name`].
fn create_request(
    workload: &str,
    pattern: &str,
    name: &str,
    specifications: &Map<String, Value>,
    tags: Option<Value>,
    dry_run: bool,
) -> Value {
    let mut body = json!({
        "workloadName": workload,
        "deploymentPatternName": pattern,
        "name": deployment_name(name),
        "specifications": Value::Object(specifications.clone()),
        "dryRun": dry_run,
    });
    if let Some(tags) = tags {
        body.as_object_mut()
            .expect("a create request body is an object")
            .insert("tags".to_string(), tags);
    }
    body
}

/// Every deployment the service currently stores, as `ListDeployments` reports them —
/// the state a pre-flight must leave untouched.
async fn stored(harness: &Harness, case: &str) -> Vec<Value> {
    walk_pages(
        harness,
        case,
        "/listDeployments",
        &json!({}),
        "deployments",
        100,
    )
    .await
}

/// The create rows: one per deployment pattern the catalogue publishes, alternating
/// between a settings map of only the required fields and one of every published field,
/// and between a request that carries `tags` and one that does not.
async fn create_rows(harness: &Harness) -> Vec<CreateRow> {
    let mut rows = Vec::new();
    for (index, (workload, pattern)) in every_pattern(harness).await.into_iter().enumerate() {
        let case = format!("{workload}/{pattern} (row {index})");
        let data = pattern_data(harness, &case, &workload, &pattern).await;
        let specifications = conforming_specifications(&case, &data, index % 2 == 0);
        let tags = (index % 2 == 1).then(|| json!({"Owner": "emulator-tests"}));
        let name = deployment_name(&format!("dry-run-{index}"));
        let body = create_request(
            &workload,
            &pattern,
            &name,
            &specifications,
            tags,
            /* dry_run */ true,
        );
        rows.push(CreateRow {
            case,
            name,
            specifications,
            body,
        });
    }
    rows
}

#[tokio::test]
async fn a_dry_run_create_answers_the_preflight_error_and_stores_nothing() {
    let harness = Harness::new();
    let rows = create_rows(&harness).await;

    // RULE 1: each row passes every check a real create is judged by, so each is
    // answered with the pre-flight error response rather than success, and the store
    // is unchanged by it.
    for row in &rows {
        let before = stored(&harness, &row.case).await;
        let body = harness
            .refused(
                &row.case,
                "/createDeployment",
                row.body.clone(),
                DRY_RUN_OPERATION_STATUS,
                DRY_RUN_OPERATION_CODE,
            )
            .await;
        assert!(
            body.get("deploymentId").is_none() || body["deploymentId"].is_null(),
            "case {:?}: a pre-flight stores nothing, so it reports no deploymentId, got {}",
            row.case,
            body["deploymentId"]
        );
        let message = error_message(&body);
        assert!(
            !message.is_empty(),
            "case {:?}: the pre-flight error response must say why the request was \
             refused, got {message:?}",
            row.case
        );
        // `DeploymentSpecifications` is @sensitive: no submitted value is replayed.
        for value in row.specifications.values().filter_map(Value::as_str) {
            assert!(
                !message.contains(value),
                "case {:?}: the pre-flight must not echo a submitted setting value, got \
                 {message:?}",
                row.case
            );
        }
        let after = stored(&harness, &row.case).await;
        assert_eq!(
            after, before,
            "case {:?}: a pre-flight must leave the stored deployments untouched, got \
             {after:?}",
            row.case
        );
    }

    // RULE 3: the SAME body with `dryRun` dropped performs the create — the flag was
    // the only difference — and the record it stored reads back through GetDeployment.
    for row in &rows {
        let performed = with_member(&row.body, "dryRun", json!(false));
        let answered = harness
            .ok(&row.case, "/createDeployment", performed)
            .await;
        let id = answered["deploymentId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the same request without dryRun must report an id, got \
                     {answered}",
                    row.case
                )
            })
            .to_string();
        let deployment = get(&harness, &row.case, &id).await;
        assert_eq!(
            deployment["specifications"],
            Value::Object(row.specifications.clone()),
            "case {:?}: the performed create must store the settings the pre-flight \
             checked, got {}",
            row.case,
            deployment["specifications"]
        );
        assert_eq!(
            deployment["name"],
            json!(row.name),
            "case {:?}: the performed create must store the name it was given, got {}",
            row.case,
            deployment["name"]
        );
    }
}

#[tokio::test]
async fn a_dry_run_create_the_service_would_refuse_reports_that_refusal() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let case = format!("{workload}/{pattern}");
    let data = pattern_data(&harness, &case, &workload, &pattern).await;
    let conforming = conforming_specifications(&case, &data, true);
    let required = required_field(&case, &data);

    // A stored deployment, so the name-reuse row below has a name to collide with.
    let taken = "dry-run-taken";
    harness
        .ok(
            &case,
            "/createDeployment",
            create_request(
                &workload,
                &pattern,
                taken,
                &conforming,
                None,
                /* dry_run */ false,
            ),
        )
        .await;

    // One row per reason this service refuses a create: a workload and a pattern the
    // catalogue does not publish, a settings map the pattern's own schema warns about
    // (a required field removed, and a value outside a field's allowedValues), and a
    // name a live deployment already holds.
    // The value row uses a field the schema RESTRICTS to `allowedValues`; a field that
    // publishes none takes any `ValueString`, so it could not state this clause.
    let constrained = constrained_field(&case, &data);
    let mut outside_allowed = conforming.clone();
    outside_allowed.insert(constrained, json!(OUTSIDE_ALLOWED));
    let mut without_required = conforming.clone();
    without_required.remove(&required);
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an unknown workload",
            create_request(
                "NoSuchWorkload",
                &pattern,
                "dry-run-unknown-workload",
                &conforming,
                None,
                true,
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an unknown deployment pattern",
            create_request(
                &workload,
                "NoSuchPattern",
                "dry-run-unknown-pattern",
                &conforming,
                None,
                true,
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a required setting removed",
            create_request(
                &workload,
                &pattern,
                "dry-run-missing-setting",
                &without_required,
                None,
                true,
            ),
            400,
            "ValidationException",
        ),
        (
            "a setting outside its allowedValues",
            create_request(
                &workload,
                &pattern,
                "dry-run-bad-setting",
                &outside_allowed,
                None,
                true,
            ),
            400,
            "ValidationException",
        ),
        (
            "a name a live deployment holds",
            create_request(&workload, &pattern, taken, &conforming, None, true),
            400,
            "ValidationException",
        ),
    ];

    // RULE 2: the pre-flight reports the outcome the real request would have had, so
    // none of these is the pre-flight code.
    for (label, body, status, code) in rows {
        let case = format!("{case}: {label}");
        assert_ne!(
            code, DRY_RUN_OPERATION_CODE,
            "case {case:?}: this row must assert a refusal of its own"
        );
        harness
            .refused(&case, "/createDeployment", body, status, code)
            .await;
    }
}

#[tokio::test]
async fn a_dry_run_update_answers_the_preflight_error_and_changes_nothing() {
    let harness = Harness::new();
    // One deployment per workload, so the rule is driven over more than one pattern's
    // published schema. Each is created first and the whole table waited out once: an
    // update is taken only by a deployment whose lifecycle is at rest.
    let mut rows: Vec<(String, String, Value)> = Vec::new();
    for (index, (workload, pattern)) in one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .enumerate()
    {
        let case = format!("{workload}/{pattern}");
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        let specifications = conforming_specifications(&case, &data, index % 2 == 0);
        let name = format!("dry-run-update-{index}");
        let answered = harness
            .ok(
                &case,
                "/createDeployment",
                create_request(
                    &workload,
                    &pattern,
                    &name,
                    &specifications,
                    None,
                    /* dry_run */ false,
                ),
            )
            .await;
        let id = answered["deploymentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no deploymentId in {answered}"))
            .to_string();
        rows.push((case, id, data));
    }
    std::thread::sleep(CREATE_SETTLE);

    for (case, id, data) in &rows {
        let conforming = conforming_specifications(case, data, true);
        let mut warning = conforming.clone();
        warning.remove(&required_field(case, data));
        let before = get(&harness, case, id).await;

        // Two rows: settings the schema accepts, and settings it warns about carried by
        // `force` — both are requests an update would SERVE, so both are pre-flights
        // that passed every check.
        let probes = [
            ("conforming settings", update_body(id, &conforming)),
            (
                "warned settings, forced",
                with_member(&update_body(id, &warning), "force", json!(true)),
            ),
        ];
        for (label, body) in probes {
            let case = format!("{case}: {label}");
            let answered = harness
                .refused(
                    &case,
                    "/updateDeployment",
                    with_member(&body, "dryRun", json!(true)),
                    DRY_RUN_OPERATION_STATUS,
                    DRY_RUN_OPERATION_CODE,
                )
                .await;
            assert!(
                answered.get("deployment").is_none() || answered["deployment"].is_null(),
                "case {case:?}: a pre-flight makes no request, so it reports no \
                 deployment, got {answered}"
            );
            let after = get(&harness, &case, id).await;
            assert_eq!(
                after, before,
                "case {case:?}: a pre-flight must leave the record untouched, got {after}"
            );
        }

        // RULE 2 on this operation too: the same settings WITHOUT `force` are a request
        // the update would refuse, so the pre-flight reports that refusal.
        harness
            .refused(
                &format!("{case}: warned settings, not forced"),
                "/updateDeployment",
                with_member(
                    &with_member(&update_body(id, &warning), "dryRun", json!(true)),
                    "force",
                    json!(false),
                ),
                400,
                "ValidationException",
            )
            .await;
        assert_eq!(
            get(&harness, case, id).await,
            before,
            "case {case:?}: a refused pre-flight must leave the record untouched"
        );

        // RULE 3: the same body with `dryRun` dropped applies the update.
        harness
            .ok(case, "/updateDeployment", update_body(id, &conforming))
            .await;
        let applied = get(&harness, case, id).await;
        assert_eq!(
            applied["specifications"],
            Value::Object(conforming),
            "case {case:?}: the same request without dryRun must apply, got {}",
            applied["specifications"]
        );
    }
}
