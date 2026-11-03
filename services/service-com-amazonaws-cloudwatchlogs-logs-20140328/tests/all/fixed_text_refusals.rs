// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal messages CloudWatch Logs states as FIXED TEXT for
//! their error class — text that interpolates nothing from the request or from state — and
//! for the boundaries of that fixed text:
//!
//! * a missing resource the request identifies NOTHING of — a log-group level policy
//!   document addressed by the identifier of the log group it is attached to and by nothing
//!   else — is refused with the service's own missing-resource text, naming neither the log
//!   group, nor a policy name, nor a word for the kind of resource (`DeleteIndexPolicy`,
//!   `GetDataProtectionPolicy` / `DeleteDataProtectionPolicy`);
//! * an `expectedRevisionId` that IS a revision the service issued and a later write
//!   superseded is refused as a conflict against the resource the caller is to re-read and
//!   retry, naming neither the revision presented, nor the stored one, nor the policy's name
//!   or ARN (`PutResourcePolicy`, `DeleteResourcePolicy`); a value the service never issued
//!   as a revision is a bad parameter instead, refused with that class's own fixed text,
//!   because no retry of it can ever succeed.
//!
//! Where a request DOES identify the missing resource the refusal names it — a log group the
//! account does not hold is refused by naming the log group, and the transformer delete
//! names the group it found no transformer on — so the boundary of the fixed text is
//! asserted here too.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! message a handler does not actually answer cannot pass.
//!
//! The tables below span what the rules generalise over: which log-group level resource is
//! missing, which operation refuses it (reader and delete), which identifier form names its
//! group, and how it came to be missing (never held, deleted, deleted twice); and for the
//! revision guard, which operation presents the revision, whether the request also names
//! the policy, and which form the presented revision takes (one the policy really held and a
//! later write superseded, one in the form of a revision that no write ever minted, and a
//! value that is no revision at all). Any single resource or revision an issue happens to
//! mention is one row.

use serde_json::{json, Value};

use crate::harness::Logs;

/// The code CloudWatch Logs answers when the resource an operation names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code CloudWatch Logs answers when concurrent writes to one resource conflict.
const CONFLICT: &str = "OperationAbortedException";

/// The whole message a missing resource the request identifies nothing of is refused with.
const UNIDENTIFIED_NOT_FOUND: &str = "Specified resource does not exist!";

/// The whole message a write or delete presenting a superseded revision is refused with.
const REVISION_CONFLICT: &str =
    "A conflicting operation is currently in progress against this resource. Please try again.";

/// The code CloudWatch Logs answers when a parameter is specified incorrectly, which is what
/// a value the service never issued as a revision id is.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The whole message a write or delete presenting a value that is no revision this service
/// ever issued is refused with.
const INVALID_REVISION: &str = "Invalid ExpectedRevisionId";

/// The whole message a request naming a log group the account does not hold is refused
/// with: the resource it names IS in the message, which is what the fixed text above is
/// the counterpart of.
const LOG_GROUP_NOT_FOUND: &str = "The specified log group does not exist.";

/// One log-group level resource: the put that stores one, the operations that refuse a
/// group holding none, and the word the kind of resource would be named by if the refusal
/// interpolated one.
struct Row {
    /// The case name, and the slug the groups of this row are named under.
    slug: &'static str,
    /// The word for this kind of resource — which a refusal stating the fixed text must NOT
    /// carry.
    resource: &'static str,
    /// The operation that stores one on a log group.
    put: &'static str,
    /// The put's members besides `logGroupIdentifier`.
    document: Value,
    /// The reporting operations that refuse a group holding none.
    readers: &'static [&'static str],
    /// The operation that removes one, which refuses a group holding none too.
    delete: &'static str,
    /// The whole message this row's operations refuse a group holding none of its resource
    /// with, given that group's NAME: the service's fixed text where the request identifies
    /// nothing of the missing resource, and a sentence naming the resource and its group
    /// where it does.
    refusal: fn(&str) -> String,
}

/// The fixed text, for a row whose missing resource the request identifies nothing of.
fn unidentified_refusal(_log_group_name: &str) -> String {
    UNIDENTIFIED_NOT_FOUND.to_string()
}

/// The sentence the transformer delete refuses a group holding no transformer with: the kind
/// of resource that is missing, and the group it was looked for on.
fn transformer_refusal(log_group_name: &str) -> String {
    format!("Transformer does not exist on log group '{log_group_name}'")
}

/// The log-group level resources a request addresses by its log group's identifier alone.
fn rows() -> Vec<Row> {
    vec![
        Row {
            slug: "index-policy",
            resource: "index policy",
            put: "PutIndexPolicy",
            // "Fields" names the fields to index, as `PutIndexPolicy$policyDocument`
            // documents the document's one required field.
            document: json!({"policyDocument": json!({"Fields": ["requestId"]}).to_string()}),
            readers: &[],
            delete: "DeleteIndexPolicy",
            refusal: unidentified_refusal,
        },
        Row {
            slug: "data-protection-policy",
            resource: "data protection policy",
            put: "PutDataProtectionPolicy",
            document: json!({"policyDocument": data_protection_document("EmailAddress")}),
            readers: &["GetDataProtectionPolicy"],
            delete: "DeleteDataProtectionPolicy",
            refusal: unidentified_refusal,
        },
        Row {
            slug: "transformer",
            resource: "transformer",
            put: "PutTransformer",
            // One processor of the kinds the model's `Processor` structure names, which is
            // the shortest pipeline the `Processors` list admits (`@length(min: 1)`).
            document: json!({
                "transformerConfig": [{"parseJSON": {"source": "@message", "destination": "parsed"}}],
            }),
            // `GetTransformer` is NOT a refusing reader of this resource: a group that has
            // had no transformer created reports an empty `transformerConfig`, which
            // `tests/all/transformers.rs` is about.
            readers: &[],
            delete: "DeleteTransformer",
            refusal: transformer_refusal,
        },
    ]
}

/// A data protection policy document of the form `PutDataProtectionPolicy$policyDocument`
/// documents: the two blocks (an Audit action with a findings destination, and a Deidentify
/// action with a mask configuration) over one AWS-managed data identifier. The identifier
/// names come from the catalogue that member links to ("Types of data that you can mask").
fn data_protection_document(identifier_name: &str) -> String {
    let identifiers =
        json!([format!("arn:aws:dataprotection::aws:data-identifier/{identifier_name}")]);
    json!({
        "Name": "fixed-text-refusals",
        "Version": "2021-06-01",
        "Statement": [
            {
                "Sid": "audit-sensitive-data",
                "DataIdentifier": identifiers,
                "Operation": {"Audit": {"FindingsDestination": {}}},
            },
            {
                "Sid": "mask-sensitive-data",
                "DataIdentifier": identifiers,
                "Operation": {"Deidentify": {"MaskConfig": {}}},
            },
        ],
    })
    .to_string()
}

/// The body that stores one of `row`'s resources on the group `identifier` names.
fn put_body(row: &Row, identifier: &str) -> Value {
    let mut body = json!({"logGroupIdentifier": identifier});
    let members = body
        .as_object_mut()
        .expect("the request body is a JSON object");
    for (key, value) in row
        .document
        .as_object()
        .expect("a row's document members are a JSON object")
    {
        members.insert(key.clone(), value.clone());
    }
    body
}

/// Every operation of `row` that answers a group holding none of its resource.
fn refusing_operations(row: &Row) -> Vec<&'static str> {
    let mut operations = row.readers.to_vec();
    operations.push(row.delete);
    operations
}

/// Create one log group and answer the identifier form `use_arn` selects: its name, or the
/// `logGroupArn` the service reports for it (the ARN form the modelled `LogGroupIdentifier`
/// pattern admits).
async fn group_identifier(logs: &Logs, name: &str, use_arn: bool) -> String {
    logs.ok("CreateLogGroup", &json!({"logGroupName": name}))
        .await;
    if use_arn {
        logs.group_arns(name).await.1
    } else {
        name.to_string()
    }
}

/// Assert one operation refuses a group holding none of `row`'s resource with the message
/// that row states — and, where that message is the fixed text, that it carries nothing of
/// the request and no word for the kind of resource.
async fn assert_missing_resource_refusal(
    logs: &Logs,
    case: &str,
    operation: &str,
    identifier: &str,
    log_group_name: &str,
    row: &Row,
) {
    let expected = (row.refusal)(log_group_name);
    let (code, message) = logs
        .refusal(operation, &json!({"logGroupIdentifier": identifier}))
        .await;
    assert_eq!(
        code, NOT_FOUND,
        "case {case:?} ({operation}): a resource that is not there is a not-found, got {code} {message:?}"
    );
    assert_eq!(
        message, expected,
        "case {case:?} ({operation}): expected {expected:?}, got {message:?}"
    );
    if expected == UNIDENTIFIED_NOT_FOUND {
        assert!(
            !message.contains(identifier),
            "case {case:?} ({operation}): the fixed text carries nothing of the request, got {message:?}"
        );
        assert!(
            !message.contains(row.resource),
            "case {case:?} ({operation}): the fixed text names no kind of resource, got {message:?}"
        );
    }
}

/// A log-group level policy document the request identifies nothing of — it is addressed by
/// its log group's identifier alone — is refused with the service's fixed missing-resource
/// text, whichever resource it is, whichever operation asks for it, whichever identifier
/// form names its group, and however it came to be missing; and where the refusal does name
/// the resource and its group (the transformer delete) it does so under the same conditions.
#[tokio::test]
async fn an_unidentified_missing_resource_is_refused_with_the_services_fixed_text() {
    let logs = Logs::new();

    for row in rows() {
        for (form, use_arn) in [("name", false), ("arn", true)] {
            // A group that has never held one of this resource.
            let never = format!("/fixed/{}-never-held-by-{form}", row.slug);
            let identifier = group_identifier(&logs, &never, use_arn).await;
            for operation in refusing_operations(&row) {
                let case = format!("{} never held, by {form}", row.slug);
                assert_missing_resource_refusal(
                    &logs,
                    &case,
                    operation,
                    &identifier,
                    &never,
                    &row,
                )
                .await;
            }

            // A group that held one and no longer does, and the same group asked twice.
            let deleted = format!("/fixed/{}-deleted-by-{form}", row.slug);
            let identifier = group_identifier(&logs, &deleted, use_arn).await;
            logs.ok(row.put, &put_body(&row, &identifier)).await;
            // While it is there the same operations answer it, so the refusals below are
            // computed from the store rather than fixed in the handler.
            for operation in row.readers {
                logs.ok(operation, &json!({"logGroupIdentifier": identifier}))
                    .await;
            }
            logs.ok(row.delete, &json!({"logGroupIdentifier": identifier}))
                .await;
            for operation in refusing_operations(&row) {
                let case = format!("{} deleted, by {form}", row.slug);
                assert_missing_resource_refusal(
                    &logs,
                    &case,
                    operation,
                    &identifier,
                    &deleted,
                    &row,
                )
                .await;
            }
            // Asking a second time refuses identically: the refusal follows the state, not
            // the history of the request.
            for operation in refusing_operations(&row) {
                let case = format!("{} asked twice after the delete, by {form}", row.slug);
                assert_missing_resource_refusal(
                    &logs,
                    &case,
                    operation,
                    &identifier,
                    &deleted,
                    &row,
                )
                .await;
            }

            // A put after the refusals stores the resource again, so nothing above left the
            // group unable to hold one.
            logs.ok(row.put, &put_body(&row, &identifier)).await;
            for operation in row.readers {
                logs.ok(operation, &json!({"logGroupIdentifier": identifier}))
                    .await;
            }
        }
    }
}

/// The boundary of that fixed text: where the request DOES identify the missing resource —
/// a log group the account does not hold — the refusal names it, on every one of the same
/// operations and under either identifier form.
#[tokio::test]
async fn a_missing_log_group_is_still_named_in_the_refusal() {
    let logs = Logs::new();
    const ABSENT: &str = "/fixed/no-such-group";
    let absent_arn = format!(
        "arn:aws:logs:{}:000000000000:log-group:{ABSENT}",
        crate::harness::REGION
    );

    for row in rows() {
        for (form, identifier) in [("name", ABSENT.to_string()), ("arn", absent_arn.clone())] {
            for operation in refusing_operations(&row) {
                let (code, message) = logs
                    .refusal(operation, &json!({"logGroupIdentifier": identifier}))
                    .await;
                assert_eq!(
                    code, NOT_FOUND,
                    "case {} by {form} ({operation}): a group the account does not hold is a not-found, got {code} {message:?}",
                    row.slug
                );
                let expected = missing_group_message(operation, ABSENT);
                assert_eq!(
                    message, expected,
                    "case {} by {form} ({operation}): expected {expected:?}, got {message:?}",
                    row.slug
                );
            }
        }
    }
}

/// The message an operation refuses a log group the account does not hold with: the
/// service's sentence naming the resource kind, except the data protection policy READ,
/// which \u2014 because such a policy is readable across accounts \u2014 names the group that was
/// looked for and the account that would own it.
fn missing_group_message(operation: &str, log_group_name: &str) -> String {
    if operation == "GetDataProtectionPolicy" {
        return format!(
            "Log group '{log_group_name}' for owner '{}' cannot be found",
            engine::emulator_account_id()
        );
    }
    LOG_GROUP_NOT_FOUND.to_string()
}

/// A resource policy document of the form `PutResourcePolicy$policyDocument` documents:
/// one statement allowing a service principal an action the API supports.
fn resource_policy_document(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "FixedTextRefusals",
            "Effect": "Allow",
            "Principal": {"Service": [principal]},
            "Action": ["logs:PutLogEvents"],
            "Resource": "*",
        }],
    })
    .to_string()
}

/// The `revisionId` a resource-scoped `PutResourcePolicy` answers.
fn revision_of(response: &Value) -> String {
    response
        .get("revisionId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a resource-scoped put answers a revisionId, got {response}"))
        .to_string()
}

/// A revision the policy no longer holds is refused as a conflict against the resource, in
/// the error class's own fixed text: on the write and on the delete, whether or not the
/// request also names the policy, and whichever form the stale revision takes.
#[tokio::test]
async fn a_superseded_revision_is_refused_as_a_conflict_against_the_resource() {
    let logs = Logs::new();
    const GROUP: &str = "/fixed/revision-guard";
    const POLICY: &str = "the-attached-policy";

    let arn = group_identifier(&logs, GROUP, true).await;
    let created = logs
        .ok(
            "PutResourcePolicy",
            &json!({
                "policyName": POLICY,
                "policyDocument": resource_policy_document("route53.amazonaws.com"),
                "resourceArn": arn,
            }),
        )
        .await;
    let superseded = revision_of(&created);
    let updated = logs
        .ok(
            "PutResourcePolicy",
            &json!({
                "policyName": POLICY,
                "policyDocument": resource_policy_document("es.amazonaws.com"),
                "resourceArn": arn,
                "expectedRevisionId": superseded,
            }),
        )
        .await;
    let current = revision_of(&updated);
    assert_ne!(
        current, superseded,
        "case the update: an accepted write moves the policy to a new revision, got {current:?}"
    );

    // The forms a revision the policy does not hold takes, each with the class it belongs to:
    // one the policy really held until a later write superseded it is the optimistic-lock
    // CONFLICT, while a value this service never issued — one of the same form that no write
    // ever minted, and a value that is no revision at all — is a bad PARAMETER.
    let stale_revisions = [
        (
            "superseded",
            superseded.clone(),
            CONFLICT,
            REVISION_CONFLICT,
        ),
        (
            "never minted",
            "ffffffffffffffffffffffffffffffff".to_string(),
            INVALID_PARAMETER,
            INVALID_REVISION,
        ),
        (
            "not a revision",
            "not-a-revision".to_string(),
            INVALID_PARAMETER,
            INVALID_REVISION,
        ),
    ];
    // The requests that present a revision: the write, the delete naming the policy, and
    // the delete addressing the resource alone.
    for (stale_case, stale, expected_code, expected_message) in &stale_revisions {
        assert_ne!(
            stale, &current,
            "case {stale_case:?}: a stale row must not be the live revision"
        );
        let requests: [(&str, &str, Value); 3] = [
            (
                "the write",
                "PutResourcePolicy",
                json!({
                    "policyName": POLICY,
                    "policyDocument": resource_policy_document("delivery.logs.amazonaws.com"),
                    "resourceArn": arn,
                    "expectedRevisionId": stale,
                }),
            ),
            (
                "the delete naming the policy",
                "DeleteResourcePolicy",
                json!({"policyName": POLICY, "resourceArn": arn, "expectedRevisionId": stale}),
            ),
            (
                "the delete addressing the resource alone",
                "DeleteResourcePolicy",
                json!({"resourceArn": arn, "expectedRevisionId": stale}),
            ),
        ];
        for (request_case, operation, body) in requests {
            let case = format!("{request_case} at a {stale_case} revision");
            let (code, message) = logs.refusal(operation, &body).await;
            assert_eq!(
                code, *expected_code,
                "case {case:?} ({operation}): expected {expected_code}, got {code} {message:?}"
            );
            assert_eq!(
                message, *expected_message,
                "case {case:?} ({operation}): expected {expected_message:?}, got {message:?}"
            );
            for carried in [stale.as_str(), current.as_str(), POLICY, arn.as_str()] {
                assert!(
                    !message.contains(carried),
                    "case {case:?} ({operation}): the refusal carries nothing of the request or \
                     the stored policy, got {carried:?} in {message:?}"
                );
            }
        }
    }

    // Positive control: the live revision is accepted on the same request the stale rows
    // were refused on, so the guard is the revision and not the shape of the call.
    logs.ok(
        "DeleteResourcePolicy",
        &json!({"resourceArn": arn, "expectedRevisionId": current}),
    )
    .await;
    // And the policy really is gone, so the accepted delete was not a no-op.
    let page = logs
        .ok(
            "DescribeResourcePolicies",
            &json!({"resourceArn": arn, "policyScope": "RESOURCE"}),
        )
        .await;
    let attached = page
        .get("resourcePolicies")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("DescribeResourcePolicies answers resourcePolicies, got {page}"));
    assert!(
        attached.is_empty(),
        "case the accepted delete: the policy is gone, got {page}"
    );
}
