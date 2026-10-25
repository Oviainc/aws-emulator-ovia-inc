// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `expectedRevisionId` a resource-policy write or delete presents:
//! which of the two conditions the model declares each presented value belongs to.
//!
//! The rule under test: a value the service has never ISSUED as a resource-policy revision id
//! — one in any other form, or one in the right form that no accepted write ever minted — is
//! an invalid PARAMETER, refused `InvalidParameterException`, because no retry of it can ever
//! succeed; a value the service DID issue and a later accepted write has since superseded is
//! an optimistic-lock CONFLICT, refused `OperationAbortedException`, which tells the caller to
//! re-read the policy and retry. The well-formedness question is answered first and is
//! distinct from the comparison against the policy's current revision.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! classification a handler does not actually make cannot pass.
//!
//! The table spans what the rule generalises over rather than one value: a revision id with a
//! suffix, one with a prefix, one that is too short and one too long, one in upper case, one
//! that is not hexadecimal at all, one of the right form that is past every revision the
//! store has minted, and the revisions the store really did mint — the one it is at (accepted)
//! and the ones it has left behind (a conflict). The recorded `<current-revision>-stale` an
//! issue happens to mention is one row of the first class.

use serde_json::{json, Value};

use crate::harness::Logs;

/// The operations that present an `expectedRevisionId`.
const PUT: &str = "PutResourcePolicy";
const DELETE: &str = "DeleteResourcePolicy";

/// The modelled codes the two conditions are refused under.
const INVALID_PARAMETER: &str = "InvalidParameterException";
const CONFLICT: &str = "OperationAbortedException";

/// The whole message a never-issued revision is refused with.
const INVALID_REVISION: &str = "Invalid ExpectedRevisionId";

/// A resource policy document of the form `PutResourcePolicy$policyDocument` documents.
fn document(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "RevisionIds",
            "Effect": "Allow",
            "Principal": {"Service": [principal]},
            "Action": ["logs:PutLogEvents"],
            "Resource": "*",
        }],
    })
    .to_string()
}

/// The `revisionId` a resource-scoped put answers.
fn revision_of(answer: &Value) -> String {
    answer["revisionId"]
        .as_str()
        .unwrap_or_else(|| panic!("a resource-scoped put answers a revisionId, got {answer}"))
        .to_string()
}

/// Every form a presented revision takes, classified by whether the service ever issued it.
#[tokio::test]
async fn a_revision_the_service_never_issued_is_a_parameter_and_not_a_conflict() {
    let logs = Logs::new();
    const GROUP: &str = "/revisions/guarded";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let (_, arn) = logs.group_arns(GROUP).await;

    // Three accepted writes, so the store has issued three revisions: the two earlier ones
    // are superseded and the last one is current.
    let first = revision_of(
        &logs
            .ok(
                PUT,
                &json!({
                    "policyName": "guarded",
                    "policyDocument": document("route53.amazonaws.com"),
                    "resourceArn": arn,
                }),
            )
            .await,
    );
    let second = revision_of(
        &logs
            .ok(
                PUT,
                &json!({
                    "policyName": "guarded",
                    "policyDocument": document("es.amazonaws.com"),
                    "resourceArn": arn,
                    "expectedRevisionId": first,
                }),
            )
            .await,
    );
    let current = revision_of(
        &logs
            .ok(
                PUT,
                &json!({
                    "policyName": "guarded",
                    "policyDocument": document("delivery.logs.amazonaws.com"),
                    "resourceArn": arn,
                    "expectedRevisionId": second,
                }),
            )
            .await,
    );

    // (case, the presented value, the code it is refused under)
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "the current revision with a suffix",
            format!("{current}-stale"),
            INVALID_PARAMETER,
        ),
        (
            "the current revision with a prefix",
            format!("stale-{current}"),
            INVALID_PARAMETER,
        ),
        (
            "one character short of a revision id",
            current.chars().skip(1).collect(),
            INVALID_PARAMETER,
        ),
        (
            "one character longer than a revision id",
            format!("{current}0"),
            INVALID_PARAMETER,
        ),
        (
            // The minted form is lower case, so a value spelled with an upper-case hex digit
            // is in no form this service issues.
            "a revision id with an upper-case hex digit",
            "0000000000000000000000000000000A".to_string(),
            INVALID_PARAMETER,
        ),
        (
            "a value that is not hexadecimal",
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".to_string(),
            INVALID_PARAMETER,
        ),
        (
            "a revision id past every revision the store has minted",
            "ffffffffffffffffffffffffffffffff".to_string(),
            INVALID_PARAMETER,
        ),
        (
            "the revision the first write issued, since superseded",
            first.clone(),
            CONFLICT,
        ),
        (
            "the revision the second write issued, since superseded",
            second.clone(),
            CONFLICT,
        ),
    ];
    for (case, presented, expected) in &cases {
        assert_ne!(
            presented, &current,
            "case {case}: the presented value must not be the live revision"
        );
        // Both operations that present a revision classify it the same way.
        let requests: [(&str, Value); 2] = [
            (
                PUT,
                json!({
                    "policyName": "guarded",
                    "policyDocument": document("vpc-flow-logs.amazonaws.com"),
                    "resourceArn": arn,
                    "expectedRevisionId": presented,
                }),
            ),
            (
                DELETE,
                json!({"resourceArn": arn, "expectedRevisionId": presented}),
            ),
        ];
        for (operation, body) in requests {
            let (code, message) = logs.refusal(operation, &body).await;
            assert_eq!(
                &code, expected,
                "case {case} ({operation}): expected {expected}, got {code} with message \
                 {message:?}"
            );
            if expected == &INVALID_PARAMETER {
                assert_eq!(
                    message, INVALID_REVISION,
                    "case {case} ({operation}): expected {INVALID_REVISION:?}, got {message:?}"
                );
            }
            // A refused write leaves the policy where it was, whichever class it belonged to.
            let page = logs
                .ok(
                    "DescribeResourcePolicies",
                    &json!({"resourceArn": arn, "policyScope": "RESOURCE"}),
                )
                .await;
            let stored = page["resourcePolicies"][0].clone();
            assert_eq!(
                stored["revisionId"].as_str(),
                Some(current.as_str()),
                "case {case} ({operation}): a refused request leaves the policy at its \
                 revision, got {page}"
            );
            assert_eq!(
                stored["policyDocument"].as_str(),
                Some(document("delivery.logs.amazonaws.com").as_str()),
                "case {case} ({operation}): a refused request leaves the document alone, got \
                 {page}"
            );
        }
    }

    // The revision the policy IS at is accepted by both operations, which is what makes the
    // two refusals above a classification rather than a blanket rejection.
    let next = revision_of(
        &logs
            .ok(
                PUT,
                &json!({
                    "policyName": "guarded",
                    "policyDocument": document("appsync.amazonaws.com"),
                    "resourceArn": arn,
                    "expectedRevisionId": current,
                }),
            )
            .await,
    );
    assert_ne!(
        next, current,
        "case the current revision: an accepted write moves the policy to a new revision"
    );
    logs.ok(
        DELETE,
        &json!({"resourceArn": arn, "expectedRevisionId": next}),
    )
    .await;
}
