// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rest of the A/B-test resource family — `GetABTest`,
//! `UpdateABTest` and `DeleteABTest`.
//!
//! `CreateABTest` and `ListABTests` have their own suite (`ab_tests.rs`), and both
//! suites share one harness (`ab_test_support.rs`); what this one pins is the lifecycle
//! a created test then has:
//!
//! * a created A/B test is a RESOURCE of this service, so every member of the creating
//!   request reads back through `GetABTest` — whatever shape of test was asked for —
//!   and `executionStatus` is `RUNNING` exactly when `enableOnCreate` asked for traffic
//!   splitting to begin and `NOT_STARTED` otherwise;
//! * `ABTestStatus` is read off the CLOCK: the transient status the last create or
//!   update put the test in until that transition falls due, and `ACTIVE` afterwards,
//!   so reading it advances nothing;
//! * `UpdateABTest` changes exactly the members its request carried and leaves every
//!   other one as it was, and the change reads back through `GetABTest`;
//! * `DeleteABTest` removes the test, so every later `GetABTest`, `UpdateABTest` and
//!   `DeleteABTest` for that `abTestId` is refused 404 `ResourceNotFoundException`
//!   with the sentence `AB test not found: abTestId=<abTestId>`;
//! * an update the service cannot act on is refused with the status and code the model
//!   declares: variants that are not one control and one treatment whose weights sum
//!   to 100 are a 400 `ValidationException`, and a rename onto a name another test
//!   already holds is a 409 `ConflictException`;
//! * a replayed `clientToken` answers with the test or the update it already made
//!   rather than making a second one.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator
//! serves with, so the routes, status codes, `restJson1` bodies and modelled error
//! codes are exercised and not just the handler functions.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

use super::ab_test_support::*;

/// Create one A/B test and return its `abTestId`, failing the test if the create did
/// not answer 202.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, created) = create_ab_test(router, body).await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from CreateABTest, got {status} {created:?}"
    );
    text(&created, "abTestId").to_string()
}

/// One row of the create/read round-trip table.
struct ReadCase {
    case: &'static str,
    name: &'static str,
    description: Option<&'static str>,
    /// `enableOnCreate` as the request carries it: absent, `false` or `true`.
    enable_on_create: Option<bool>,
    /// The control and treatment weights, in the order the request lists the variants.
    weights: (i64, i64),
    /// Whether the treatment variant is listed first, so ordering is exercised too.
    treatment_first: bool,
    gateway_arn: &'static str,
    /// The single target path a `gatewayFilter` restricts the test to, or none.
    target_path: Option<&'static str>,
    per_variant_evaluation: bool,
    expected_execution_status: &'static str,
}

/// The create/read table: a row per shape of A/B test a caller can ask for.
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            case: "even split, enableOnCreate absent",
            name: "checkout_flow",
            description: None,
            enable_on_create: None,
            weights: (50, 50),
            treatment_first: false,
            gateway_arn: GATEWAY_ARN,
            target_path: None,
            per_variant_evaluation: false,
            expected_execution_status: "NOT_STARTED",
        },
        ReadCase {
            case: "control-heavy split, described, enabled on create",
            name: "search_ranking",
            description: Some("compare the reranked search prompt against the current one"),
            enable_on_create: Some(true),
            weights: (90, 10),
            treatment_first: false,
            gateway_arn: GATEWAY_ARN,
            target_path: Some("/search/*"),
            per_variant_evaluation: false,
            expected_execution_status: "RUNNING",
        },
        ReadCase {
            case: "treatment listed first, per-variant evaluation, explicitly not enabled",
            name: "summary_tone",
            description: Some("tone experiment"),
            enable_on_create: Some(false),
            weights: (25, 75),
            treatment_first: true,
            gateway_arn: OTHER_GATEWAY_ARN,
            target_path: None,
            per_variant_evaluation: true,
            expected_execution_status: "NOT_STARTED",
        },
        ReadCase {
            // The published bounds of `Variant$weight` (1..=100) at both ends of one
            // pair that still sums to 100.
            case: "smallest and largest usable weights",
            name: "A",
            description: Some("x"),
            enable_on_create: Some(true),
            weights: (1, 99),
            treatment_first: false,
            gateway_arn: OTHER_GATEWAY_ARN,
            target_path: Some("/"),
            per_variant_evaluation: true,
            expected_execution_status: "RUNNING",
        },
    ]
}

impl ReadCase {
    /// The request body this row sends.
    fn body(&self) -> Value {
        let control = variant("C", self.weights.0, target_configuration("control"));
        let treatment = variant("T1", self.weights.1, target_configuration("treatment"));
        let variants = if self.treatment_first {
            json!([treatment, control])
        } else {
            json!([control, treatment])
        };
        let mut body = json!({
            "name": self.name,
            "gatewayArn": self.gateway_arn,
            "roleArn": ROLE_ARN,
            "variants": variants,
            "evaluationConfig": if self.per_variant_evaluation {
                per_variant_evaluation_config()
            } else {
                json!({"onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN})
            },
        });
        let object = body.as_object_mut().expect("a JSON object");
        if let Some(description) = self.description {
            object.insert("description".to_string(), json!(description));
        }
        if let Some(enabled) = self.enable_on_create {
            object.insert("enableOnCreate".to_string(), json!(enabled));
        }
        if let Some(path) = self.target_path {
            object.insert("gatewayFilter".to_string(), json!({"targetPaths": [path]}));
        }
        body
    }
}

/// Whatever shape of A/B test a caller asks for, every member of the creating request
/// reads back through `GetABTest`, beside the ARN the create minted and the two
/// statuses the create and `enableOnCreate` chose.
#[tokio::test]
async fn a_created_ab_test_reads_back_through_get() {
    let (router, _engine) = emulator();

    for case in read_cases() {
        let name = case.case;
        let body = case.body();
        let ab_test_id = create_ok(&router, name, body.clone()).await;
        let ab_test_arn =
            format!("arn:aws:bedrock-agentcore:us-east-1:000000000000:ab-test/{ab_test_id}");

        let (status, read) = get_ab_test(&router, &ab_test_id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from GetABTest, got {status} {read:?}"
        );
        for (reported, expected) in [
            ("abTestId", json!(ab_test_id)),
            ("abTestArn", json!(ab_test_arn)),
            ("name", json!(case.name)),
            ("gatewayArn", json!(case.gateway_arn)),
            ("roleArn", json!(ROLE_ARN)),
            ("variants", member(&body, "variants").clone()),
            ("evaluationConfig", member(&body, "evaluationConfig").clone()),
            ("description", member(&body, "description").clone()),
            ("gatewayFilter", member(&body, "gatewayFilter").clone()),
            ("status", json!("CREATING")),
            ("executionStatus", json!(case.expected_execution_status)),
        ] {
            assert_eq!(
                member(&read, reported),
                &expected,
                "case {name:?}: expected GetABTest to report {reported} as {expected:?}, \
                 got {read:?}"
            );
        }
        assert_eq!(
            member(&read, "createdAt"),
            member(&read, "updatedAt"),
            "case {name:?}: a test nothing has updated was last updated when it was \
             created, got {read:?}"
        );
        assert_eq!(
            member(&read, "startedAt").is_number(),
            case.expected_execution_status == "RUNNING",
            "case {name:?}: startedAt is reported exactly when traffic splitting has \
             started, got {read:?}"
        );
        for absent in [
            "currentRunId",
            "errorDetails",
            "results",
            "maxDurationExpiresAt",
            "stoppedAt",
        ] {
            assert!(
                member(&read, absent).is_null(),
                "case {name:?}: nothing in this emulator produces {absent}, got {read:?}"
            );
        }
    }
}

/// The lifecycle status is read off the clock: a test is `CREATING` while the
/// transition the create started is not yet due, and `ACTIVE` once it is — and two
/// reads inside the transient window answer identically, so polling changes nothing.
#[tokio::test]
async fn the_lifecycle_status_settles_on_the_clock() {
    let (router, _engine) = emulator();
    let case = "create then settle";

    let ab_test_id = create_ok(&router, case, minimal_request("settling_test")).await;

    let (_, first) = get_ab_test(&router, &ab_test_id).await;
    let (_, second) = get_ab_test(&router, &ab_test_id).await;
    assert_eq!(
        first, second,
        "case {case:?}: two reads inside the transient window must answer identically, \
         got {first:?} then {second:?}"
    );
    assert_eq!(
        text(&first, "status"),
        "CREATING",
        "case {case:?}: expected CREATING before the transition falls due, got {first:?}"
    );

    tokio::time::sleep(
        agentcore::state::AB_TEST_PROVISIONING_SPAN + std::time::Duration::from_millis(250),
    )
    .await;

    let (status, settled) = get_ab_test(&router, &ab_test_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetABTest, got {status} {settled:?}"
    );
    assert_eq!(
        text(&settled, "status"),
        "ACTIVE",
        "case {case:?}: expected ACTIVE once the transition is due, got {settled:?}"
    );
    assert_eq!(
        text(&settled, "executionStatus"),
        "NOT_STARTED",
        "case {case:?}: the lifecycle settling must not start traffic splitting, got \
         {settled:?}"
    );
}

/// One row of the update table: the patch to send, the members it is expected to
/// change, and the members whose value an update of this kind also moves.
struct UpdateCase {
    case: &'static str,
    patch: Value,
    changed: Vec<(&'static str, Value)>,
    /// Members whose value is checked separately rather than compared to the created
    /// one: `updatedAt` always, and the execution instants an `executionStatus` change
    /// sets.
    volatile: Vec<&'static str>,
    expected_execution_status: &'static str,
}

/// The update table: a row per member `UpdateABTest` can change.
fn update_cases() -> Vec<UpdateCase> {
    let reweighted = json!([
        variant("C", 20, target_configuration("control")),
        variant("T1", 80, target_configuration("treatment")),
    ]);
    vec![
        UpdateCase {
            case: "description only",
            patch: json!({"description": "now comparing the terser prompt"}),
            changed: vec![("description", json!("now comparing the terser prompt"))],
            volatile: vec!["updatedAt"],
            expected_execution_status: "NOT_STARTED",
        },
        UpdateCase {
            case: "rename",
            patch: json!({"name": "checkout_flow_v2"}),
            changed: vec![("name", json!("checkout_flow_v2"))],
            volatile: vec!["updatedAt"],
            expected_execution_status: "NOT_STARTED",
        },
        UpdateCase {
            case: "role",
            patch: json!({"roleArn": OTHER_ROLE_ARN}),
            changed: vec![("roleArn", json!(OTHER_ROLE_ARN))],
            volatile: vec!["updatedAt"],
            expected_execution_status: "NOT_STARTED",
        },
        UpdateCase {
            case: "reweighted variants",
            patch: json!({"variants": reweighted.clone()}),
            changed: vec![("variants", reweighted)],
            volatile: vec!["updatedAt"],
            expected_execution_status: "NOT_STARTED",
        },
        UpdateCase {
            case: "variants configured from a bundle instead of a target",
            patch: json!({"variants": [
                variant("C", 40, bundle_configuration()),
                variant("T1", 60, bundle_configuration()),
            ]}),
            changed: vec![(
                "variants",
                json!([
                    variant("C", 40, bundle_configuration()),
                    variant("T1", 60, bundle_configuration()),
                ]),
            )],
            volatile: vec!["updatedAt"],
            expected_execution_status: "NOT_STARTED",
        },
        UpdateCase {
            case: "gateway filter",
            patch: json!({"gatewayFilter": {"targetPaths": ["/checkout/*"]}}),
            changed: vec![("gatewayFilter", json!({"targetPaths": ["/checkout/*"]}))],
            volatile: vec!["updatedAt"],
            expected_execution_status: "NOT_STARTED",
        },
        UpdateCase {
            case: "per-variant evaluation configuration",
            patch: json!({"evaluationConfig": per_variant_evaluation_config()}),
            changed: vec![("evaluationConfig", per_variant_evaluation_config())],
            volatile: vec!["updatedAt"],
            expected_execution_status: "NOT_STARTED",
        },
        UpdateCase {
            case: "start traffic splitting",
            patch: json!({"executionStatus": "RUNNING"}),
            changed: vec![("executionStatus", json!("RUNNING"))],
            volatile: vec!["updatedAt", "startedAt"],
            expected_execution_status: "RUNNING",
        },
        UpdateCase {
            case: "pause traffic splitting",
            patch: json!({"executionStatus": "PAUSED"}),
            changed: vec![("executionStatus", json!("PAUSED"))],
            volatile: vec!["updatedAt"],
            expected_execution_status: "PAUSED",
        },
        UpdateCase {
            case: "stop traffic splitting",
            patch: json!({"executionStatus": "STOPPED"}),
            changed: vec![("executionStatus", json!("STOPPED"))],
            volatile: vec!["updatedAt", "stoppedAt"],
            expected_execution_status: "STOPPED",
        },
        UpdateCase {
            case: "two members at once",
            patch: json!({
                "description": "two at once",
                "roleArn": OTHER_ROLE_ARN,
            }),
            changed: vec![
                ("description", json!("two at once")),
                ("roleArn", json!(OTHER_ROLE_ARN)),
            ],
            volatile: vec!["updatedAt"],
            expected_execution_status: "NOT_STARTED",
        },
    ]
}

/// An update changes exactly the members its request carried, reports the lifecycle
/// status it put the test in, and the change reads back through `GetABTest`.
#[tokio::test]
async fn an_update_changes_only_the_members_it_carries() {
    for case in update_cases() {
        let name = case.case;
        // A fresh emulator per row, so a rename cannot collide with another row's name.
        let (router, _engine) = emulator();
        let ab_test_id = create_ok(&router, name, minimal_request("checkout_flow")).await;
        let (_, before) = get_ab_test(&router, &ab_test_id).await;

        let mut body = case.patch.clone();
        body.as_object_mut()
            .expect("a JSON object")
            .insert("clientToken".to_string(), json!(CLIENT_TOKEN));
        let (status, updated) = update_ab_test(&router, &ab_test_id, body).await;
        assert_eq!(
            status, 202,
            "case {name:?}: expected 202 from UpdateABTest, got {status} {updated:?}"
        );
        assert_eq!(
            text(&updated, "abTestId"),
            ab_test_id,
            "case {name:?}: expected the update to report the test it changed, got \
             {updated:?}"
        );
        assert_eq!(
            text(&updated, "status"),
            "UPDATING",
            "case {name:?}: expected the update to report UPDATING, got {updated:?}"
        );
        assert_eq!(
            text(&updated, "executionStatus"),
            case.expected_execution_status,
            "case {name:?}: expected executionStatus {:?}, got {updated:?}",
            case.expected_execution_status
        );

        let (status, after) = get_ab_test(&router, &ab_test_id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from GetABTest, got {status} {after:?}"
        );
        let mut expected = before.clone();
        let object = expected.as_object_mut().expect("a JSON object");
        object.insert("status".to_string(), json!("UPDATING"));
        for (changed, value) in &case.changed {
            object.insert((*changed).to_string(), value.clone());
        }
        for reported in expected
            .as_object()
            .expect("a JSON object")
            .keys()
            .cloned()
            .collect::<Vec<_>>()
        {
            if case.volatile.contains(&reported.as_str()) {
                continue;
            }
            assert_eq!(
                member(&after, &reported),
                member(&expected, &reported),
                "case {name:?}: expected {reported} to be {:?} after the update, got \
                 {after:?}",
                member(&expected, &reported)
            );
        }
        let created_at = member(&after, "createdAt").as_f64().unwrap_or_default();
        let updated_at = member(&after, "updatedAt").as_f64().unwrap_or_default();
        assert!(
            updated_at >= created_at,
            "case {name:?}: expected updatedAt at or after createdAt, got {updated_at} \
             and {created_at}"
        );
        if case.expected_execution_status == "RUNNING" {
            assert!(
                member(&after, "startedAt").is_number(),
                "case {name:?}: a started test reports when it started, got {after:?}"
            );
        }
        if case.expected_execution_status == "STOPPED" {
            assert!(
                member(&after, "stoppedAt").is_number(),
                "case {name:?}: a stopped test reports when it stopped, got {after:?}"
            );
        }
    }
}

/// One row of the update-refusal table.
struct RefusedUpdate {
    case: &'static str,
    patch: Value,
    code: &'static str,
    /// A fragment the message has to carry, so the refusal says what to fix.
    message_contains: &'static str,
}

/// The update-refusal table: each row is a `variants` list the model lets through and
/// the service cannot route traffic with — the same two rules the create holds.
fn refused_updates() -> Vec<RefusedUpdate> {
    vec![
        RefusedUpdate {
            case: "weights over 100",
            patch: json!({"variants": [
                variant("C", 60, target_configuration("control")),
                variant("T1", 50, target_configuration("treatment")),
            ]}),
            code: "ValidationException",
            message_contains: "sum to 100",
        },
        RefusedUpdate {
            case: "weights under 100",
            patch: json!({"variants": [
                variant("C", 50, target_configuration("control")),
                variant("T1", 49, target_configuration("treatment")),
            ]}),
            code: "ValidationException",
            message_contains: "sum to 100",
        },
        RefusedUpdate {
            case: "two control variants",
            patch: json!({"variants": [
                variant("C", 50, target_configuration("control")),
                variant("C", 50, target_configuration("other-control")),
            ]}),
            code: "ValidationException",
            message_contains: "T1",
        },
        RefusedUpdate {
            case: "two treatment variants",
            patch: json!({"variants": [
                variant("T1", 30, target_configuration("treatment")),
                variant("T1", 70, target_configuration("other-treatment")),
            ]}),
            code: "ValidationException",
            message_contains: "C",
        },
    ]
}

/// An update the service cannot act on is refused 400 with the modelled code, and
/// changes nothing: the test still reports the split it was created with.
#[tokio::test]
async fn an_update_the_service_cannot_act_on_is_refused() {
    for case in refused_updates() {
        let name = case.case;
        let (router, _engine) = emulator();
        let ab_test_id = create_ok(&router, name, minimal_request("refused_update")).await;
        let (_, before) = get_ab_test(&router, &ab_test_id).await;

        let (status, refused) = update_ab_test(&router, &ab_test_id, case.patch.clone()).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400 from UpdateABTest, got {status} {refused:?}"
        );
        let code = error_code(&refused);
        assert!(
            code.ends_with(case.code),
            "case {name:?}: expected the {} error code, got {code:?}",
            case.code
        );
        let message = text(&refused, "message");
        assert!(
            message.contains(case.message_contains),
            "case {name:?}: expected a message carrying {:?}, got {message:?}",
            case.message_contains
        );

        let (_, after) = get_ab_test(&router, &ab_test_id).await;
        assert_eq!(
            after, before,
            "case {name:?}: a refused update must change nothing, got {after:?} after \
             {before:?}"
        );
    }
}

/// A rename onto a name another A/B test already holds is refused 409 — `ABTestName`
/// is unique within an account — while renaming a test to the name it already has is
/// accepted.
#[tokio::test]
async fn a_rename_onto_a_held_name_is_refused() {
    let (router, _engine) = emulator();
    let case = "duplicate name";

    create_ok(&router, case, minimal_request("unique_name")).await;
    let second = create_ok(&router, case, minimal_request("another_name")).await;

    let (status, refused) = update_ab_test(&router, &second, json!({"name": "unique_name"})).await;
    assert_eq!(
        status, 409,
        "case {case:?}: expected 409 from a rename onto a held name, got {status} \
         {refused:?}"
    );
    let code = error_code(&refused);
    assert!(
        code.ends_with("ConflictException"),
        "case {case:?}: expected the ConflictException error code, got {code:?}"
    );
    assert!(
        text(&refused, "message").contains("unique_name"),
        "case {case:?}: expected the refusal to name the name it could not take, got \
         {refused:?}"
    );
    let (_, held) = get_ab_test(&router, &second).await;
    assert_eq!(
        text(&held, "name"),
        "another_name",
        "case {case:?}: a refused rename must leave the name alone, got {held:?}"
    );

    let (status, accepted) =
        update_ab_test(&router, &second, json!({"name": "another_name"})).await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from a rename to the same name, got {status} \
         {accepted:?}"
    );
}

/// A deleted A/B test is gone: the delete reports `DELETING` and the identifiers the
/// test was created with, every later call addressing it is refused 404 with the one
/// sentence this service words that refusal in, and the listing no longer holds it.
#[tokio::test]
async fn a_deleted_ab_test_is_refused_by_every_later_call() {
    let (router, _engine) = emulator();
    let case = "delete then read";

    let ab_test_id = create_ok(&router, case, minimal_request("retired_test")).await;
    let (_, read) = get_ab_test(&router, &ab_test_id).await;
    let ab_test_arn = text(&read, "abTestArn").to_string();

    let (status, deleted) = delete_ab_test(&router, &ab_test_id).await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from DeleteABTest, got {status} {deleted:?}"
    );
    assert_eq!(
        text(&deleted, "abTestId"),
        ab_test_id,
        "case {case:?}: expected the delete to report the test it removed, got {deleted:?}"
    );
    assert_eq!(
        text(&deleted, "abTestArn"),
        ab_test_arn,
        "case {case:?}: expected the delete to report the ARN the test was created with, \
         got {deleted:?}"
    );
    assert_eq!(
        text(&deleted, "status"),
        "DELETING",
        "case {case:?}: expected the delete to report DELETING, got {deleted:?}"
    );

    let (status, listed) = list_ab_tests(&router, "maxResults=100").await;
    let page = member(&listed, "abTests").as_array().cloned().unwrap_or_default();
    assert_eq!(
        (status, page.len()),
        (200, 0),
        "case {case:?}: expected a deleted test to be listed nowhere, got {status} \
         {listed:?}"
    );

    // Two ids: the one this test deleted, and one no create ever minted.
    for addressed in [ab_test_id.as_str(), "never_created-0123456789"] {
        let expected = format!("AB test not found: abTestId={addressed}");
        let (status, refused) = get_ab_test(&router, addressed).await;
        assert_eq!(
            (status, text(&refused, "message")),
            (404, expected.as_str()),
            "case {case:?}: expected GetABTest for {addressed:?} to be refused 404 with \
             {expected:?}, got {status} {refused:?}"
        );
        let code = error_code(&refused);
        assert!(
            code.ends_with("ResourceNotFoundException"),
            "case {case:?}: expected the ResourceNotFoundException error code for \
             {addressed:?}, got {code:?}"
        );
        let (status, refused) =
            update_ab_test(&router, addressed, json!({"description": "x"})).await;
        assert_eq!(
            (status, text(&refused, "message")),
            (404, expected.as_str()),
            "case {case:?}: expected UpdateABTest for {addressed:?} to be refused 404 \
             with {expected:?}, got {status} {refused:?}"
        );
        let (status, refused) = delete_ab_test(&router, addressed).await;
        assert_eq!(
            (status, text(&refused, "message")),
            (404, expected.as_str()),
            "case {case:?}: expected DeleteABTest for {addressed:?} to be refused 404 \
             with {expected:?}, got {status} {refused:?}"
        );
    }
}

/// A replayed `clientToken` answers with the work it already did: the create makes no
/// second test and the update is not applied twice.
#[tokio::test]
async fn a_replayed_client_token_does_the_work_once() {
    let (router, _engine) = emulator();
    let case = "replayed tokens";

    let mut body = minimal_request("idempotent_test");
    body.as_object_mut()
        .expect("a JSON object")
        .insert("clientToken".to_string(), json!(CLIENT_TOKEN));

    let (status, first) = create_ab_test(&router, body.clone()).await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from the first create, got {status} {first:?}"
    );
    let (status, replay) = create_ab_test(&router, body.clone()).await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from the replayed create, got {status} {replay:?}"
    );
    assert_eq!(
        member(&first, "abTestId"),
        member(&replay, "abTestId"),
        "case {case:?}: a replayed clientToken must answer with the test it already \
         created, got {first:?} then {replay:?}"
    );
    assert_eq!(
        member(&first, "createdAt"),
        member(&replay, "createdAt"),
        "case {case:?}: a replayed create must not re-date the test, got {first:?} then \
         {replay:?}"
    );

    let ab_test_id = text(&first, "abTestId").to_string();
    let (_, updated) = update_ab_test(
        &router,
        &ab_test_id,
        json!({"description": "first description", "clientToken": OTHER_CLIENT_TOKEN}),
    )
    .await;
    let (status, replayed_update) = update_ab_test(
        &router,
        &ab_test_id,
        json!({"description": "second description", "clientToken": OTHER_CLIENT_TOKEN}),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from the replayed update, got {status} \
         {replayed_update:?}"
    );
    assert_eq!(
        member(&updated, "updatedAt"),
        member(&replayed_update, "updatedAt"),
        "case {case:?}: a replayed update must report the update it already made, got \
         {updated:?} then {replayed_update:?}"
    );
    let (_, read) = get_ab_test(&router, &ab_test_id).await;
    assert_eq!(
        text(&read, "description"),
        "first description",
        "case {case:?}: a replayed clientToken must not apply a second update, got \
         {read:?}"
    );

    // A different token with a different name is a different test.
    let mut other = minimal_request("another_idempotent_test");
    other
        .as_object_mut()
        .expect("a JSON object")
        .insert("clientToken".to_string(), json!(OTHER_CLIENT_TOKEN));
    let (status, fresh) = create_ab_test(&router, other).await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from a create with its own token, got {status} \
         {fresh:?}"
    );
    assert_ne!(
        member(&fresh, "abTestId"),
        member(&first, "abTestId"),
        "case {case:?}: a different clientToken must create its own test, got {fresh:?} \
         and {first:?}"
    );
}
