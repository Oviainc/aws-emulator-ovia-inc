// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT A `RedriveAllowPolicy` DOCUMENT HAS TO LOOK LIKE, driven over
//! the wire (`POST /` with `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared
//! harness in [`super::support`].
//!
//! The rule under test: **a `RedriveAllowPolicy` is judged against the shape its own
//! documentation gives it when it is WRITTEN — a JSON object, a `redrivePermission` of the
//! three values the member publishes, a `sourceQueueArns` only under `byQueue` and listing
//! at most the 10 ARNs the member admits — and a document breaking any of those rules is
//! refused at HTTP 400 and leaves the queue carrying exactly the permission it had, through
//! both paths that write a queue attribute.**
//!
//! A document the write accepts but the service never understood would leave redrive
//! authorization as whatever the malformed text degrades to, which is why the rules are
//! enforced at the write rather than at the read.
//!
//! The table spans what the rule generalises over rather than one example: the MEMBER at
//! fault (the permission type, the list, the document itself), the FORM of the fault (an
//! unknown value, a list under each permission that takes none, a list one past the
//! published limit, a list of the wrong type, a document that is no JSON object), the
//! permission the faulty document names, and both write paths — with the accepted rows of
//! each permission form carried alongside, so a rule that refused everything would fail the
//! table too.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    source_queue_arns_limit_reason, MAX_SOURCE_QUEUE_ARNS, REDRIVE_ALLOW_POLICY_FORM_REASON,
    REDRIVE_PERMISSION_REASON, SOURCE_QUEUE_ARNS_PERMISSION_REASON,
    SOURCE_QUEUE_ARNS_REQUIRED_REASON,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{all_attributes, call, create, create_ok, emulator, error_code, expected_arn};

/// The attribute the documents under test are written to.
const REDRIVE_ALLOW_POLICY: &str = "RedriveAllowPolicy";

/// The refusal class a refused parameter VALUE carries.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The refusal class a refused attribute VALUE carries, which is what the modelled
/// `InvalidAttributeValue` of `CreateQueue`/`SetQueueAttributes` is named by on the wire.
const INVALID_ATTRIBUTE_VALUE: &str = "InvalidAttributeValue";

/// The published ceiling on the ARNs one permission lists, spelled out here rather than
/// imported, so a wrong constant in the service cannot make a row pass. Source: the
/// `sourceQueueArns` member's own documentation — "You can specify up to 10 source queue
/// ARNs" (`smithy_query(query="doc",shape="com.amazonaws.sqs#SetQueueAttributesRequest",
/// member="Attributes")`).
const PUBLISHED_ARN_LIMIT: usize = 10;

/// The refused-parameter-value template this service answers a refused VALUE in, spelled out
/// rather than imported: the document the request supplied, the parameter it was supplied
/// for, and the one rule that value broke.
fn parameter_refusal(supplied: &str, reason: &str) -> String {
    format!("Value {supplied} for parameter RedriveAllowPolicy is invalid. Reason: {reason}")
}

/// The refused-attribute-value template, which names the attribute and the rule and not the
/// document: the value that was refused is one MEMBER of it, not the whole attribute.
fn attribute_refusal(reason: &str) -> String {
    format!("Invalid value for the parameter RedriveAllowPolicy. Reason: {reason}")
}

/// The `message` member of a refusal, as awsJson renders it.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// `count` source queue ARNs of this region, each naming a different queue.
fn source_arns(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| expected_arn(&format!("rap-source-{index}")))
        .collect()
}

/// Write `document` onto the queue at `url`, answering `(status, body)`.
async fn set_allow_policy(
    router: &MultiServiceRouter,
    url: &str,
    document: &str,
) -> (u16, Value) {
    call(
        router,
        "SetQueueAttributes",
        &json!({"QueueUrl": url, "Attributes": {REDRIVE_ALLOW_POLICY: document}}),
    )
    .await
}

/// One row of the document table.
struct DocumentRow {
    /// What the row is about, named in every assertion it makes.
    case: &'static str,
    /// The document the write submits.
    document: String,
    /// The refusal the document must draw, as `(code, message)`, or `None` for a document
    /// both write paths must accept.
    refusal: Option<(&'static str, String)>,
}

/// Every document rule the attribute publishes, judged the same way by `SetQueueAttributes`
/// and by `CreateQueue`, with the refused document leaving the queue's stored permission
/// untouched and no queue behind.
#[tokio::test]
async fn a_redrive_allow_policy_is_judged_against_the_shape_its_documentation_gives_it() {
    assert_eq!(
        MAX_SOURCE_QUEUE_ARNS, PUBLISHED_ARN_LIMIT,
        "case the published ceiling: expected the documented limit, got {MAX_SOURCE_QUEUE_ARNS}"
    );
    let at_limit = source_arns(PUBLISHED_ARN_LIMIT);
    let past_limit = source_arns(PUBLISHED_ARN_LIMIT + 1);
    let one = source_arns(1);
    let rows: Vec<DocumentRow> = vec![
        // The three permission types the member publishes, each accepted in the form that
        // names it.
        DocumentRow {
            case: "allowAll, the documented default",
            document: json!({"redrivePermission": "allowAll"}).to_string(),
            refusal: None,
        },
        DocumentRow {
            case: "denyAll",
            document: json!({"redrivePermission": "denyAll"}).to_string(),
            refusal: None,
        },
        DocumentRow {
            case: "byQueue listing one source queue",
            document: json!({"redrivePermission": "byQueue", "sourceQueueArns": one}).to_string(),
            refusal: None,
        },
        DocumentRow {
            case: "byQueue listing exactly the published number of ARNs",
            document: json!({"redrivePermission": "byQueue", "sourceQueueArns": at_limit})
                .to_string(),
            refusal: None,
        },
        // The permission type's own value set: a value outside it names no permission the
        // attribute has, and is refused as a refused attribute value.
        DocumentRow {
            case: "a permission type the member does not publish",
            document: json!({"redrivePermission": "allowSome"}).to_string(),
            refusal: Some((
                INVALID_ATTRIBUTE_VALUE,
                attribute_refusal(REDRIVE_PERMISSION_REASON),
            )),
        },
        DocumentRow {
            case: "a permission type spelled in the wrong case",
            document: json!({"redrivePermission": "ALLOWALL"}).to_string(),
            refusal: Some((
                INVALID_ATTRIBUTE_VALUE,
                attribute_refusal(REDRIVE_PERMISSION_REASON),
            )),
        },
        DocumentRow {
            case: "a permission type that is not text at all",
            document: json!({"redrivePermission": true}).to_string(),
            refusal: Some((
                INVALID_ATTRIBUTE_VALUE,
                attribute_refusal(REDRIVE_PERMISSION_REASON),
            )),
        },
        // The coupling of the two members: the list belongs to `byQueue` alone.
        DocumentRow {
            case: "a list under allowAll",
            document: json!({"redrivePermission": "allowAll", "sourceQueueArns": one.clone()})
                .to_string(),
            refusal: Some((
                INVALID_PARAMETER_VALUE,
                parameter_refusal(
                    &json!({"redrivePermission": "allowAll", "sourceQueueArns": one.clone()})
                        .to_string(),
                    SOURCE_QUEUE_ARNS_PERMISSION_REASON,
                ),
            )),
        },
        DocumentRow {
            case: "a list under denyAll",
            document: json!({"redrivePermission": "denyAll", "sourceQueueArns": one.clone()})
                .to_string(),
            refusal: Some((
                INVALID_PARAMETER_VALUE,
                parameter_refusal(
                    &json!({"redrivePermission": "denyAll", "sourceQueueArns": one.clone()})
                        .to_string(),
                    SOURCE_QUEUE_ARNS_PERMISSION_REASON,
                ),
            )),
        },
        DocumentRow {
            case: "a list on a document naming no permission type",
            document: json!({"sourceQueueArns": one.clone()}).to_string(),
            refusal: Some((
                INVALID_PARAMETER_VALUE,
                parameter_refusal(
                    &json!({"sourceQueueArns": one.clone()}).to_string(),
                    SOURCE_QUEUE_ARNS_PERMISSION_REASON,
                ),
            )),
        },
        // The list's arity: the permission that admits exactly the queues it names has to
        // name at least one, and at most the number the member publishes.
        DocumentRow {
            case: "byQueue naming no list at all",
            document: json!({"redrivePermission": "byQueue"}).to_string(),
            refusal: Some((
                INVALID_PARAMETER_VALUE,
                parameter_refusal(
                    &json!({"redrivePermission": "byQueue"}).to_string(),
                    SOURCE_QUEUE_ARNS_REQUIRED_REASON,
                ),
            )),
        },
        DocumentRow {
            case: "byQueue listing nothing",
            document: json!({"redrivePermission": "byQueue", "sourceQueueArns": []}).to_string(),
            refusal: Some((
                INVALID_PARAMETER_VALUE,
                parameter_refusal(
                    &json!({"redrivePermission": "byQueue", "sourceQueueArns": []}).to_string(),
                    SOURCE_QUEUE_ARNS_REQUIRED_REASON,
                ),
            )),
        },
        DocumentRow {
            case: "byQueue listing one ARN past the published limit",
            document: json!({"redrivePermission": "byQueue", "sourceQueueArns": past_limit})
                .to_string(),
            refusal: Some((
                INVALID_PARAMETER_VALUE,
                parameter_refusal(
                    &json!({"redrivePermission": "byQueue", "sourceQueueArns": past_limit})
                        .to_string(),
                    &source_queue_arns_limit_reason(),
                ),
            )),
        },
        // The document's own shape.
        DocumentRow {
            case: "a list whose entries are not ARNs",
            document: json!({"redrivePermission": "byQueue", "sourceQueueArns": [7]}).to_string(),
            refusal: Some((
                INVALID_PARAMETER_VALUE,
                parameter_refusal(
                    &json!({"redrivePermission": "byQueue", "sourceQueueArns": [7]}).to_string(),
                    REDRIVE_ALLOW_POLICY_FORM_REASON,
                ),
            )),
        },
        DocumentRow {
            case: "a list that is not a list",
            document: json!({"redrivePermission": "byQueue", "sourceQueueArns": one[0].clone()})
                .to_string(),
            refusal: Some((
                INVALID_PARAMETER_VALUE,
                parameter_refusal(
                    &json!({"redrivePermission": "byQueue", "sourceQueueArns": one[0].clone()})
                        .to_string(),
                    REDRIVE_ALLOW_POLICY_FORM_REASON,
                ),
            )),
        },
        DocumentRow {
            case: "a document that is JSON but no object",
            document: json!(["allowAll"]).to_string(),
            refusal: Some((
                INVALID_PARAMETER_VALUE,
                parameter_refusal(
                    &json!(["allowAll"]).to_string(),
                    REDRIVE_ALLOW_POLICY_FORM_REASON,
                ),
            )),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let DocumentRow {
            case,
            document,
            refusal,
        } = row;
        let router = emulator();

        // 1. The SetQueueAttributes path, onto a queue already carrying a permission: a
        // refused document must leave THAT permission in force.
        let held = json!({"redrivePermission": "denyAll"}).to_string();
        let url = create_ok(
            &router,
            &format!("rap-{index}"),
            &json!({REDRIVE_ALLOW_POLICY: held.clone()}),
        )
        .await;
        let (status, parsed) = set_allow_policy(&router, &url, document).await;
        let stored = all_attributes(&router, &url)
            .await
            .get(REDRIVE_ALLOW_POLICY)
            .cloned();
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case} on SetQueueAttributes: expected the document accepted, got \
                     {status} with {parsed}"
                );
                assert_eq!(
                    stored.as_deref(),
                    Some(document.as_str()),
                    "case {case}: expected the accepted document reported back, got {stored:?}"
                );
            }
            Some((code, message)) => {
                assert_eq!(
                    (status, error_code(&parsed).as_str(), message_of(&parsed)),
                    (400, *code, message.clone()),
                    "case {case} on SetQueueAttributes: expected the refusal to state its own \
                     rule, got {status} with {parsed}"
                );
                assert_eq!(
                    stored.as_deref(),
                    Some(held.as_str()),
                    "case {case}: a refused document must leave the stored permission \
                     untouched, got {stored:?}"
                );
            }
        }

        // 2. The CreateQueue path, which judges the same document the same way.
        let created = format!("rap-created-{index}");
        let (status, parsed) =
            create(&router, &created, &json!({REDRIVE_ALLOW_POLICY: document})).await;
        match refusal {
            None => assert_eq!(
                status, 200,
                "case {case} on CreateQueue: expected the document accepted, got {status} with \
                 {parsed}"
            ),
            Some((code, message)) => {
                assert_eq!(
                    (status, error_code(&parsed).as_str(), message_of(&parsed)),
                    (400, *code, message.clone()),
                    "case {case} on CreateQueue: expected the refusal to state its own rule, got \
                     {status} with {parsed}"
                );
                let (status, parsed) =
                    call(&router, "GetQueueUrl", &json!({"QueueName": created})).await;
                assert_eq!(
                    (status, error_code(&parsed).as_str()),
                    (400, "QueueDoesNotExist"),
                    "case {case}: a refused create must leave no queue behind, got {status} with \
                     {parsed}"
                );
            }
        }
    }
}
