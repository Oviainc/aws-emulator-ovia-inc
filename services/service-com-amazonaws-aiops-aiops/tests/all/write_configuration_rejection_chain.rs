// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the chain of configuration rules a WRITE applies.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: the configuration rules a request that stores an
//! investigation group's configuration has to satisfy are applied in ONE order, and
//! the first rule with something to say is what the request is refused with — so
//! `CreateInvestigationGroup` and `UpdateInvestigationGroup` refuse the same
//! configuration with the same 400 `ValidationException` and the same message, and
//! accept the same configuration. The chain is
//! `state::write_configuration_rejection`; each row below is driven through BOTH
//! writes and their answers compared.
//!
//! An account holds one investigation group per Region, so each row signs for its own
//! Regions: one with an empty store for the create, one already holding a group for
//! the update.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_aiops_aiops::state::{
    constraint_clause, validation_envelope, AWS_OWNED_KEY_TYPE, MAX_COLLECTION_ENTRIES,
};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`): the investigation
/// account, which a cross-account source role may NOT name.
const ACCOUNT: &str = "000000000000";

/// An account OTHER than the investigation account, which a source role may name.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The modelled error shape both writes refuse a bad configuration with
/// (`smithy_query(query="operation",operation="CreateInvestigationGroup")`:
/// `ValidationException`, `@httpError 400`).
const VALIDATION: &str = "ValidationException";

/// A value the modelled `EncryptionConfigurationType` enum does not carry
/// (`smithy_query(query="shape",shape="EncryptionConfigurationType")`), so the
/// encryption rule refuses it.
const UNMODELLED_KEY_TYPE: &str = "CUSTOMER_MANAGED_HSM_KEY";

/// Real AWS Regions the rows sign for, in order; past the list the names are
/// synthetic ones inside what a SigV4 credential scope admits (`[a-zA-Z0-9-]`).
const REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-1",
    "us-east-2",
    "eu-central-1",
    "eu-west-1",
    "eu-west-2",
    "ap-southeast-1",
    "ap-northeast-1",
    "sa-east-1",
    "ca-central-1",
    "ap-south-1",
    "eu-north-1",
    "me-south-1",
    "af-south-1",
    "ap-east-1",
    "us-gov-west-1",
];

/// The Region slot `n` signs for; each row needs two of its own.
fn region(n: usize) -> String {
    REGIONS
        .get(n)
        .map_or_else(|| format!("region-{n:04}"), |name| (*name).to_string())
}

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in `region`, the signal the router routes on and the
/// handlers read the region and the caller principal from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{region}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Percent-encode a value for use as a single `@httpLabel` path segment, which is
/// what an SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

async fn create(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in(router, region, "POST", "/investigationGroups", Some(body)).await
}

async fn update(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "PATCH",
        &format!("/investigationGroups/{}", label(arn)),
        Some(body),
    )
    .await
}

async fn get(router: &MultiServiceRouter, region: &str, arn: &str) -> Value {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/investigationGroups/{}", label(arn)),
        None,
    )
    .await;
    assert_eq!(status, 200, "a read-back must answer 200, got {body:?}");
    body
}

/// Create a group that must succeed in `region`, and return its ARN.
async fn create_ok(router: &MultiServiceRouter, region: &str, body: &Value) -> String {
    let (status, created) = create(router, region, body).await;
    assert_eq!(status, 201, "create {body}: expected 201, got {created:?}");
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {body}: response carried no arn, got {created:?}"))
        .to_string()
}

/// A create body carrying `configuration` beside the two required members.
fn create_body(name: &str, configuration: &Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), Value::String(name.to_string()));
    body.insert("roleArn".to_string(), Value::String(ROLE_ARN.to_string()));
    for (member, value) in configuration
        .as_object()
        .expect("every row's configuration is an object")
    {
        body.insert(member.clone(), value.clone());
    }
    Value::Object(body)
}

/// A `tagKeyBoundaries` list of `entries` entries.
fn tag_key_boundaries(entries: usize) -> Value {
    Value::Array(
        (0..entries)
            .map(|n| Value::String(format!("Boundary{n}")))
            .collect(),
    )
}

/// A `chatbotNotificationChannel` map of `entries` SNS-topic keys, each carrying one
/// chat-application configuration ARN.
fn chatbot_notification_channel(entries: usize) -> Value {
    Value::Object(
        (0..entries)
            .map(|n| {
                (
                    format!("arn:aws:sns:us-west-2:{ACCOUNT}:aiops-alerts-{n}"),
                    json!([format!(
                        "arn:aws:chatbot::{ACCOUNT}:chat-configuration/slack-channel/ops-{n}"
                    )]),
                )
            })
            .collect(),
    )
}

/// One cross-account configuration naming `account`.
fn cross_account_configurations(account: &str) -> Value {
    json!([{ "sourceRoleArn": source_role_arn(account) }])
}

/// The refusal a row must draw, as the service words that rule.
enum Refusal {
    /// A rule the clause grammar states: the member path the clause names.
    Clause(&'static str),
    /// A rule the service words as its own sentence: the whole message.
    Sentence(String),
}

impl Refusal {
    /// How the message must OPEN — the whole message for a service sentence, and the
    /// envelope plus the clause subject for a clause, whose requirement half each rule
    /// states for itself.
    fn prefix(&self) -> String {
        match self {
            Refusal::Clause(member) => validation_envelope(&[constraint_clause(member, "")]),
            Refusal::Sentence(message) => message.clone(),
        }
    }
}

/// The sentence a source role that does not name another account is refused with: the
/// service's own wording for that error class, which names the investigation account and
/// echoes the ARN the request sent.
fn source_role_sentence(source_role_arn: &str) -> String {
    format!(
        "Source role ARN cannot belong to the investigation account {ACCOUNT}. \
         Found ARN: {source_role_arn}"
    )
}

/// The `sourceRoleArn` [`cross_account_configurations`] builds for `account`.
fn source_role_arn(account: &str) -> String {
    format!("arn:aws:iam::{account}:role/AIOpsCrossAccountRole")
}

/// A row: a configuration both writes accept, and the refusal it must draw — `None`
/// when the whole chain accepts it.
struct Row {
    case: &'static str,
    configuration: Value,
    refused: Option<Refusal>,
}

/// The table. The rule is "one chain, one order, both writes", so the rows differ
/// along WHICH rule of the chain the configuration violates, along the member and the
/// shape of the value that violates it (an enum value, an ARN's account, a list
/// length, a map size), and along HOW MANY rules are violated at once — which is what
/// pins the order the rules are applied in. The last row violates none of them, so it
/// pins that the shared chain still lets a good configuration through.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an encryption type outside the modelled value set",
            configuration: json!({ "encryptionConfiguration": { "type": UNMODELLED_KEY_TYPE } }),
            refused: Some(Refusal::Clause("encryptionConfiguration.type")),
        },
        Row {
            case: "a source role naming the investigation account itself",
            configuration: json!({
                "crossAccountConfigurations": cross_account_configurations(ACCOUNT),
            }),
            refused: Some(Refusal::Sentence(source_role_sentence(&source_role_arn(
                ACCOUNT,
            )))),
        },
        Row {
            case: "tagKeyBoundaries one entry past the bound",
            configuration: json!({
                "tagKeyBoundaries": tag_key_boundaries(MAX_COLLECTION_ENTRIES + 1),
            }),
            refused: Some(Refusal::Clause("tagKeyBoundaries")),
        },
        Row {
            case: "chatbotNotificationChannel far past the bound",
            configuration: json!({
                "chatbotNotificationChannel":
                    chatbot_notification_channel(MAX_COLLECTION_ENTRIES * 3),
            }),
            refused: Some(Refusal::Clause("chatbotNotificationChannel")),
        },
        // Every rule of the chain violated at once: the FIRST one reports.
        Row {
            case: "every rule violated at once",
            configuration: json!({
                "encryptionConfiguration": { "type": UNMODELLED_KEY_TYPE },
                "crossAccountConfigurations": cross_account_configurations(ACCOUNT),
                "tagKeyBoundaries": tag_key_boundaries(MAX_COLLECTION_ENTRIES + 1),
                "chatbotNotificationChannel":
                    chatbot_notification_channel(MAX_COLLECTION_ENTRIES + 1),
            }),
            refused: Some(Refusal::Clause("encryptionConfiguration.type")),
        },
        // The encryption rule accepts, so the next one in the chain reports.
        Row {
            case: "a same-account source role beside oversized collections",
            configuration: json!({
                "encryptionConfiguration": { "type": AWS_OWNED_KEY_TYPE },
                "crossAccountConfigurations": cross_account_configurations(ACCOUNT),
                "tagKeyBoundaries": tag_key_boundaries(MAX_COLLECTION_ENTRIES + 2),
            }),
            refused: Some(Refusal::Sentence(source_role_sentence(&source_role_arn(
                ACCOUNT,
            )))),
        },
        // Both collections oversized: the one the bounds rule judges first reports.
        Row {
            case: "both unbounded collections past the bound",
            configuration: json!({
                "tagKeyBoundaries": tag_key_boundaries(MAX_COLLECTION_ENTRIES + 1),
                "chatbotNotificationChannel":
                    chatbot_notification_channel(MAX_COLLECTION_ENTRIES + 1),
            }),
            refused: Some(Refusal::Clause("tagKeyBoundaries")),
        },
        // Nothing violated, with both collections AT the bound: accepted by both
        // writes, and stored.
        Row {
            case: "a configuration at every bound",
            configuration: json!({
                "encryptionConfiguration": { "type": AWS_OWNED_KEY_TYPE },
                "crossAccountConfigurations": cross_account_configurations(FOREIGN_ACCOUNT),
                "tagKeyBoundaries": tag_key_boundaries(MAX_COLLECTION_ENTRIES),
                "chatbotNotificationChannel":
                    chatbot_notification_channel(MAX_COLLECTION_ENTRIES),
            }),
            refused: None,
        },
    ]
}

/// The members a write may replace, which a refused write must leave exactly as they
/// were.
const CONFIGURATION_MEMBERS: &[&str] = &[
    "encryptionConfiguration",
    "crossAccountConfigurations",
    "tagKeyBoundaries",
    "chatbotNotificationChannel",
    "roleArn",
    "isCloudTrailEventHistoryEnabled",
    "lastModifiedAt",
];

#[tokio::test]
async fn both_writes_apply_one_configuration_rejection_chain() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        // Two Regions per row: one with an empty store for the create, one already
        // holding a group for the update.
        let create_region = region(2 * index);
        let update_region = region(2 * index + 1);

        let arn = create_ok(
            &router,
            &update_region,
            &json!({ "name": format!("chain-target-{index}"), "roleArn": ROLE_ARN }),
        )
        .await;
        let before = get(&router, &update_region, &arn).await;

        let (create_status, created) = create(
            &router,
            &create_region,
            &create_body(&format!("chain-source-{index}"), &row.configuration),
        )
        .await;
        let (update_status, updated) =
            update(&router, &update_region, &arn, &row.configuration).await;

        match &row.refused {
            Some(refusal) => {
                // A clause's envelope and subject are read off the builders the service
                // words a clause through; a service sentence is stated in full.
                let expected_prefix = refusal.prefix();

                for (operation, status, body) in [
                    ("CreateInvestigationGroup", create_status, &created),
                    ("UpdateInvestigationGroup", update_status, &updated),
                ] {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected {operation} to refuse with 400, \
                         got {status} / {body:?}"
                    );
                    assert_eq!(
                        error_code(body),
                        VALIDATION,
                        "case {case:?}: expected {operation} to report {VALIDATION}, \
                         got {body:?}"
                    );
                    assert!(
                        message(body).starts_with(&expected_prefix),
                        "case {case:?}: expected {operation} to open its refusal with \
                         {expected_prefix:?}, got {:?}",
                        message(body)
                    );
                }

                // The two writes refuse it IDENTICALLY, which is what one shared chain
                // buys: same rule reported, same sentence.
                assert_eq!(
                    message(&created),
                    message(&updated),
                    "case {case:?}: the two writes must word one refusal the same way, \
                     got {:?} and {:?}",
                    message(&created),
                    message(&updated)
                );

                // A refused write stores nothing: the group the update addressed is
                // exactly as it was, stamp included.
                let after = get(&router, &update_region, &arn).await;
                for member in CONFIGURATION_MEMBERS {
                    assert_eq!(
                        after.get(member),
                        before.get(member),
                        "case {case:?}: a refused update must leave {member} alone, \
                         was {:?} now {:?}",
                        before.get(member),
                        after.get(member)
                    );
                }
            }
            None => {
                assert_eq!(
                    create_status, 201,
                    "case {case:?}: expected the create to be accepted, \
                     got {create_status} / {created:?}"
                );
                assert_eq!(
                    update_status, 200,
                    "case {case:?}: expected the update to be accepted, \
                     got {update_status} / {updated:?}"
                );

                // Accepted means STORED: the update's configuration is what a later
                // read reports.
                let after = get(&router, &update_region, &arn).await;
                let configuration = row
                    .configuration
                    .as_object()
                    .expect("every row's configuration is an object");
                for (member, value) in configuration {
                    assert_eq!(
                        after.get(member.as_str()),
                        Some(value),
                        "case {case:?}: expected the read-back to report {member} as \
                         {value:?}, got {:?}",
                        after.get(member.as_str())
                    );
                }
            }
        }
    }
}
