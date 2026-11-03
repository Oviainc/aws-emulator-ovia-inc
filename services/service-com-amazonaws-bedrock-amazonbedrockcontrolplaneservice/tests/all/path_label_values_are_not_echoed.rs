// SPDX-License-Identifier: CC0-1.0

//! The two remaining operations that address the automated-reasoning policy TEST CASE
//! through a `@httpLabel`, driven end-to-end over the same `MultiServiceRouter` the
//! emulator boots.
//!
//! The RULE — whether a constraint clause quotes the value a member held is a property of
//! the SHAPE reached at that member, and this service states
//! `AutomatedReasoningPolicyTestCaseId`'s refusals by the member's position alone — is
//! stated and contrasted in `tests/all/refused_subject_follows_the_member_shape.rs`, whose
//! rows reach that shape through the test case's READ and through a test result's. These
//! two rows are the operations it does not reach: the test case's UPDATE and DELETE, which
//! address the same shape with another method, another required-member set and (for the
//! delete) a query parameter beside the label. A property of the shape holds for every
//! operation reaching it, so these are the ones that would show a decision taken per
//! operation instead.
//!
//! This suite is named for the BINDING its rows happen to use — both reach the shape
//! through a `@httpLabel` path segment. That binding is not what decides the subject: a
//! path label of another shape IS quoted back (`guardrailIdentifier`, the evaluation-job
//! `jobIdentifier`), which the sibling suite's contrast rows pin.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation")` / `query="shape"`), never from a recorded response
//! body. The requests go through `crate::support` — the ONE harness every suite in this
//! crate drives the emulator with.

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator};
use crate::validation_message_wording::{pattern_requirement, TEST_CASE_ID};

/// A well-formed `AutomatedReasoningPolicyArn`: the policy need not exist, since the
/// constraint walk refuses the request before any handler reads state.
const POLICY_ARN: &str =
    "arn:aws:bedrock:us-west-2:000000000000:automated-reasoning-policy/abcdef123456";

/// `…/test-cases/{testCaseId}`, as both operations bind it.
fn test_case_path(test_case_id: &str) -> String {
    format!(
        "/automated-reasoning-policies/{}/test-cases/{}",
        label(POLICY_ARN),
        label(test_case_id)
    )
}

/// A test-case update body whose every required member is present and valid, so the
/// `testCaseId` label is the only thing the request breaks (`query="operation"`,
/// `UpdateAutomatedReasoningPolicyTestCase`: `guardContent` and `lastUpdatedAt` are
/// `@required`).
fn update_body() -> Value {
    json!({
        "guardContent": "All returns are accepted within 30 days.",
        "expectedAggregatedFindingsResult": "VALID",
        // Epoch seconds: a JSON number is read as epoch seconds whatever format the member
        // declares, so the row carries the timestamp in a form that reaches the constraint
        // walk rather than asserting the wire form.
        "lastUpdatedAt": 1_767_225_600_i64,
    })
}

/// The update and the delete of a test case state the member's position and the modelled
/// requirement, and quote nothing of the id they refused.
#[tokio::test]
async fn the_update_and_delete_of_a_test_case_quote_no_test_case_id() {
    let emulator: Emulator = emulator();
    // (case, method, path, body, the id the request carried)
    let rows: Vec<(&str, &str, String, Option<Value>, &str)> = vec![
        (
            "UpdateAutomatedReasoningPolicyTestCase with a lower-case test-case id",
            "PATCH",
            test_case_path("abcdefghijkl"),
            Some(update_body()),
            "abcdefghijkl",
        ),
        (
            // `query="operation"`, `DeleteAutomatedReasoningPolicyTestCase`: DELETE with
            // `lastUpdatedAt` as the `@required` `@httpQuery` parameter `updatedAt`, which
            // this row carries well formed so the label is the only fault.
            "DeleteAutomatedReasoningPolicyTestCase with a test-case id carrying a separator",
            "DELETE",
            format!(
                "{}?updatedAt=2026-01-01T00%3A00%3A00Z",
                test_case_path("ABCDEF-23456")
            ),
            None,
            "ABCDEF-23456",
        ),
    ];

    let expected = format!(
        "1 validation error detected: Value at 'testCaseId' failed to satisfy constraint: {}",
        pattern_requirement(TEST_CASE_ID)
    );
    for (case, method, path, body, carried) in rows {
        let reply = emulator.call(method, &path, body).await;
        let stated = reply.get("message").as_str().unwrap_or_default().to_string();
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(carried) && !stated.contains("Value '"),
            "case {case:?}: the id the request carried must not reach the sentence, got \
             {stated:?}"
        );
    }
}
