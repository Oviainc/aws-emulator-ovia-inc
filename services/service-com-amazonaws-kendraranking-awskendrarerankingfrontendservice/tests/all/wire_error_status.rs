// SPDX-License-Identifier: CC0-1.0

//! Two rules of this service's error paths, end to end over the wire.
//!
//! RULE 1 — the transport status of a client-side failure. This service speaks only
//! the RPC protocols (`awsJson1_0` and `rpcv2Cbor`, see the crate's `OPERATIONS.md`),
//! whose envelope says WHICH failure happened with the error type in the body; the
//! status every client-side failure travels with is the protocol's own 400, never the
//! per-shape `@httpError` the model annotates (`ResourceNotFoundException` 404,
//! `ResourceUnavailableException` 404, `ConflictException` 409,
//! `ServiceQuotaExceededException` 402 — all from `smithy_query query=shape` on this
//! crate's `smithy-model/`). A server fault keeps its own status, so a retry
//! classifier still tells one class from the other.
//!
//! The table below is the CENSUS of the client errors this service's operations can
//! reach end to end: every error shape, through every operation that declares it, so
//! the rule is asserted on shapes and operations the issue never cited (the tagging
//! operations' `ResourceUnavailableException`, `CreateRescoreExecutionPlan`'s
//! `ServiceQuotaExceededException`, the conflict a mid-transition plan answers with).
//!
//! RULE 2 — `Rescore` document validation precedence. A submitted document must carry
//! its body text in exactly one of `Body` / `TokenizedBody`, and may carry its title in
//! at most one of `Title` / `TokenizedTitle` ("You must choose to provide Body or
//! TokenizedBody. You cannot provide both." — `Document$TokenizedBody`; the same
//! sentence for the title on `Document$TokenizedTitle`). The REQUIREMENT on the body is
//! evaluated before the title's exclusivity, so a document failing both is told what it
//! is missing rather than what it duplicates.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::scaffolded;
use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state;

use crate::harness::{
    call, create_active_plan, create_plan_with_tags, emulator, error_code, error_message,
    sleep_past_the_transition,
};

/// A 36-character id of the modelled shape that no create in this suite mints.
const UNKNOWN_PLAN_ID: &str = "00000000-0000-4000-8000-000000000000";

/// One row of the client-error census.
struct ErrorCase {
    /// What this row varies.
    case: String,
    operation: &'static str,
    body: Value,
    /// The error type the body must name.
    expected_code: &'static str,
    /// The `@httpError` status this error's shape is annotated with in the model —
    /// which is exactly what the wire must NOT carry when it is a 4xx.
    modeled_status: u16,
}

/// A `Rescore` document carrying `extra` on top of the required members.
fn document(id: &str, extra: Value) -> Value {
    let mut doc = json!({ "Id": id, "OriginalScore": 0.5 });
    if let Some(members) = extra.as_object() {
        for (key, value) in members {
            doc[key] = value.clone();
        }
    }
    doc
}

/// A `Rescore` request against `plan_id` carrying `documents`.
fn rescore(plan_id: &str, documents: Value) -> Value {
    json!({
        "RescoreExecutionPlanId": plan_id,
        "SearchQuery": "intelligent ranking",
        "Documents": documents,
    })
}

/// Put `id` into the transient `UPDATING` status, so the next mutating call on it is a
/// conflict. The plan must be `ACTIVE` when this is called.
async fn begin_update(router: &MultiServiceRouter, id: &str) {
    let (status, updated) = call(
        router,
        "UpdateRescoreExecutionPlan",
        json!({ "Id": id, "Description": "the update that opens the window" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "opening the update window on {id}: got {status} {updated}"
    );
}

#[tokio::test]
async fn every_client_error_travels_with_the_protocols_client_error_status() {
    let router = emulator();
    // A live plan, so a refusal is about the id or ARN in the request and not about an
    // empty store; ACTIVE, so a delete or update of it is about the request too.
    let (live_id, live_arn) = create_plan_with_tags(&router, "live-plan", Value::Null).await;
    // A second plan, left mid-update, is what the conflict rows address.
    let (conflicted_id, _) = create_plan_with_tags(&router, "conflicted-plan", Value::Null).await;
    sleep_past_the_transition();
    begin_update(&router, &conflicted_id).await;

    // An ARN of the right shape naming a plan this store never held, and one naming no
    // resource at all.
    let unknown_arn = live_arn.replace(&live_id, UNKNOWN_PLAN_ID);
    let not_an_arn = "not-an-arn";

    let mut cases: Vec<ErrorCase> = vec![
        ErrorCase {
            case: "an id no plan was created for, on the describe".to_string(),
            operation: "DescribeRescoreExecutionPlan",
            body: json!({ "Id": UNKNOWN_PLAN_ID }),
            expected_code: "ResourceNotFoundException",
            modeled_status: 404,
        },
        ErrorCase {
            case: "an id no plan was created for, on the update".to_string(),
            operation: "UpdateRescoreExecutionPlan",
            body: json!({ "Id": UNKNOWN_PLAN_ID, "Name": "no-such-plan" }),
            expected_code: "ResourceNotFoundException",
            modeled_status: 404,
        },
        ErrorCase {
            case: "an id no plan was created for, on the delete".to_string(),
            operation: "DeleteRescoreExecutionPlan",
            body: json!({ "Id": UNKNOWN_PLAN_ID }),
            expected_code: "ResourceNotFoundException",
            modeled_status: 404,
        },
        ErrorCase {
            case: "an id no plan was created for, on the data plane".to_string(),
            operation: "Rescore",
            body: rescore(
                UNKNOWN_PLAN_ID,
                json!([document("d1", json!({ "Body": "a body" }))]),
            ),
            expected_code: "ResourceNotFoundException",
            modeled_status: 404,
        },
        ErrorCase {
            case: "a mutating call on a plan mid-transition, the update".to_string(),
            operation: "UpdateRescoreExecutionPlan",
            body: json!({ "Id": conflicted_id, "Name": "conflicting-rename" }),
            expected_code: "ConflictException",
            modeled_status: 409,
        },
        ErrorCase {
            case: "a mutating call on a plan mid-transition, the delete".to_string(),
            operation: "DeleteRescoreExecutionPlan",
            body: json!({ "Id": conflicted_id }),
            expected_code: "ConflictException",
            modeled_status: 409,
        },
        ErrorCase {
            case: "a plan that is not ACTIVE, on the data plane".to_string(),
            operation: "Rescore",
            body: rescore(
                &conflicted_id,
                json!([document("d1", json!({ "Body": "a body" }))]),
            ),
            expected_code: "ValidationException",
            modeled_status: 400,
        },
        ErrorCase {
            case: "a request outside a modelled constraint, refused at the boundary"
                .to_string(),
            operation: "DescribeRescoreExecutionPlan",
            body: json!({ "Id": "too-short" }),
            expected_code: "ValidationException",
            modeled_status: 400,
        },
    ];
    // The tagging operations answer an ARN that resolves to no plan with their own
    // not-found shape, so the rule is asserted on a second 404 shape as well.
    for (arn_case, arn) in [
        ("an ARN naming a plan this store never held", unknown_arn.as_str()),
        ("a string that is not an ARN", not_an_arn),
    ] {
        for (operation, body) in [
            (
                "TagResource",
                json!({ "ResourceARN": arn, "Tags": [{ "Key": "k", "Value": "v" }] }),
            ),
            ("UntagResource", json!({ "ResourceARN": arn, "TagKeys": ["k"] })),
            ("ListTagsForResource", json!({ "ResourceARN": arn })),
        ] {
            cases.push(ErrorCase {
                case: format!("{arn_case}, on the tagging surface"),
                operation,
                body,
                expected_code: "ResourceUnavailableException",
                modeled_status: 404,
            });
        }
    }

    for case in &cases {
        let (status, answer) = call(&router, case.operation, case.body.clone()).await;
        assert_eq!(
            status,
            scaffolded::CLIENT_ERROR_STATUS,
            "case {:?} on {}: a client-side failure must carry the protocol's \
             client-error status, got {status} {answer}",
            case.case,
            case.operation
        );
        assert_eq!(
            error_code(&answer),
            case.expected_code,
            "case {:?} on {}: unexpected error type, got {status} {answer}",
            case.case,
            case.operation
        );
        assert!(
            !error_message(&answer).is_empty(),
            "case {:?} on {}: the body must still state what was wrong, got {answer}",
            case.case,
            case.operation
        );
        if case.modeled_status != scaffolded::CLIENT_ERROR_STATUS {
            assert_ne!(
                status, case.modeled_status,
                "case {:?} on {}: the shape's @httpError {} must not reach the wire, \
                 got {status} {answer}",
                case.case, case.operation, case.modeled_status
            );
        }
    }

    // The refusals changed nothing: both plans are still there, so none of the rows
    // above passed by destroying the store.
    let (status, listed) = call(&router, "ListRescoreExecutionPlans", json!({})).await;
    assert_eq!(status, 200, "list after the refusals: got {listed}");
    assert_eq!(
        listed["SummaryItems"].as_array().map(Vec::len),
        Some(2),
        "a refused call must not create or drop a plan, got {listed}"
    );
}

#[tokio::test]
async fn the_quota_error_travels_with_the_protocols_client_error_status() {
    let router = emulator();
    // `ServiceQuotaExceededException` (@httpError 402) is only reachable with the store
    // full, so it gets its own row here rather than joining the census above.
    for n in 0..state::MAX_RESCORE_EXECUTION_PLANS {
        create_plan_with_tags(&router, &format!("quota-plan-{n}"), Value::Null).await;
    }

    let (status, answer) = call(
        &router,
        "CreateRescoreExecutionPlan",
        json!({ "Name": "one-too-many" }),
    )
    .await;
    assert_eq!(
        status,
        scaffolded::CLIENT_ERROR_STATUS,
        "a create past the quota must carry the protocol's client-error status, \
         got {status} {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "ServiceQuotaExceededException",
        "a create past the quota: unexpected error type, got {status} {answer}"
    );
}

#[test]
fn client_error_statuses_collapse_and_every_other_status_is_left_alone() {
    // The left column is every `@httpError` status this service's model annotates
    // (smithy_query query=shape on ValidationException 400,
    // ServiceQuotaExceededException 402, AccessDeniedException 403,
    // ResourceNotFoundException / ResourceUnavailableException 404,
    // ConflictException 409, ThrottlingException 429, InternalServerException 500),
    // plus each bound of the client-error class and one status past it.
    let cases: Vec<(u16, u16)> = vec![
        (400, 400),
        (402, 400),
        (403, 400),
        (404, 400),
        (409, 400),
        (429, 400),
        // Both bounds of the 4xx class, and the statuses either side of them.
        (399, 399),
        (499, 400),
        (500, 500),
        // The scaffold channel's own status, which is not a client error.
        (501, 501),
        (503, 503),
    ];
    for (modeled, expected) in cases {
        let actual = scaffolded::wire_status(modeled);
        assert_eq!(
            actual, expected,
            "case @httpError {modeled}: expected the wire status {expected}, got {actual}"
        );
    }
}

/// One row of the `Rescore` document-validation table.
struct DocumentCase {
    /// What this row varies.
    case: &'static str,
    documents: Value,
    /// The whole message the refusal must state, or `None` where the request is valid
    /// and must be served.
    expected_message: Option<String>,
}

#[tokio::test]
async fn a_document_missing_its_body_is_told_so_before_any_exclusivity_rule() {
    let router = emulator();
    let plan = create_active_plan(&router, "validation-plan").await;

    let body_required = Some(state::DOCUMENT_BODY_REQUIRED_MESSAGE.to_string());
    let cases = vec![
        DocumentCase {
            // The precedence row: the document fails BOTH rules, and the one it is
            // told about is the body it is MISSING.
            case: "neither body form, and both title forms",
            documents: json!([document(
                "d1",
                json!({ "Title": "a title", "TokenizedTitle": ["a", "title"] })
            )]),
            expected_message: body_required.clone(),
        },
        DocumentCase {
            case: "neither body form, and a text title",
            documents: json!([document("d1", json!({ "Title": "a title" }))]),
            expected_message: body_required.clone(),
        },
        DocumentCase {
            case: "neither body form, and a pre-tokenized title",
            documents: json!([document("d1", json!({ "TokenizedTitle": ["a", "title"] }))]),
            expected_message: body_required.clone(),
        },
        DocumentCase {
            case: "neither body form, and no title at all",
            documents: json!([document("d1", json!({}))]),
            expected_message: body_required.clone(),
        },
        DocumentCase {
            case: "neither body form, on the second document of the batch",
            documents: json!([
                document("d1", json!({ "Body": "a body" })),
                document("d2", json!({ "Title": "a title", "TokenizedTitle": ["a"] })),
            ]),
            expected_message: body_required.clone(),
        },
        DocumentCase {
            case: "neither body form, with a GroupId beside it",
            documents: json!([document("d1", json!({ "GroupId": "g1" }))]),
            expected_message: body_required.clone(),
        },
        DocumentCase {
            // With the body there, the title exclusivity is what is reported — so the
            // reordering did not swallow the second rule.
            case: "a text body, and both title forms",
            documents: json!([document(
                "d1",
                json!({ "Body": "a body", "Title": "a title", "TokenizedTitle": ["a"] })
            )]),
            expected_message: Some(
                "Document d1 provides both Title and TokenizedTitle; provide one.".to_string(),
            ),
        },
        DocumentCase {
            case: "a pre-tokenized body, and both title forms",
            documents: json!([document(
                "d2",
                json!({ "TokenizedBody": ["a", "body"], "Title": "t", "TokenizedTitle": ["t"] })
            )]),
            expected_message: Some(
                "Document d2 provides both Title and TokenizedTitle; provide one.".to_string(),
            ),
        },
        DocumentCase {
            case: "both body forms, with one title form",
            documents: json!([document(
                "d3",
                json!({ "Body": "a body", "TokenizedBody": ["a", "body"], "Title": "t" })
            )]),
            expected_message: Some(
                "Document d3 provides both Body and TokenizedBody; provide one.".to_string(),
            ),
        },
        DocumentCase {
            case: "a text body only",
            documents: json!([document("d1", json!({ "Body": "a body" }))]),
            expected_message: None,
        },
        DocumentCase {
            case: "a pre-tokenized body only",
            documents: json!([document("d1", json!({ "TokenizedBody": ["a", "body"] }))]),
            expected_message: None,
        },
        DocumentCase {
            case: "a text body with a text title",
            documents: json!([document(
                "d1",
                json!({ "Body": "a body", "Title": "a title" })
            )]),
            expected_message: None,
        },
        DocumentCase {
            case: "a pre-tokenized body with a pre-tokenized title",
            documents: json!([document(
                "d1",
                json!({ "TokenizedBody": ["a"], "TokenizedTitle": ["t"] })
            )]),
            expected_message: None,
        },
        DocumentCase {
            case: "a batch where every document carries one body form",
            documents: json!([
                document("d1", json!({ "Body": "a body" })),
                document("d2", json!({ "TokenizedBody": ["another", "body"] })),
                document("d3", json!({ "Body": "a third body", "Title": "t" })),
            ]),
            expected_message: None,
        },
    ];

    for case in &cases {
        let (status, answer) = call(&router, "Rescore", rescore(&plan, case.documents.clone())).await;
        match &case.expected_message {
            Some(expected) => {
                assert_eq!(
                    status,
                    scaffolded::CLIENT_ERROR_STATUS,
                    "case {:?}: the document must be refused, got {status} {answer}",
                    case.case
                );
                assert_eq!(
                    error_code(&answer),
                    "ValidationException",
                    "case {:?}: unexpected error type, got {status} {answer}",
                    case.case
                );
                assert_eq!(
                    &error_message(&answer),
                    expected,
                    "case {:?}: expected the refusal to state {expected:?}, got {:?}",
                    case.case,
                    error_message(&answer)
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {:?}: a document carrying exactly one body form must be \
                     served, got {status} {answer}",
                    case.case
                );
                let submitted = case.documents.as_array().map(Vec::len);
                assert_eq!(
                    answer["ResultItems"].as_array().map(Vec::len),
                    submitted,
                    "case {:?}: one result item per submitted document, got {answer}",
                    case.case
                );
            }
        }
    }
}
