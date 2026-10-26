// SPDX-License-Identifier: CC0-1.0

//! The RESPONSE FORM of `UpdateDeployment`'s `dryRun` pre-flight, driven over HTTP
//! through the same router the production emulator serves with.
//!
//! The rule under test, from the member's own modelled documentation
//! (`smithy_query(query="doc",shape="UpdateDeploymentInput",member="dryRun")`, the same
//! sentence `CreateDeploymentInput$dryRun` carries): a request that sets the pre-flight
//! flag "provides an error response" — never a success envelope — and for a caller who
//! holds the required permissions that error response is `DryRunOperation`. So, for
//! EVERY request this operation would have served:
//!
//! 1. the answer is that error code with its HTTP status, and carries no `deployment`;
//! 2. its message is the one fixed sentence for the class — the same text whatever the
//!    request carried, so nothing of one request's settings or identifiers is in it;
//! 3. nothing the service stores moves: neither the record a reader reports nor the
//!    deployment's event log, and repeating the pre-flight answers the same way again;
//! 4. the same body with the flag cleared performs the update, so the flag is the only
//!    difference between a check and a call;
//! 5. the operation that models the member on the create path answers the identical
//!    response form, because it is the identical contract.
//!
//! `tests/all/dry_run_preflight.rs` tables the rule across both operations; this suite
//! is the response form on the UPDATE path specifically, spanning what an update may
//! vary over: every deployment pattern the catalogue publishes, settings maps of two
//! sizes, each combination of the two optional version members, and `force` both set
//! and clear. No workload, pattern, field name, allowed value or version is written by
//! hand — each is read out of what the service itself publishes.

use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_launchwizard_launchwizard::state::{
    DRY_RUN_OPERATION_CODE, DRY_RUN_OPERATION_STATUS,
};

use crate::deployment_lifecycle::{create, create_body, get};
use crate::deployment_update::{
    conforming_specifications, error_message, events, every_pattern, pattern_data, update_body,
    with_member,
};
use crate::harness::Harness;

/// Long enough for every clock-owed step a CREATE schedules to come due (the three
/// `state::TRANSITION_SPAN`s a create walks, plus slack). A deployment whose create is
/// still in flight refuses an update outright, so the whole table waits this out once.
/// The emulator never sleeps; this is the test waiting for the clock.
const CREATE_SETTLE: Duration = Duration::from_millis(2_100);

/// The status an update moves a deployment into, declared by the model's
/// `DeploymentStatus` enum for exactly that transition — what row 4 observes once the
/// flag is cleared.
const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";

/// One row of the table: a deployment at rest and an update body that would be served.
struct Row {
    case: String,
    id: String,
    specifications: Map<String, Value>,
    /// The update body WITHOUT `dryRun`, so the same body serves both halves of rule 4.
    body: Value,
}

/// The update bodies: one per deployment pattern the catalogue publishes, alternating
/// between a settings map of only the required fields and one of every published field,
/// cycling through the four combinations of the optional version members, and setting
/// `force` on every other row.
async fn rows(harness: &Harness) -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, (workload, pattern)) in every_pattern(harness).await.into_iter().enumerate() {
        let case = format!("{workload}/{pattern} (row {index})");
        let id = create(harness, &case, &workload, &pattern, &format!("dry-run-form-{index}")).await;
        let data = pattern_data(harness, &case, &workload, &pattern).await;
        let specifications = conforming_specifications(&case, &data, index % 2 == 0);
        // The versions the catalogue publishes for this pattern: the only ones an
        // update may name, so naming them keeps the row one the service would serve.
        let workload_version = data["workloadVersionName"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no workloadVersionName reported: {data}"))
            .to_string();
        let pattern_version = data["deploymentPatternVersionName"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: no deploymentPatternVersionName reported: {data}")
            })
            .to_string();
        let mut body = update_body(&id, &specifications);
        if index % 4 == 1 || index % 4 == 3 {
            body = with_member(&body, "workloadVersionName", json!(workload_version));
        }
        if index % 4 == 2 || index % 4 == 3 {
            body = with_member(
                &body,
                "deploymentPatternVersionName",
                json!(pattern_version),
            );
        }
        body = with_member(&body, "force", json!(index % 2 == 1));
        rows.push(Row {
            case,
            id,
            specifications,
            body,
        });
    }
    rows
}

/// The pre-flight answer to `body` at `path`, as (status, error code, message, body).
async fn preflight(harness: &Harness, path: &str, body: &Value) -> (u16, String, String, Value) {
    let reply = harness.call(path, body).await;
    (
        reply.status,
        reply.error_code.clone().unwrap_or_default(),
        error_message(&reply.body),
        reply.body,
    )
}

#[tokio::test]
async fn a_dry_run_update_answers_the_preflight_error_response_for_every_request_it_would_serve() {
    let harness = Harness::new();
    let rows = rows(&harness).await;
    std::thread::sleep(CREATE_SETTLE);

    let mut messages: Vec<(String, String)> = Vec::new();
    for row in &rows {
        let before = get(&harness, &row.case, &row.id).await;
        let before_events = events(&harness, &row.case, &row.id).await;
        let probe = with_member(&row.body, "dryRun", json!(true));

        // RULE 3's "again": the pre-flight is asked twice and must answer the same
        // thing both times, which a pre-flight that had moved anything could not.
        let mut answers = Vec::new();
        for attempt in 0..2 {
            let case = format!("{}: attempt {attempt}", row.case);
            let (status, code, message, body) =
                preflight(&harness, "/updateDeployment", &probe).await;

            // RULE 1.
            assert_eq!(
                status, DRY_RUN_OPERATION_STATUS,
                "case {case:?}: a pre-flight provides an error response, expected HTTP \
                 {DRY_RUN_OPERATION_STATUS}, got {status} with body {body}"
            );
            assert_eq!(
                code, DRY_RUN_OPERATION_CODE,
                "case {case:?}: expected error code {DRY_RUN_OPERATION_CODE:?}, got \
                 {code:?} with body {body}"
            );
            assert!(
                body.get("deployment").is_none() || body["deployment"].is_null(),
                "case {case:?}: a pre-flight makes no request, so it reports no \
                 deployment, got {body}"
            );

            // RULE 2: a message, and one that replays nothing of this request —
            // `DeploymentSpecifications` is @sensitive, and the identifier the request
            // named is not part of the sentence either.
            assert!(
                !message.is_empty(),
                "case {case:?}: the pre-flight error response must carry a message, got \
                 {message:?}"
            );
            assert!(
                !message.contains(&row.id),
                "case {case:?}: the message must not render the deployment it was asked \
                 about, got {message:?}"
            );
            for value in row.specifications.values().filter_map(Value::as_str) {
                assert!(
                    !message.contains(value),
                    "case {case:?}: the message must not echo a submitted setting value, \
                     got {message:?}"
                );
            }
            answers.push((status, code, message, body));
        }
        assert_eq!(
            answers[0], answers[1],
            "case {:?}: two identical pre-flights must answer identically, got {:?} then \
             {:?}",
            row.case, answers[0], answers[1]
        );
        messages.push((row.case.clone(), answers[0].2.clone()));

        // RULE 3: neither the record nor its event log moved.
        let after = get(&harness, &row.case, &row.id).await;
        assert_eq!(
            after, before,
            "case {:?}: a pre-flight must leave the record untouched, got {after}",
            row.case
        );
        let after_events = events(&harness, &row.case, &row.id).await;
        assert_eq!(
            after_events, before_events,
            "case {:?}: a pre-flight must append no deployment event, got {after_events:?}",
            row.case
        );
    }

    // RULE 2 over the table: ONE fixed sentence for the class, not a sentence built out
    // of the request that happened to ask for it.
    let (first_case, first_message) = messages
        .first()
        .cloned()
        .expect("the table holds at least one row");
    for (case, message) in &messages {
        assert_eq!(
            message, &first_message,
            "case {case:?}: every pre-flight carries the same fixed message as \
             {first_case:?}, got {message:?}"
        );
    }

    // RULE 4: the very same bodies with the flag cleared perform the update, and the
    // settings the pre-flight checked are what the record then reads back as.
    for row in &rows {
        let answered = harness
            .ok(
                &row.case,
                "/updateDeployment",
                with_member(&row.body, "dryRun", json!(false)),
            )
            .await;
        assert_eq!(
            answered["deployment"]["status"].as_str(),
            Some(UPDATE_IN_PROGRESS),
            "case {:?}: the same request without the flag must apply, got {answered}",
            row.case
        );
        let applied = get(&harness, &row.case, &row.id).await;
        assert_eq!(
            applied["specifications"],
            Value::Object(row.specifications.clone()),
            "case {:?}: the performed update must store the settings the pre-flight \
             checked, got {}",
            row.case,
            applied["specifications"]
        );
    }
}

#[tokio::test]
async fn the_update_and_create_preflights_answer_the_identical_response_form() {
    let harness = Harness::new();
    let (workload, pattern) = every_pattern(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let case = format!("{workload}/{pattern}");
    let id = create(&harness, &case, &workload, &pattern, "dry-run-form-shared").await;
    let data = pattern_data(&harness, &case, &workload, &pattern).await;
    let specifications = conforming_specifications(&case, &data, true);
    std::thread::sleep(CREATE_SETTLE);

    // RULE 5: both operations model the same member with the same documented sentence,
    // so a pre-flight that passes its checks is answered the same way on either path —
    // status, code and message.
    let (update_status, update_code, update_message, update_body_answer) = preflight(
        &harness,
        "/updateDeployment",
        &with_member(&update_body(&id, &specifications), "dryRun", json!(true)),
    )
    .await;
    let (create_status, create_code, create_message, create_body_answer) = preflight(
        &harness,
        "/createDeployment",
        &create_body(
            &harness,
            &workload,
            &pattern,
            "dry-run-form-create",
            /* dry_run */ true,
        )
        .await,
    )
    .await;

    assert_eq!(
        (update_status, update_code.as_str()),
        (DRY_RUN_OPERATION_STATUS, DRY_RUN_OPERATION_CODE),
        "case {case:?}: the update pre-flight must answer the documented error \
         response, got {update_status} {update_code:?} with body {update_body_answer}"
    );
    assert_eq!(
        (create_status, create_code.as_str()),
        (update_status, update_code.as_str()),
        "case {case:?}: the create pre-flight must answer the same status and code as \
         the update pre-flight, got {create_status} {create_code:?} with body \
         {create_body_answer}"
    );
    assert_eq!(
        create_message, update_message,
        "case {case:?}: both pre-flights carry the same fixed message, got \
         {create_message:?} and {update_message:?}"
    );
}
