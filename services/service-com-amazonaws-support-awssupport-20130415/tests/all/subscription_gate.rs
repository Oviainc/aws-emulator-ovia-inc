// SPDX-License-Identifier: CC0-1.0

//! The account-level Premium Support subscription gate, end to end over the same
//! router the emulator boots.
//!
//! RULE: the Amazon Web Services Support API is served only to an account holding a
//! qualifying Support plan; a call from an account without one is refused with HTTP
//! 400 and the error code `SubscriptionRequiredException`, for EVERY operation the
//! service binds and before the request's input is validated, before any case is
//! looked up, and before anything is written.
//!
//! Each suite asserts that rule over a SET of inputs: the operation (mutators,
//! readers, and an operation that is still a stub), the shape of the input (complete,
//! empty, naming a case that does not exist), and the plan the account holds (every
//! plan the documentation names, in several spellings, plus plans that do not
//! qualify).

use serde_json::{json, Value};

use super::case_lifecycle::{error_code, Support, SUBSCRIBED_PLAN};

/// The wire code and status AWS answers a Support API call from an account with no
/// qualifying plan with. The shape is absent from this service's model
/// (`smithy_query(shape="SubscriptionRequiredException")` finds nothing), so this
/// pair — not the message text — is what a client can rely on.
const SUBSCRIPTION_REQUIRED: &str = "SubscriptionRequiredException";
const SUBSCRIPTION_REQUIRED_STATUS: u16 = 400;

/// A `caseId` of the documented shape that no call ever created, so a row carrying
/// it would reach `CaseIdNotFound` if the gate did not precede the case lookup.
const ABSENT_CASE_ID: &str = "case-123456789012-muen-2025-0123456789abcdef";

/// Every operation the service binds, with an input to call it with. Spans the
/// implemented mutators and readers and the operations that are still stubs (which
/// answer HTTP 501 for a subscribed account), because the gate fronts the whole API
/// rather than the case operations alone. The operation names and their required
/// members are the model's: see `OPERATIONS.md`, rendered from
/// `smithy-model/com-amazonaws-support-awssupport-20130415.json`.
fn every_bound_operation() -> Vec<(&'static str, Value)> {
    vec![
        (
            "AddAttachmentsToSet",
            json!({ "attachments": [{ "fileName": "a.txt", "data": "" }] }),
        ),
        (
            "AddCommunicationToCase",
            json!({ "caseId": ABSENT_CASE_ID, "communicationBody": "body" }),
        ),
        (
            "CompleteAttachmentUpload",
            json!({ "uploadId": "upload", "completedUploads": [] }),
        ),
        (
            "CreateCase",
            json!({ "subject": "subject", "communicationBody": "body" }),
        ),
        ("DescribeAttachment", json!({ "attachmentId": "attachment" })),
        (
            "DescribeAttachmentUploadStatus",
            json!({ "uploadId": "upload" }),
        ),
        ("DescribeCases", json!({})),
        (
            "DescribeCommunications",
            json!({ "caseId": ABSENT_CASE_ID }),
        ),
        (
            "DescribeCreateCaseOptions",
            json!({
                "issueType": "technical",
                "serviceCode": "amazon-elastic-compute-cloud-linux",
                "language": "en",
                "categoryCode": "performance",
            }),
        ),
        ("DescribeServices", json!({})),
        ("DescribeSeverityLevels", json!({})),
        (
            "DescribeSupportedLanguages",
            json!({
                "issueType": "technical",
                "serviceCode": "amazon-elastic-compute-cloud-linux",
                "categoryCode": "performance",
            }),
        ),
        (
            "DescribeTrustedAdvisorCheckRefreshStatuses",
            json!({ "checkIds": ["check"] }),
        ),
        (
            "DescribeTrustedAdvisorCheckResult",
            json!({ "checkId": "check" }),
        ),
        ("DescribeTrustedAdvisorChecks", json!({ "language": "en" })),
        (
            "DescribeTrustedAdvisorCheckSummaries",
            json!({ "checkIds": ["check"] }),
        ),
        (
            "GetAttachmentDownloadLink",
            json!({ "attachmentId": "attachment" }),
        ),
        ("GetAttachmentUploadLinks", json!({ "fileName": "a.txt" })),
        ("RefreshTrustedAdvisorCheck", json!({ "checkId": "check" })),
        ("ResolveCase", json!({ "caseId": ABSENT_CASE_ID })),
    ]
}

/// Assert one call was refused by the gate.
fn assert_refused(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, SUBSCRIPTION_REQUIRED_STATUS,
        "case {case:?}: expected HTTP {SUBSCRIPTION_REQUIRED_STATUS}, got {status} {body}"
    );
    let code = error_code(body);
    assert_eq!(
        code, SUBSCRIPTION_REQUIRED,
        "case {case:?}: expected error code {SUBSCRIPTION_REQUIRED}, got {code:?} in {body}"
    );
    assert!(
        body["message"].as_str().is_some_and(|m| !m.is_empty()),
        "case {case:?}: expected the refusal to carry a message, got {body}"
    );
}

/// RULE: with no qualifying plan, EVERY bound operation answers the
/// subscription-required refusal — the implemented ones and the stubs alike.
#[tokio::test]
async fn no_plan_refuses_every_bound_operation() {
    let support = Support::with_plan(None);
    for (operation, input) in every_bound_operation() {
        let (status, body) = support.call(operation, &input).await;
        assert_refused(operation, status, &body);
    }
}

/// RULE: the gate precedes request validation and resource lookup, so the refusal
/// does not depend on the input being well formed or on the named case existing.
/// Rows differ along the shape of the input: complete, empty (so required members
/// are absent), carrying a member outside its modelled bound, and naming a case id
/// that was never created.
#[tokio::test]
async fn the_refusal_precedes_input_and_resource_validation() {
    let support = Support::with_plan(None);
    let rows = [
        (
            "complete input",
            "CreateCase",
            json!({ "subject": "subject", "communicationBody": "body" }),
        ),
        (
            "required members absent",
            "CreateCase",
            json!({ "dryRun": false }),
        ),
        (
            "communicationBody shorter than the modelled minimum of 1",
            "AddCommunicationToCase",
            json!({ "caseId": ABSENT_CASE_ID, "communicationBody": "" }),
        ),
        (
            "maxResults below the modelled range of 10..100",
            "DescribeCommunications",
            json!({ "caseId": ABSENT_CASE_ID, "maxResults": 1 }),
        ),
        (
            "a caseId no call ever created",
            "ResolveCase",
            json!({ "caseId": ABSENT_CASE_ID }),
        ),
        (
            "a caseId that is not even of the documented shape",
            "DescribeCommunications",
            json!({ "caseId": "not-a-case-id" }),
        ),
        (
            "a nextToken this service never minted",
            "DescribeCases",
            json!({ "nextToken": "zzzz" }),
        ),
        (
            "dryRun, which otherwise has its own refusal",
            "CreateCase",
            json!({ "subject": "s", "communicationBody": "b", "dryRun": true }),
        ),
    ];
    for (case, operation, input) in rows {
        let (status, body) = support.call(operation, &input).await;
        assert_refused(&format!("{operation}: {case}"), status, &body);
    }
}

/// RULE: which plan the account holds decides whether the API is served at all.
///
/// The qualifying plans are the ones this service's own documentation names (see
/// `state::QUALIFYING_SUPPORT_PLANS`, which cites the sentence): Business Support+,
/// Enterprise Support and Unified Operations, plus Business, Enterprise On-Ramp and
/// Enterprise. Rows vary the plan, its spelling (case, separators, the "Amazon Web
/// Services" / "AWS" prefix the documentation writes) and whether it qualifies;
/// Basic and Developer are the plans AWS publishes that do not.
#[tokio::test]
async fn only_a_qualifying_plan_reaches_the_operations() {
    let rows: [(Option<&str>, bool); 14] = [
        (Some("business"), true),
        (Some("Business Support+"), true),
        (Some("businesssupport+"), true),
        (Some("Amazon Web Services Enterprise Support"), true),
        (Some("enterprise"), true),
        (Some("Enterprise On-Ramp"), true),
        (Some("AWS Enterprise On-Ramp"), true),
        (Some("unified_operations"), true),
        (Some("Unified Operations"), true),
        (Some("basic"), false),
        (Some("Developer"), false),
        (Some("Business Partner"), false),
        (Some(""), false),
        (None, false),
    ];
    for (plan, qualifies) in rows {
        let support = Support::with_plan(plan);
        let input = json!({ "subject": "subject", "communicationBody": "body" });
        let (status, body) = support.call("CreateCase", &input).await;
        if qualifies {
            assert_eq!(
                status, 200,
                "plan {plan:?}: a qualifying plan must reach CreateCase, got {status} {body}"
            );
            assert!(
                body["caseId"].as_str().is_some(),
                "plan {plan:?}: expected a caseId, got {body}"
            );
        } else {
            assert_refused(&format!("plan {plan:?}"), status, &body);
        }
    }
}

/// RULE: the gate fronts the operations this service BINDS. A request naming none
/// is still answered by the routing path — the refusal is about the account's plan,
/// not a catch-all for every request the service cannot serve.
#[tokio::test]
async fn a_request_naming_no_bound_operation_is_not_a_subscription_refusal() {
    for plan in [None, Some(SUBSCRIBED_PLAN)] {
        let support = Support::with_plan(plan);
        let (status, body) = support.call("NoSuchOperation", &json!({})).await;
        assert_ne!(
            error_code(&body),
            SUBSCRIPTION_REQUIRED,
            "plan {plan:?}: an unknown operation is a routing failure, got {status} {body}"
        );
    }
}

/// RULE: a refused call runs nothing — so nothing it would have written exists once
/// the account's plan qualifies, and nothing it would have read was lost. Proved by
/// reading the store back through `DescribeCases` on the SAME emulator after the
/// plan changes, in both directions.
#[tokio::test]
async fn a_refused_call_neither_writes_nor_loses_state() {
    let support = Support::with_plan(None);

    // Three creates and a resolve, all refused.
    for attempt in 0..3 {
        let input = json!({
            "subject": format!("refused {attempt}"),
            "communicationBody": "body",
        });
        let (status, body) = support.call("CreateCase", &input).await;
        assert_refused(&format!("refused create {attempt}"), status, &body);
    }
    let (status, body) = support
        .call("ResolveCase", &json!({ "caseId": ABSENT_CASE_ID }))
        .await;
    assert_refused("refused resolve", status, &body);

    // With a qualifying plan the reader works — and reports no case at all, so none
    // of the refused creates wrote one.
    support.set_plan(Some(SUBSCRIBED_PLAN));
    let listed = support
        .ok("DescribeCases", json!({ "includeResolvedCases": true }))
        .await;
    let cases = listed["cases"].as_array().map_or(0, Vec::len);
    assert_eq!(
        cases, 0,
        "a refused CreateCase must create nothing, got {listed}"
    );

    // A case created under a qualifying plan survives the account losing it: the
    // refusal is about the call, not about the stored resource.
    let case_id = support
        .create_case(json!({ "subject": "kept", "communicationBody": "body" }))
        .await;
    support.set_plan(Some("basic"));
    let (status, body) = support
        .call("DescribeCases", &json!({ "caseIdList": [&case_id] }))
        .await;
    assert_refused("read under a non-qualifying plan", status, &body);
    support.set_plan(Some(SUBSCRIBED_PLAN));
    let listed = support
        .ok("DescribeCases", json!({ "caseIdList": [&case_id] }))
        .await;
    let subjects: Vec<&str> = listed["cases"]
        .as_array()
        .map(|cases| {
            cases
                .iter()
                .filter_map(|case| case["subject"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        subjects,
        vec!["kept"],
        "the stored case must survive a refused read, got {listed}"
    );
}
