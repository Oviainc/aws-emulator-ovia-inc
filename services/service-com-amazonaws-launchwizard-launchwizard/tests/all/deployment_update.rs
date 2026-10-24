// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateDeployment`, driven over HTTP through the same router
//! the production emulator serves with.
//!
//! The rules under test:
//!
//! * an update REPLACES the settings of the deployment it names, moves it into the
//!   `UPDATE_IN_PROGRESS` status the model declares for an update, and the record it
//!   left behind is what every later reader reports — with `UPDATE_COMPLETED` owed by
//!   the clock rather than by another call;
//! * the settings are judged against the specification schema of the deployment
//!   pattern the deployment deploys (the schema `GetWorkloadDeploymentPattern`
//!   reports), and `force` is what applies an update despite those warnings;
//! * `dryRun` checks the request and persists nothing;
//! * a deployment that is not there, is going away, or is asked for a version the
//!   catalogue does not hold, is refused rather than updated.
//!
//! Every assertion is made over a TABLE spanning the class rather than over one call:
//! every workload and deployment pattern the catalogue holds, settings maps of
//! different sizes, each kind of validation warning, and identifiers spanning the
//! published bounds of `DeploymentId`. No name, allowed value or version is written by
//! hand: each one is read out of what the service itself reports for the pattern.

use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_launchwizard_launchwizard::ops::types::{
    DEPLOYMENT_STATUS_VALUES, EVENT_STATUS_VALUES,
};
use service_com_amazonaws_launchwizard_launchwizard::state::MAX_EVENTS_PER_DEPLOYMENT;

use crate::deployment_lifecycle::{create, get};
use crate::harness::{catalogue, walk_pages, Harness};

/// Long enough for the clock-owed step an update schedules to come due (one
/// `state::TRANSITION_SPAN` plus slack). The emulator never sleeps; this is the test
/// waiting for the clock.
const SETTLE: Duration = Duration::from_millis(1_200);

/// The status an update puts a deployment into, and the one the clock then owes. Both
/// are members of the model's `DeploymentStatus` enum, which declares them for exactly
/// this transition.
const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
/// See [`UPDATE_IN_PROGRESS`].
const UPDATE_COMPLETED: &str = "UPDATE_COMPLETED";

/// The `WorkloadDeploymentPatternData` the service reports for one pattern — the
/// source of every specification name, allowed value and version this suite submits.
async fn pattern_data(harness: &Harness, case: &str, workload: &str, pattern: &str) -> Value {
    let body = harness
        .ok(
            case,
            "/getWorkloadDeploymentPattern",
            json!({"workloadName": workload, "deploymentPatternName": pattern}),
        )
        .await;
    let data = body["workloadDeploymentPattern"].clone();
    assert!(
        data.is_object(),
        "case {case:?}: GetWorkloadDeploymentPattern must report a pattern, got {body}"
    );
    data
}

/// The reported specification schema of `data`.
fn schema_fields(case: &str, data: &Value) -> Vec<Value> {
    let fields = data["specifications"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the pattern reports no specifications: {data}"))
        .clone();
    assert!(
        !fields.is_empty(),
        "case {case:?}: the pattern must publish at least one specification field"
    );
    fields
}

/// A `specifications` map that raises NO warning against `data`'s schema: every field
/// the schema marks required, and one of the values it allows wherever it publishes
/// any. With `include_optional`, every field it publishes — so two rows of the same
/// pattern differ in how many settings they carry.
fn conforming_specifications(case: &str, data: &Value, include_optional: bool) -> Map<String, Value> {
    let mut specifications = Map::new();
    for field in schema_fields(case, data) {
        let name = field["name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a schema field carries no name: {field}"))
            .to_string();
        let required = field["required"].as_str() == Some("Yes");
        if !required && !include_optional {
            continue;
        }
        match first_allowed_value(&field) {
            Some(value) => specifications.insert(name, json!(value)),
            // A field that publishes no allowedValues takes any `ValueString`.
            None => specifications.insert(name.clone(), json!(format!("value-for-{name}"))),
        };
    }
    assert!(
        !specifications.is_empty(),
        "case {case:?}: the schema must ask for at least one setting"
    );
    specifications
}

/// The first value a schema field publishes in `allowedValues`, or `None` when it
/// publishes none.
fn first_allowed_value(field: &Value) -> Option<String> {
    field["allowedValues"]
        .as_array()
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The name of the first field `data`'s schema marks required.
fn required_field(case: &str, data: &Value) -> String {
    schema_fields(case, data)
        .into_iter()
        .find(|field| field["required"].as_str() == Some("Yes"))
        .and_then(|field| field["name"].as_str().map(ToString::to_string))
        .unwrap_or_else(|| panic!("case {case:?}: the schema must mark a field required"))
}

/// The name of the first field `data`'s schema restricts to `allowedValues`.
fn constrained_field(case: &str, data: &Value) -> String {
    schema_fields(case, data)
        .into_iter()
        .find(|field| first_allowed_value(field).is_some())
        .and_then(|field| field["name"].as_str().map(ToString::to_string))
        .unwrap_or_else(|| panic!("case {case:?}: the schema must restrict a field's values"))
}

/// One row of the warning table: what it does to a conforming settings map, and
/// whether the stored schema warns about the result.
type WarningRow = (&'static str, Box<dyn Fn(&mut Map<String, Value>)>, bool);

/// An `UpdateDeployment` request body.
fn update_body(id: &str, specifications: &Map<String, Value>) -> Value {
    json!({"deploymentId": id, "specifications": Value::Object(specifications.clone())})
}

/// `body` with one extra member — `force`, `dryRun` or a version name.
fn with_member(body: &Value, member: &str, value: Value) -> Value {
    let mut object = body
        .as_object()
        .expect("an update request body is an object")
        .clone();
    object.insert(member.to_string(), value);
    Value::Object(object)
}

/// The `message` an error body carries.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The deployment events of `id`, oldest first.
async fn events(harness: &Harness, case: &str, id: &str) -> Vec<Value> {
    let body = harness
        .ok(case, "/listDeploymentEvents", json!({"deploymentId": id}))
        .await;
    body["deploymentEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListDeploymentEvents reported no list: {body}"))
        .clone()
}

/// Every `(workload, pattern)` pair the catalogue holds, in the order it reports them.
async fn every_pattern(harness: &Harness) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for workload in catalogue(harness).await {
        let body = harness
            .ok(
                &workload,
                "/listWorkloadDeploymentPatterns",
                json!({"workloadName": workload}),
            )
            .await;
        for summary in body["workloadDeploymentPatterns"]
            .as_array()
            .unwrap_or_else(|| panic!("case {workload:?}: no patterns reported: {body}"))
        {
            let pattern = summary["deploymentPatternName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {workload:?}: a pattern carries no name"))
                .to_string();
            pairs.push((workload.clone(), pattern));
        }
    }
    assert!(
        pairs.len() >= 3,
        "the catalogue must hold enough patterns to span the rule, got {pairs:?}"
    );
    pairs
}

/// The first pattern of each workload — the smaller table the refusal rules run over.
async fn one_pattern_per_workload(harness: &Harness) -> Vec<(String, String)> {
    let every = every_pattern(harness).await;
    let mut first: Vec<(String, String)> = Vec::new();
    for (workload, pattern) in every {
        if !first.iter().any(|(known, _)| known == &workload) {
            first.push((workload, pattern));
        }
    }
    first
}

#[tokio::test]
async fn an_update_replaces_the_settings_and_every_reader_reports_the_record() {
    let harness = Harness::new();
    // One row per deployment pattern the catalogue holds, and the rows alternate
    // between a settings map of only the required fields and one of every published
    // field, so the rule is exercised over maps of different sizes and contents.
    let mut updated: Vec<(String, String, Map<String, Value>, String)> = Vec::new();
    for (index, (workload, pattern)) in every_pattern(&harness).await.into_iter().enumerate() {
        let case = format!("{workload}/{pattern}");
        let id = create(&harness, &case, &workload, &pattern, &format!("upd-{index}")).await;
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        let specifications = conforming_specifications(&case, &data, index % 2 == 0);
        let created = get(&harness, &case, &id).await;

        let body = harness
            .ok(&case, "/updateDeployment", update_body(&id, &specifications))
            .await;
        let summary = body["deployment"].clone();
        assert!(
            summary.is_object(),
            "case {case:?}: UpdateDeployment must report the deployment, got {body}"
        );
        // Every reported member is the stored record's, not the request's.
        for (member, expected) in [
            ("id", json!(id)),
            ("name", created["name"].clone()),
            ("workloadName", json!(workload)),
            ("patternName", json!(pattern)),
            ("createdAt", created["createdAt"].clone()),
        ] {
            assert_eq!(
                summary[member], expected,
                "case {case:?}: the summary's {member} must come from the record, got {}",
                summary[member]
            );
        }
        let status = summary["status"].as_str().unwrap_or_default();
        assert!(
            DEPLOYMENT_STATUS_VALUES.contains(&status),
            "case {case:?}: status {status:?} is not a DeploymentStatus value"
        );
        // The update is not instantaneous: a caller reading straight back still sees it
        // under way, which is why the transient status is observable at all.
        assert_eq!(
            status, UPDATE_IN_PROGRESS,
            "case {case:?}: an update must report the update under way, got {status:?}"
        );
        let modified = summary["modifiedAt"].as_f64().unwrap_or_default();
        let born = summary["createdAt"].as_f64().unwrap_or_default();
        assert!(
            modified >= born,
            "case {case:?}: modifiedAt {modified} must not precede createdAt {born}"
        );

        // Read back through GetDeployment: the settings are the submitted map and only
        // it — an update REPLACES the settings rather than merging into them, so the
        // key the create stored is gone.
        let read = get(&harness, &case, &id).await;
        assert_eq!(
            read["specifications"],
            Value::Object(specifications.clone()),
            "case {case:?}: the stored settings must be the submitted map, got {}",
            read["specifications"]
        );
        assert_eq!(
            read["status"].as_str(),
            Some(UPDATE_IN_PROGRESS),
            "case {case:?}: GetDeployment must report the same status as the update, got {}",
            read["status"]
        );
        let version = data["workloadVersionName"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the pattern reports no workloadVersionName"))
            .to_string();
        updated.push((case, id, specifications, version));
    }

    // ONE wait for the clock, then the step it owed has been taken for every row —
    // without any further call having asked for it.
    std::thread::sleep(SETTLE);
    for (case, id, specifications, version) in &updated {
        let settled = get(&harness, case, id).await;
        assert_eq!(
            settled["status"].as_str(),
            Some(UPDATE_COMPLETED),
            "case {case:?}: the clock-owed step must have completed the update, got {}",
            settled["status"]
        );
        assert_eq!(
            settled["specifications"],
            Value::Object(specifications.clone()),
            "case {case:?}: the settled record must keep the submitted settings, got {}",
            settled["specifications"]
        );
        // Reading changes nothing a later caller sees.
        let again = get(&harness, case, id).await;
        assert_eq!(
            again, settled,
            "case {case:?}: a second read must report exactly the same deployment"
        );

        // The events are a consequence of the update: the two it appends close the
        // history, each with a modelled `EventStatus`, and the first of them names the
        // version the deployment now deploys.
        let history = events(&harness, case, id).await;
        let names: Vec<&str> = history
            .iter()
            .map(|event| event["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names.iter().rev().take(2).copied().collect::<Vec<&str>>(),
            vec!["DeploymentUpdateCompleted", "DeploymentUpdateInProgress"],
            "case {case:?}: the update must append its two events last, got {names:?}"
        );
        for event in &history {
            let status = event["status"].as_str().unwrap_or_default();
            assert!(
                EVENT_STATUS_VALUES.contains(&status),
                "case {case:?}: event status {status:?} is not an EventStatus value"
            );
        }
        let described = history
            .iter()
            .find(|event| event["name"].as_str() == Some("DeploymentUpdateInProgress"))
            .map(|event| event["description"].as_str().unwrap_or_default().to_string())
            .unwrap_or_default();
        assert!(
            described.contains(version),
            "case {case:?}: the update event must name the version {version:?} the \
             deployment deploys, got {described:?}"
        );
    }
}

#[tokio::test]
async fn settings_the_pattern_warns_about_are_refused_unless_force_is_set() {
    let harness = Harness::new();
    for (workload, pattern) in one_pattern_per_workload(&harness).await {
        let case = format!("{workload}/{pattern}");
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        let required = required_field(&case, &data);
        let restricted = constrained_field(&case, &data);

        // One row per kind of warning the stored schema can state about a submitted
        // map, plus the row that raises none.
        let rows: Vec<WarningRow> = vec![
            ("conforming settings", Box::new(|_: &mut Map<String, Value>| {}), false),
            (
                "a setting the pattern does not define",
                Box::new(|specifications: &mut Map<String, Value>| {
                    specifications.insert(
                        "NoSuchSetting".to_string(),
                        json!("unexpected-setting-value"),
                    );
                }),
                true,
            ),
            (
                "a value outside the field's allowedValues",
                Box::new({
                    let restricted = restricted.clone();
                    move |specifications: &mut Map<String, Value>| {
                        specifications
                            .insert(restricted.clone(), json!("not-an-allowed-value"));
                    }
                }),
                true,
            ),
            (
                "a required field the map does not carry",
                Box::new({
                    let required = required.clone();
                    move |specifications: &mut Map<String, Value>| {
                        specifications.remove(&required);
                    }
                }),
                true,
            ),
        ];

        for (index, (label, mutate, warns)) in rows.into_iter().enumerate() {
            let case = format!("{case} with {label}");
            let name = format!("warn-{}-{index}", workload.to_lowercase());
            let id = create(&harness, &case, &workload, &pattern, &name).await;
            let stored = get(&harness, &case, &id).await;
            let mut specifications = conforming_specifications(&case, &data, true);
            mutate(&mut specifications);
            let body = update_body(&id, &specifications);

            if !warns {
                harness.ok(&case, "/updateDeployment", body.clone()).await;
                let read = get(&harness, &case, &id).await;
                assert_eq!(
                    read["specifications"],
                    Value::Object(specifications.clone()),
                    "case {case:?}: settings the schema accepts must be applied without \
                     force, got {}",
                    read["specifications"]
                );
                continue;
            }

            // Without `force` the warnings are a refusal, and nothing is persisted.
            let refused = harness
                .refused(&case, "/updateDeployment", body.clone(), 400, "ValidationException")
                .await;
            let message = error_message(&refused);
            assert!(
                !message.is_empty(),
                "case {case:?}: a refused update must carry a message, got {refused}"
            );
            // `DeploymentSpecifications` is @sensitive: the report names the setting so
            // the caller can fix the request, and never the value it carried.
            for secret in ["unexpected-setting-value", "not-an-allowed-value"] {
                assert!(
                    !message.contains(secret),
                    "case {case:?}: the report must not echo a submitted value, got \
                     {message:?}"
                );
            }
            let read = get(&harness, &case, &id).await;
            assert_eq!(
                read["specifications"], stored["specifications"],
                "case {case:?}: a refused update must persist nothing, got {}",
                read["specifications"]
            );

            // `force` applies the same update anyway: the warnings are what it
            // overrides.
            harness
                .ok(&case, "/updateDeployment", with_member(&body, "force", json!(true)))
                .await;
            let forced = get(&harness, &case, &id).await;
            assert_eq!(
                forced["specifications"],
                Value::Object(specifications),
                "case {case:?}: force must apply the update, got {}",
                forced["specifications"]
            );
        }
    }
}

#[tokio::test]
async fn a_dry_run_update_checks_the_request_and_persists_nothing() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let case = format!("{workload}/{pattern} dry run");
    let id = create(&harness, &case, &workload, &pattern, "dry-run-target").await;
    let data = pattern_data(&harness, &case, &workload, &pattern).await;
    let conforming = conforming_specifications(&case, &data, true);
    let mut warning = conforming.clone();
    warning.remove(&required_field(&case, &data));

    // Settled first, so the record below is not moving under the clock while the dry
    // runs are made.
    std::thread::sleep(SETTLE);
    let before = get(&harness, &case, &id).await;

    // Every row is a dry run that passes its checks: it answers success, reports no
    // deployment (nothing was made, so there is no updated record to report), and
    // leaves the stored record exactly as it was.
    let rows = [
        ("conforming settings", update_body(&id, &conforming)),
        (
            "settings the schema warns about, forced",
            with_member(&update_body(&id, &warning), "force", json!(true)),
        ),
    ];
    for (label, body) in rows {
        let case = format!("{case}: {label}");
        let answered = harness
            .ok(
                &case,
                "/updateDeployment",
                with_member(&body, "dryRun", json!(true)),
            )
            .await;
        assert!(
            answered.get("deployment").is_none() || answered["deployment"].is_null(),
            "case {case:?}: a dry run makes no request, so it reports no deployment, got \
             {answered}"
        );
        let after = get(&harness, &case, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a dry run must leave the record untouched, got {after}"
        );
    }

    // A dry run still RUNS the checks: the same settings without `force` are refused,
    // and nothing is persisted by that either.
    let refused = with_member(
        &with_member(&update_body(&id, &warning), "dryRun", json!(true)),
        "force",
        json!(false),
    );
    harness
        .refused(
            &format!("{case}: warned settings, not forced"),
            "/updateDeployment",
            refused,
            400,
            "ValidationException",
        )
        .await;
    assert_eq!(
        get(&harness, &case, &id).await,
        before,
        "case {case:?}: a refused dry run must leave the record untouched"
    );

    // And the same request made for real is what changes the record, so the dry run
    // was the only difference.
    harness
        .ok(&case, "/updateDeployment", update_body(&id, &conforming))
        .await;
    let applied = get(&harness, &case, &id).await;
    assert_eq!(
        applied["specifications"],
        Value::Object(conforming),
        "case {case:?}: the same request without dryRun must apply, got {}",
        applied["specifications"]
    );
}

#[tokio::test]
async fn an_update_of_a_deployment_that_is_absent_or_going_away_is_refused() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let case = format!("{workload}/{pattern}");
    let data = pattern_data(&harness, &case, &workload, &pattern).await;
    let conforming = conforming_specifications(&case, &data, true);
    // A stored deployment exists throughout, so a 404 below is about the id looked up
    // rather than about an empty store.
    let stored = create(&harness, &case, &workload, &pattern, "update-404-seed").await;

    // Well-formed `deploymentId`s (len 2..128, `^[a-zA-Z0-9-]+$`) no deployment has:
    // the rows span both published length bounds and the forms the pattern allows.
    let unknown = [
        ("minimum length 2", "ab".to_string()),
        ("maximum length 128", "d".repeat(128)),
        ("shaped like a minted id", "3f8e1c2d-5a6b-47c8-9d0e-1f2a3b4c5d6e".to_string()),
        ("all digits", "1234567890".to_string()),
        ("hyphens and mixed case", "Nope-NOT-Here-9".to_string()),
    ];
    for (label, id) in &unknown {
        let case = format!("{case} {label}");
        let body = harness
            .refused(
                &case,
                "/updateDeployment",
                update_body(id, &conforming),
                404,
                "ResourceNotFoundException",
            )
            .await;
        let message = error_message(&body);
        assert_eq!(
            message, "The specified deployment does not exist.",
            "case {case:?}: a deployment lookup carries its fixed message, got {message:?}"
        );
        assert!(
            !message.contains(id.as_str()),
            "case {case:?}: the message must not render the requested id, got {message:?}"
        );
    }

    // One past each published bound of `DeploymentId` is a constraint failure, which is
    // a 400 rather than the not-found answer.
    for (label, id) in [
        ("one below the minimum length", "a".to_string()),
        ("one above the maximum length", "d".repeat(129)),
        ("character outside the pattern", "no such id".to_string()),
    ] {
        harness
            .refused(
                &format!("{case} {label}"),
                "/updateDeployment",
                update_body(&id, &conforming),
                400,
                "ValidationException",
            )
            .await;
    }

    // A deployment whose delete has started has no settings left to apply: the update
    // is refused while the delete is under way AND once it has finished, and `force`
    // does not override it — it overrides validation warnings, not the state of the
    // resource.
    let doomed = create(&harness, &case, &workload, &pattern, "update-while-deleting").await;
    let before = get(&harness, &case, &doomed).await;
    harness
        .ok(&case, "/deleteDeployment", json!({"deploymentId": doomed}))
        .await;
    for (label, forced) in [("deleting", false), ("deleting, forced", true)] {
        harness
            .refused(
                &format!("{case} {label}"),
                "/updateDeployment",
                with_member(&update_body(&doomed, &conforming), "force", json!(forced)),
                400,
                "ValidationException",
            )
            .await;
    }
    std::thread::sleep(SETTLE);
    harness
        .refused(
            &format!("{case} deleted"),
            "/updateDeployment",
            update_body(&doomed, &conforming),
            400,
            "ValidationException",
        )
        .await;
    let after = get(&harness, &case, &doomed).await;
    assert_eq!(
        after["specifications"], before["specifications"],
        "case {case:?}: a refused update must persist nothing, got {}",
        after["specifications"]
    );

    // The deployment nobody touched is still updatable, so the refusals above are
    // about the deployments they named.
    let body = harness
        .ok(&case, "/updateDeployment", update_body(&stored, &conforming))
        .await;
    assert_eq!(
        body["deployment"]["id"],
        json!(stored),
        "case {case:?}: the untouched deployment must still update, got {body}"
    );
}

#[tokio::test]
async fn an_update_may_only_name_a_version_the_catalogue_holds() {
    let harness = Harness::new();
    for (workload, pattern) in one_pattern_per_workload(&harness).await {
        let case = format!("{workload}/{pattern}");
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        let conforming = conforming_specifications(&case, &data, true);
        let id = create(
            &harness,
            &case,
            &workload,
            &pattern,
            &format!("ver-{}", workload.to_lowercase()),
        )
        .await;
        let stored = get(&harness, &case, &id).await;

        // The versions the catalogue publishes for this pattern are accepted, together
        // and one at a time.
        let workload_version = data["workloadVersionName"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no workloadVersionName reported"))
            .to_string();
        let pattern_version = data["deploymentPatternVersionName"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no deploymentPatternVersionName reported"))
            .to_string();

        // `WorkloadVersionName` / `DeploymentPatternVersionName` are len 5..30 with
        // pattern `^(([A-Za-z0-9][a-zA-Z0-9-]*)|(\d+\.\d+\.\d+))$`, so every row below
        // is a well-formed version the catalogue simply does not hold: both forms the
        // pattern allows, and each published length bound.
        let absent = [
            ("a dotted version", "9.9.9".to_string()),
            ("a dashed version", "no-such-version".to_string()),
            ("minimum length 5", "ab-cd".to_string()),
            ("maximum length 30", format!("v{}", "9".repeat(29))),
        ];
        for (member, fixed) in [
            ("workloadVersionName", "The specified workload does not exist or insufficient permissions."),
            (
                "deploymentPatternVersionName",
                "The specified deployment pattern does not exist or insufficient permissions.",
            ),
        ] {
            for (label, version) in &absent {
                let case = format!("{case} {member} {label}");
                let body = harness
                    .refused(
                        &case,
                        "/updateDeployment",
                        with_member(&update_body(&id, &conforming), member, json!(version)),
                        404,
                        "ResourceNotFoundException",
                    )
                    .await;
                let message = error_message(&body);
                assert_eq!(
                    message, fixed,
                    "case {case:?}: a catalogue lookup carries its fixed message, got \
                     {message:?}"
                );
                assert!(
                    !message.contains(version.as_str()),
                    "case {case:?}: the message must not render the requested version, got \
                     {message:?}"
                );
            }
            // None of those refusals changed the record.
            let read = get(&harness, &case, &id).await;
            assert_eq!(
                read["specifications"], stored["specifications"],
                "case {case:?}: a refused update must persist nothing, got {}",
                read["specifications"]
            );
        }

        for (label, body) in [
            (
                "both versions",
                with_member(
                    &with_member(
                        &update_body(&id, &conforming),
                        "workloadVersionName",
                        json!(workload_version),
                    ),
                    "deploymentPatternVersionName",
                    json!(pattern_version),
                ),
            ),
            (
                "the workload version alone",
                with_member(
                    &update_body(&id, &conforming),
                    "workloadVersionName",
                    json!(workload_version),
                ),
            ),
            (
                "the pattern version alone",
                with_member(
                    &update_body(&id, &conforming),
                    "deploymentPatternVersionName",
                    json!(pattern_version),
                ),
            ),
            ("no version at all", update_body(&id, &conforming)),
        ] {
            let case = format!("{case} naming {label}");
            let answered = harness.ok(&case, "/updateDeployment", body).await;
            assert_eq!(
                answered["deployment"]["status"].as_str(),
                Some(UPDATE_IN_PROGRESS),
                "case {case:?}: a version the catalogue holds must be applied, got {answered}"
            );
            // Whichever version was named, the event records the one the deployment now
            // deploys — which is the catalogue's, since that is the only one it holds.
            let described = events(&harness, &case, &id)
                .await
                .into_iter()
                .filter(|event| event["name"].as_str() == Some("DeploymentUpdateInProgress"))
                .filter_map(|event| event["description"].as_str().map(ToString::to_string))
                .next_back()
                .unwrap_or_default();
            for version in [&workload_version, &pattern_version] {
                assert!(
                    described.contains(version.as_str()),
                    "case {case:?}: the update event must name version {version:?}, got \
                     {described:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_event_log_of_one_deployment_is_bounded_however_often_it_is_updated() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let case = format!("{workload}/{pattern} event log");
    let id = create(&harness, &case, &workload, &pattern, "event-log-bound").await;
    let data = pattern_data(&harness, &case, &workload, &pattern).await;
    let conforming = conforming_specifications(&case, &data, true);

    // An update is repeatable, so a deployment's event log is the one part of its record
    // a caller can drive the growth of: it is capped at
    // `state::MAX_EVENTS_PER_DEPLOYMENT`, and a capped log drops its OLDEST events.
    for call in 0..MAX_EVENTS_PER_DEPLOYMENT + 16 {
        harness
            .ok(
                &format!("{case} call {call}"),
                "/updateDeployment",
                update_body(&id, &conforming),
            )
            .await;
    }
    // Waited out so the last clock-owed step has come due before the history is read,
    // and the log is therefore quiescent while it is paged over.
    std::thread::sleep(SETTLE);
    let history = walk_pages(
        &harness,
        &case,
        "/listDeploymentEvents",
        &json!({"deploymentId": id}),
        "deploymentEvents",
        100,
    )
    .await;
    assert_eq!(
        history.len(),
        MAX_EVENTS_PER_DEPLOYMENT,
        "case {case:?}: the event log must stop growing at its cap of {}, got {}",
        MAX_EVENTS_PER_DEPLOYMENT,
        history.len()
    );
    // The oldest events are the ones dropped, so the create's own event has aged out
    // while the most recent history — what a caller polls for — is kept.
    let names: Vec<&str> = history
        .iter()
        .map(|event| event["name"].as_str().unwrap_or_default())
        .collect();
    assert!(
        !names.contains(&"DeploymentCreated"),
        "case {case:?}: a capped log must drop its oldest events first, got {names:?}"
    );
    assert_eq!(
        names.last().copied(),
        Some("DeploymentUpdateCompleted"),
        "case {case:?}: the newest event must be kept, got {names:?}"
    );

    // One more update still only appends, so the log stays at the cap rather than
    // growing by one.
    harness
        .ok(&case, "/updateDeployment", update_body(&id, &conforming))
        .await;
    let after = walk_pages(
        &harness,
        &case,
        "/listDeploymentEvents",
        &json!({"deploymentId": id}),
        "deploymentEvents",
        100,
    )
    .await;
    assert_eq!(
        after.len(),
        MAX_EVENTS_PER_DEPLOYMENT,
        "case {case:?}: a further update must not grow the log past {}, got {}",
        MAX_EVENTS_PER_DEPLOYMENT,
        after.len()
    );
}
