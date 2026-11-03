// SPDX-License-Identifier: CC0-1.0

//! An event bus's mutable CONFIGURATION and its resource POLICY, end to end over
//! the wire (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`),
//! dispatched through the real `MultiServiceRouter` — so every case exercises
//! routing, deserialization, the modelled-constraint validator, the handler, the
//! persisted store and serialization: the same path a client takes.
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded
//! shape:
//!
//! * every configuration member `UpdateEventBus` accepts is written to the bus
//!   record and reported back by that call AND by `DescribeEventBus`, so a
//!   read-after-write agrees with the write; a member a request does not carry
//!   keeps the value it had, and one it does carry replaces it;
//! * a key identifier is reported as the ARN of the KMS resource it names,
//!   whichever of the four spellings the request used;
//! * a request no bus configuration can be built from is refused with the error
//!   for WHY, and the stored bus is left exactly as it was;
//! * a granted permission is a statement of the bus's resource policy, keyed by
//!   its `StatementId`, and the policy `DescribeEventBus` reports is the IAM
//!   document those statements make; a policy supplied whole is reported back as
//!   it was installed;
//! * `RemovePermission` revokes one grant by id or every grant at once;
//! * every stored bus is listed once, in name order, behind an opaque token that
//!   resumes where the previous page ended;
//! * a deleted bus is gone from every reader, and the account's default bus
//!   cannot be deleted.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    ComAmazonawsEventbridgeAwseventsState, DEFAULT_EVENT_BUS_NAME,
};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for. Deliberately NOT the store's
/// fallback region, so an ARN that ignored the request's credential scope would
/// show up as a wrong region segment rather than passing by accident.
const REGION: &str = "eu-west-2";

/// The members `UpdateEventBus` both accepts and reports back, from
/// `smithy_query(query="operation", operation="UpdateEventBus")`: the response
/// carries `Name` and `Arn` plus exactly these.
const CONFIGURATION_MEMBERS: [&str; 4] = [
    "Description",
    "KmsKeyIdentifier",
    "DeadLetterConfig",
    "LogConfig",
];

/// `IncludeDetail`'s values, from `smithy_query(query="shape",
/// shape="IncludeDetail")`. Spelled out here, not imported, so a wrong set in the
/// implementation cannot make a test pass.
const INCLUDE_DETAIL_VALUES: [&str; 2] = ["FULL", "NONE"];

/// `Level`'s values, from `smithy_query(query="shape", shape="Level")`.
const LEVEL_VALUES: [&str; 4] = ["ERROR", "INFO", "OFF", "TRACE"];

/// `EventBusDescription`'s modelled `@length(min: 0, max: 512)`, from
/// `smithy_query(query="operation", operation="UpdateEventBus")`.
const DESCRIPTION_MAX: usize = 512;

/// The IAM policy language version an assembled bus policy carries.
const POLICY_VERSION: &str = "2012-10-17";

/// "The permission policy on the event bus cannot exceed 10 KB in size"
/// (`PutPermission`'s own documentation).
const MAX_POLICY_BYTES: usize = 10 * 1024;

/// `StatementId`'s modelled `@length(min: 1, max: 64)`, from
/// `smithy_query(query="shape", shape="StatementId")`.
const STATEMENT_ID_MAX: usize = 64;

/// The emulator: one router carrying only this service over one shared engine,
/// plus that engine, so a test can read the store the handlers wrote.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// This emulator's account id — the account segment of every ARN it mints.
fn account() -> &'static str {
    engine::emulator_account_id()
}

async fn call(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, \
         SignedHeaders=host, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "events.eu-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace; the empty string on a
/// success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// `arn:aws:events:<region>:<account>:event-bus/<name>` — built here rather than
/// read from the implementation.
fn event_bus_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:event-bus/{name}", account())
}

/// An SQS queue ARN, for a bus-level dead-letter queue.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{}:{name}", account())
}

/// The identifier a bus reports its customer managed key under: the ARN of the
/// KMS resource the request's identifier names.
///
/// `KmsKeyIdentifier` admits four spellings of one key — "the key Amazon
/// Resource Name (ARN), KeyId, key alias, or key alias ARN"
/// (`smithy_query(query="doc", shape="UpdateEventBusRequest",
/// member="KmsKeyIdentifier")`) — and EventBridge reports the key it stored as an
/// ARN, so a spelling that is not already one is completed into an ARN in the
/// caller's region and account. Written here independently of the
/// implementation.
fn expected_kms_identifier(requested: &str) -> Value {
    if requested.is_empty() || requested.starts_with("arn:") {
        return json!(requested);
    }
    let resource = if requested.starts_with("alias/") {
        requested.to_string()
    } else {
        format!("key/{requested}")
    };
    json!(format!("arn:aws:kms:{REGION}:{}:{resource}", account()))
}

/// A bus the account holds, created with no configuration at all.
async fn create_bus(emulator: &Emulator, name: &str) {
    let (status, body) = call(emulator, "CreateEventBus", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "CreateEventBus {name} answered {status} {body}"
    );
}

/// The members of `reported` that carry a bus's configuration, compared against
/// `expected` — a member nothing has set must be ABSENT, never a default.
fn assert_configuration(case: &str, source: &str, expected: &Map<String, Value>, reported: &Value) {
    for member in CONFIGURATION_MEMBERS {
        let want = expected.get(member).cloned().unwrap_or(Value::Null);
        let actual = reported.get(member).cloned().unwrap_or(Value::Null);
        assert_eq!(
            actual, want,
            "case {case}: {source} must report {member} {want}, got {actual} in {reported}"
        );
    }
}

/// `DescribeEventBus` for `name`, which must succeed.
async fn describe(emulator: &Emulator, case: &str, name: &str) -> Value {
    let (status, body) = call(emulator, "DescribeEventBus", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case}: DescribeEventBus {name} answered {status} {body}"
    );
    body
}

/// The `Policy` of `name`, parsed; `None` when the bus reports none.
async fn policy_of(emulator: &Emulator, case: &str, name: &str) -> Option<Value> {
    let described = describe(emulator, case, name).await;
    let text = described.get("Policy")?.as_str().map(str::to_string)?;
    let parsed = serde_json::from_str::<Value>(&text).unwrap_or_else(|error| {
        panic!("case {case}: Policy is not a JSON document ({error}): {text:?}")
    });
    assert_eq!(
        parsed["Version"],
        json!(POLICY_VERSION),
        "case {case}: expected policy Version {POLICY_VERSION}, got {parsed}"
    );
    Some(parsed)
}

/// The statements of a parsed policy document.
fn statements(case: &str, policy: &Value) -> Vec<Value> {
    policy["Statement"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Statement list, got {policy}"))
        .clone()
}

/// Every configuration member `UpdateEventBus` accepts is written to the bus and
/// reported back, by the update itself and by a later read. The rows are applied
/// to ONE bus in sequence, so each also proves what the previous rows left
/// alone: a member a request carries replaces what was stored, and one it does
/// not carry keeps it. They differ along which members are present, the value of
/// each (a description at both modelled bounds, each spelling of a key
/// identifier, a DLQ, every value of each `LogConfig` enum) and how many arrive
/// at once.
#[tokio::test]
async fn an_update_writes_every_configuration_member_and_a_read_reports_it() {
    let emulator = emulator();
    let bus = "configured";
    create_bus(&emulator, bus).await;

    let described = describe(&emulator, "a bus nothing has configured", bus).await;
    let creation_time = described["CreationTime"].clone();
    assert_configuration(
        "a bus nothing has configured",
        "DescribeEventBus",
        &Map::new(),
        &described,
    );

    let mut updates = vec![
        json!({ "Description": "the first description" }),
        json!({ "LogConfig": { "IncludeDetail": "FULL", "Level": "TRACE" } }),
        // The same member again, with a different value: an update overwrites.
        json!({ "Description": "the second description" }),
        json!({ "DeadLetterConfig": { "Arn": queue_arn("bus-dlq") } }),
        // Each spelling of a key identifier.
        json!({ "KmsKeyIdentifier": "1234abcd-12ab-34cd-56ef-1234567890ab" }),
        json!({ "KmsKeyIdentifier": "alias/events-bus-key" }),
        json!({
            "KmsKeyIdentifier":
                format!("arn:aws:kms:{REGION}:{}:key/0f0e0d0c-0b0a-0908-0706-050403020100", account()),
        }),
        json!({ "KmsKeyIdentifier": format!("arn:aws:kms:{REGION}:{}:alias/other", account()) }),
        // Both modelled bounds of `Description`.
        json!({ "Description": "" }),
        json!({ "Description": "d".repeat(DESCRIPTION_MAX) }),
        // Several members at once, which must all land.
        json!({
            "Description": "a combined update",
            "DeadLetterConfig": { "Arn": queue_arn("spill") },
            "LogConfig": { "IncludeDetail": "NONE", "Level": "INFO" },
        }),
    ];
    // One row per published value of each `LogConfig` enum, so no admitted value
    // is refused and none is quietly replaced by a default.
    for include_detail in INCLUDE_DETAIL_VALUES {
        for level in LEVEL_VALUES {
            updates.push(json!({
                "LogConfig": { "IncludeDetail": include_detail, "Level": level },
            }));
        }
    }

    // What the bus is expected to hold, built up from the rows applied so far —
    // never read back out of a response.
    let mut expected = Map::new();
    for update in &updates {
        let case = update.to_string();
        let mut request = update.clone();
        request["Name"] = json!(bus);
        for member in CONFIGURATION_MEMBERS {
            let Some(requested) = update.get(member) else {
                continue;
            };
            let value = match (member, requested.as_str()) {
                ("KmsKeyIdentifier", Some(identifier)) => expected_kms_identifier(identifier),
                _ => requested.clone(),
            };
            expected.insert(member.to_string(), value);
        }

        let (status, updated) = call(&emulator, "UpdateEventBus", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateEventBus answered {status} {updated}"
        );
        assert_eq!(
            updated["Name"],
            json!(bus),
            "case {case}: expected Name {bus}, got {updated}"
        );
        assert_eq!(
            updated["Arn"],
            json!(event_bus_arn(bus)),
            "case {case}: expected Arn {}, got {updated}",
            event_bus_arn(bus)
        );
        assert_configuration(&case, "UpdateEventBus", &expected, &updated);

        // The read-after-write: the same record, member for member.
        let described = describe(&emulator, &case, bus).await;
        assert_configuration(&case, "DescribeEventBus", &expected, &described);
        assert_eq!(
            described["CreationTime"], creation_time,
            "case {case}: an update must not move CreationTime, got {described}"
        );
        let modified = described["LastModifiedTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case}: expected a LastModifiedTime instant, got {described}")
        });
        let created = creation_time
            .as_f64()
            .expect("CreationTime is an instant on the wire");
        assert!(
            modified >= created,
            "case {case}: expected LastModifiedTime {modified} at or after CreationTime {created}"
        );

        // A read repeated reports exactly what it reported the first time.
        let again = describe(&emulator, &case, bus).await;
        assert_eq!(
            again, described,
            "case {case}: a read must not change what a later caller sees"
        );
    }
}

/// A request naming no bus configures the account's `default` bus — "the name of
/// the event bus" is not required — and leaves every other bus alone.
#[tokio::test]
async fn an_update_naming_no_bus_configures_the_account_default_bus() {
    let emulator = emulator();
    create_bus(&emulator, "untouched").await;
    let before = describe(&emulator, "another bus", "untouched").await;

    let (status, updated) = call(
        &emulator,
        "UpdateEventBus",
        &json!({ "Description": "the default bus, configured" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateEventBus with no Name answered {status} {updated}"
    );
    assert_eq!(
        updated["Name"],
        json!(DEFAULT_EVENT_BUS_NAME),
        "a request naming no bus updates the account's default bus, got {updated}"
    );

    let (status, described) = call(&emulator, "DescribeEventBus", &json!({})).await;
    assert_eq!(
        status, 200,
        "DescribeEventBus with no Name answered {status} {described}"
    );
    assert_eq!(
        described["Description"],
        json!("the default bus, configured"),
        "the default bus must report the description just written, got {described}"
    );
    assert_eq!(
        describe(&emulator, "another bus", "untouched").await,
        before,
        "configuring one bus must leave every other bus exactly as it was"
    );
}

/// A request no bus configuration can be built from is refused with the error for
/// WHY — never a 2xx — and the stored bus is left exactly as it was. The rows
/// differ along which rule the request breaks: the bus must exist, each modelled
/// bound on `Name`, `Description` and `KmsKeyIdentifier`, and each `LogConfig`
/// enum.
#[tokio::test]
async fn an_update_no_configuration_can_be_built_from_is_refused() {
    let emulator = emulator();
    let bus = "stable";
    create_bus(&emulator, bus).await;
    let (status, configured) = call(
        &emulator,
        "UpdateEventBus",
        &json!({
            "Name": bus,
            "Description": "the stored description",
            "LogConfig": { "IncludeDetail": "FULL", "Level": "INFO" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first update answered {status} {configured}"
    );
    let before = describe(&emulator, "the configured bus", bus).await;

    let cases = [
        (
            "a bus that was never created",
            json!({ "Name": "never-created", "Description": "no bus to write to" }),
            "ResourceNotFoundException",
        ),
        (
            "a LogConfig.IncludeDetail the enum does not admit",
            json!({ "Name": bus, "LogConfig": { "IncludeDetail": "PARTIAL" } }),
            "ValidationException",
        ),
        (
            "a LogConfig.Level the enum does not admit",
            json!({ "Name": bus, "LogConfig": { "Level": "VERBOSE" } }),
            "ValidationException",
        ),
        (
            "a description past the modelled maximum",
            json!({ "Name": bus, "Description": "d".repeat(DESCRIPTION_MAX + 1) }),
            "ValidationException",
        ),
        (
            "a KMS key identifier the modelled pattern does not admit",
            json!({ "Name": bus, "KmsKeyIdentifier": "alias/not valid!" }),
            "ValidationException",
        ),
        (
            "a name the modelled pattern does not admit",
            json!({ "Name": "dollar$sign", "Description": "no such bus" }),
            "ValidationException",
        ),
        (
            "a name past the modelled maximum",
            json!({ "Name": "b".repeat(257), "Description": "no such bus" }),
            "ValidationException",
        ),
    ];

    for (case, request, expected_code) in &cases {
        let (status, body) = call(&emulator, "UpdateEventBus", request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            &error_code(&body),
            expected_code,
            "case {case}: expected {expected_code}, got {body}"
        );
        let after = describe(&emulator, case, bus).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update must leave the stored bus unchanged"
        );
    }
}

/// A granted permission is a statement of the bus's resource policy: the policy
/// `DescribeEventBus` reports is the IAM document the grants installed so far
/// make, each statement carrying the grant it was given. The rows differ along
/// the principal (one account, every account), the length of the statement id,
/// whether the grant carries a condition, and whether the statement id is new or
/// already installed.
#[tokio::test]
async fn a_granted_permission_is_reported_in_the_bus_policy() {
    let emulator = emulator();
    let bus = "granted";
    create_bus(&emulator, bus).await;

    // A bus nothing has granted a permission on has no policy at all.
    let described = describe(&emulator, "a bus with no grant", bus).await;
    assert_eq!(
        described.get("Policy").cloned().unwrap_or(Value::Null),
        Value::Null,
        "a bus nothing has granted on must report no Policy, got {described}"
    );

    let grants = [
        (
            "one account",
            json!({
                "StatementId": "CrossAccount",
                "Action": "events:PutEvents",
                "Principal": "111122223333",
            }),
        ),
        (
            "every account",
            json!({
                "StatementId": "EveryAccount",
                "Action": "events:PutEvents",
                "Principal": "*",
            }),
        ),
        (
            "every account in one organization",
            json!({
                "StatementId": "OneOrganization",
                "Action": "events:PutEvents",
                "Principal": "*",
                "Condition": {
                    "Type": "StringEquals",
                    "Key": "aws:PrincipalOrgID",
                    "Value": "o-1234567890",
                },
            }),
        ),
        (
            "a statement id at the modelled bound",
            json!({
                "StatementId": "s".repeat(STATEMENT_ID_MAX),
                "Action": "events:PutEvents",
                "Principal": "210987654321",
            }),
        ),
        // The SAME statement id as the first row, with a different grant: a
        // statement id identifies the grant, so this replaces that statement
        // rather than adding one.
        (
            "a statement id already installed",
            json!({
                "StatementId": "CrossAccount",
                "Action": "events:PutEvents",
                "Principal": "999988887777",
            }),
        ),
    ];

    let mut installed: Vec<String> = Vec::new();
    for (case, grant) in &grants {
        let mut request = grant.clone();
        request["EventBusName"] = json!(bus);
        let (status, body) = call(&emulator, "PutPermission", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: PutPermission answered {status} {body}"
        );

        let sid = grant["StatementId"]
            .as_str()
            .expect("the row names the statement")
            .to_string();
        if !installed.contains(&sid) {
            installed.push(sid.clone());
        }

        let policy = policy_of(&emulator, case, bus)
            .await
            .unwrap_or_else(|| panic!("case {case}: expected a Policy after the grant"));
        let held = statements(case, &policy);
        assert_eq!(
            held.len(),
            installed.len(),
            "case {case}: expected {} statement(s), got {policy}",
            installed.len()
        );
        let sids: Vec<&str> = held
            .iter()
            .map(|statement| statement["Sid"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            sids,
            installed.iter().map(String::as_str).collect::<Vec<_>>(),
            "case {case}: expected the statements in the order they were installed, got {policy}"
        );

        let statement = held
            .iter()
            .find(|statement| statement["Sid"] == json!(sid))
            .unwrap_or_else(|| panic!("case {case}: expected a statement {sid}, got {policy}"));
        assert_eq!(
            statement["Effect"],
            json!("Allow"),
            "case {case}: a grant allows, got {statement}"
        );
        // The policy language writes "every principal" as the bare string "*"
        // and a NAMED principal as an object keyed by the kind of principal it
        // is, so an account id is an Amazon Web Services principal under `AWS`.
        let expected_principal = if grant["Principal"] == json!("*") {
            json!("*")
        } else {
            json!({ "AWS": grant["Principal"].clone() })
        };
        assert_eq!(
            statement["Principal"], expected_principal,
            "case {case}: expected the granted principal, got {statement}"
        );
        assert_eq!(
            statement["Action"], grant["Action"],
            "case {case}: expected the granted action, got {statement}"
        );
        assert_eq!(
            statement["Resource"],
            json!(event_bus_arn(bus)),
            "case {case}: expected the bus ARN as the Resource, got {statement}"
        );
        let expected_condition = match grant.get("Condition") {
            Some(condition) => {
                // The IAM condition block a `Condition` renders as:
                // `{<Type>: {<Key>: <Value>}}`.
                let mut inner = Map::new();
                inner.insert(
                    condition["Key"].as_str().unwrap_or_default().to_string(),
                    condition["Value"].clone(),
                );
                let mut outer = Map::new();
                outer.insert(
                    condition["Type"].as_str().unwrap_or_default().to_string(),
                    Value::Object(inner),
                );
                Value::Object(outer)
            }
            None => Value::Null,
        };
        assert_eq!(
            statement.get("Condition").cloned().unwrap_or(Value::Null),
            expected_condition,
            "case {case}: expected Condition {expected_condition}, got {statement}"
        );
    }

    // One grant revoked by its id leaves every other statement installed.
    let revoked = installed.remove(0);
    let (status, body) = call(
        &emulator,
        "RemovePermission",
        &json!({ "EventBusName": bus, "StatementId": revoked }),
    )
    .await;
    assert_eq!(
        status, 200,
        "RemovePermission {revoked} answered {status} {body}"
    );
    let policy = policy_of(&emulator, "after one revocation", bus)
        .await
        .expect("statements remain, so the bus still has a policy");
    let sids: Vec<String> = statements("after one revocation", &policy)
        .iter()
        .map(|statement| statement["Sid"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        sids, installed,
        "expected exactly the grants that were not revoked, got {policy}"
    );

    // Every grant at once: the bus is back to having no policy at all.
    let (status, body) = call(
        &emulator,
        "RemovePermission",
        &json!({ "EventBusName": bus, "RemoveAllPermissions": true }),
    )
    .await;
    assert_eq!(
        status, 200,
        "RemovePermission --remove-all-permissions answered {status} {body}"
    );
    let described = describe(&emulator, "after every revocation", bus).await;
    assert_eq!(
        described.get("Policy").cloned().unwrap_or(Value::Null),
        Value::Null,
        "a bus whose every grant was revoked must report no Policy, got {described}"
    );
}

/// A policy supplied whole is installed as the bus's policy and reported back as
/// it was installed, byte for byte — a read-modify-write client gets back the
/// document it wrote. The rows differ along the document's form: compact, laid
/// out over several lines, one statement as an object rather than a list, and
/// several statements.
#[tokio::test]
async fn a_policy_supplied_whole_is_reported_back_as_it_was_installed() {
    let emulator = emulator();
    let bus = "policy-document";
    create_bus(&emulator, bus).await;
    let arn = event_bus_arn(bus);

    let documents = [
        (
            "one statement, compact",
            format!(
                "{{\"Version\":\"{POLICY_VERSION}\",\"Statement\":[{{\"Sid\":\"One\",\
                 \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"111122223333\"}},\
                 \"Action\":\"events:PutEvents\",\"Resource\":\"{arn}\"}}]}}"
            ),
            1,
        ),
        (
            "one statement, laid out over several lines",
            format!(
                "{{\n  \"Version\": \"{POLICY_VERSION}\",\n  \"Statement\": [\n    {{\n\
                 \t\"Sid\": \"Indented\",\n\t\"Effect\": \"Allow\",\n\
                 \t\"Principal\": {{ \"AWS\": \"210987654321\" }},\n\
                 \t\"Action\": \"events:PutEvents\",\n\t\"Resource\": \"{arn}\"\n    }}\n  ]\n}}"
            ),
            1,
        ),
        (
            "one statement as an object rather than a list",
            format!(
                "{{\"Version\":\"{POLICY_VERSION}\",\"Statement\":{{\"Sid\":\"Lonely\",\
                 \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"*\"}},\
                 \"Action\":\"events:PutEvents\",\"Resource\":\"{arn}\"}}}}"
            ),
            1,
        ),
        (
            "two statements",
            format!(
                "{{\"Version\":\"{POLICY_VERSION}\",\"Statement\":[{{\"Sid\":\"First\",\
                 \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"111122223333\"}},\
                 \"Action\":\"events:PutEvents\",\"Resource\":\"{arn}\"}},\
                 {{\"Sid\":\"Second\",\"Effect\":\"Deny\",\"Principal\":{{\"AWS\":\"*\"}},\
                 \"Action\":\"events:PutRule\",\"Resource\":\"{arn}\"}}]}}"
            ),
            2,
        ),
    ];

    for (case, document, statement_count) in &documents {
        let (status, body) = call(
            &emulator,
            "PutPermission",
            &json!({ "EventBusName": bus, "Policy": document }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: PutPermission answered {status} {body}"
        );
        let described = describe(&emulator, case, bus).await;
        assert_eq!(
            described["Policy"],
            json!(document),
            "case {case}: expected the document as installed, got {described}"
        );

        // A grant on top of a supplied document adds one statement to it, and
        // the whole policy is still one document.
        let (status, body) = call(
            &emulator,
            "PutPermission",
            &json!({
                "EventBusName": bus,
                "StatementId": "AddedAfterTheDocument",
                "Action": "events:PutEvents",
                "Principal": "444455556666",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: a grant after the document answered {status} {body}"
        );
        let policy = policy_of(&emulator, case, bus)
            .await
            .unwrap_or_else(|| panic!("case {case}: expected a Policy after the grant"));
        let held = statements(case, &policy);
        assert_eq!(
            held.len(),
            statement_count + 1,
            "case {case}: expected the document's statements plus the grant, got {policy}"
        );
        assert_eq!(
            held.last().map(|statement| statement["Sid"].clone()),
            Some(json!("AddedAfterTheDocument")),
            "case {case}: expected the grant last, got {policy}"
        );
    }

    // A request that is not a policy at all is refused, and the stored policy is
    // left as it was.
    let before = describe(&emulator, "before the refusals", bus).await;
    let refused = [
        ("not JSON at all", json!("this is not a policy")),
        ("a JSON value that is not an object", json!("[1,2,3]")),
        (
            "an object carrying no Statement",
            json!(format!("{{\"Version\":\"{POLICY_VERSION}\"}}")),
        ),
        (
            "a Statement that is not a statement",
            json!(format!(
                "{{\"Version\":\"{POLICY_VERSION}\",\"Statement\":[\"events:PutEvents\"]}}"
            )),
        ),
    ];
    for (case, document) in &refused {
        let (status, body) = call(
            &emulator,
            "PutPermission",
            &json!({ "EventBusName": bus, "Policy": document }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
        assert_eq!(
            describe(&emulator, case, bus).await,
            before,
            "case {case}: a refused policy must leave the stored policy unchanged"
        );
    }
}

/// A grant request that is neither a described grant nor a whole policy, and a
/// revocation that names no grant, are refused — and nothing is installed. The
/// rows differ along which member is missing or too many.
#[tokio::test]
async fn a_grant_or_revocation_that_names_nothing_is_refused() {
    let emulator = emulator();
    let bus = "refusals";
    create_bus(&emulator, bus).await;

    let grants = [
        (
            "no grant and no policy at all",
            json!({ "EventBusName": bus }),
            "ValidationException",
        ),
        (
            "a statement id with no action or principal",
            json!({ "EventBusName": bus, "StatementId": "Half" }),
            "ValidationException",
        ),
        (
            "an action and a principal with no statement id",
            json!({
                "EventBusName": bus,
                "Action": "events:PutEvents",
                "Principal": "111122223333",
            }),
            "ValidationException",
        ),
        (
            "a whole policy and a described grant at once",
            json!({
                "EventBusName": bus,
                "Policy": format!("{{\"Version\":\"{POLICY_VERSION}\",\"Statement\":[]}}"),
                "StatementId": "Both",
                "Action": "events:PutEvents",
                "Principal": "111122223333",
            }),
            "ValidationException",
        ),
        (
            "a grant on a bus that was never created",
            json!({
                "EventBusName": "never-created",
                "StatementId": "Nowhere",
                "Action": "events:PutEvents",
                "Principal": "111122223333",
            }),
            "ResourceNotFoundException",
        ),
    ];
    for (case, request, expected_code) in &grants {
        let (status, body) = call(&emulator, "PutPermission", request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            &error_code(&body),
            expected_code,
            "case {case}: expected {expected_code}, got {body}"
        );
        let described = describe(&emulator, case, bus).await;
        assert_eq!(
            described.get("Policy").cloned().unwrap_or(Value::Null),
            Value::Null,
            "case {case}: a refused grant must install nothing, got {described}"
        );
    }

    let revocations = [
        (
            "neither a statement id nor every permission",
            json!({ "EventBusName": bus }),
            "ValidationException",
        ),
        (
            "a statement id and every permission at once",
            json!({
                "EventBusName": bus,
                "StatementId": "Both",
                "RemoveAllPermissions": true,
            }),
            "ValidationException",
        ),
        (
            "a statement id no grant carries",
            json!({ "EventBusName": bus, "StatementId": "NeverGranted" }),
            "ResourceNotFoundException",
        ),
        (
            "a bus that was never created",
            json!({ "EventBusName": "never-created", "StatementId": "Nowhere" }),
            "ResourceNotFoundException",
        ),
    ];
    for (case, request, expected_code) in &revocations {
        let (status, body) = call(&emulator, "RemovePermission", request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            &error_code(&body),
            expected_code,
            "case {case}: expected {expected_code}, got {body}"
        );
    }
}

/// The published policy size bounds what can be installed: grants are accepted
/// while the assembled document fits in 10 KB and the one that would pass it is
/// refused with the modelled `PolicyLengthExceededException`, leaving the stored
/// policy as it was.
#[tokio::test]
async fn a_policy_past_the_published_size_limit_is_refused() {
    let emulator = emulator();
    let bus = "policy-limit";
    create_bus(&emulator, bus).await;

    let mut refusals = 0;
    let mut accepted = 0;
    let mut last_accepted: Option<Value> = None;
    for index in 0_u64..200 {
        let sid = format!("{index:0>width$}", width = STATEMENT_ID_MAX);
        let before = describe(&emulator, &sid, bus).await;
        let (status, body) = call(
            &emulator,
            "PutPermission",
            &json!({
                "EventBusName": bus,
                "StatementId": sid,
                "Action": "events:PutEvents",
                "Principal": format!("{:012}", 111_111_111_111_u64 + index),
            }),
        )
        .await;
        if status == 200 {
            accepted += 1;
            last_accepted = Some(describe(&emulator, &sid, bus).await);
            continue;
        }
        refusals += 1;
        assert_eq!(
            status, 400,
            "case {sid}: expected either a grant or a refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "PolicyLengthExceededException",
            "case {sid}: expected PolicyLengthExceededException past the limit, got {body}"
        );
        assert_eq!(
            describe(&emulator, &sid, bus).await,
            before,
            "case {sid}: a refused grant must leave the stored policy unchanged"
        );
        break;
    }

    assert!(
        accepted > 0,
        "expected the grants that fit to be accepted, got {accepted}"
    );
    assert_eq!(
        refusals, 1,
        "expected the first grant past the limit to be refused, got {refusals} refusal(s)"
    );
    let described = last_accepted.expect("at least one grant was accepted");
    let size = described["Policy"]
        .as_str()
        .expect("the bus holds a policy")
        .len();
    assert!(
        size <= MAX_POLICY_BYTES,
        "expected a stored policy of at most {MAX_POLICY_BYTES} bytes, got {size}"
    );
}

/// Every stored bus is listed once, in name order, with the same members
/// `DescribeEventBus` reports for it; `NamePrefix` narrows the list and `Limit`
/// pages it behind an opaque token that resumes where the previous page ended.
/// The rows differ along the prefix and the page size.
#[tokio::test]
async fn every_stored_bus_is_listed_once_in_name_order() {
    let emulator = emulator();
    let created = [
        "orders",
        "orders.eu",
        "orders_us",
        "payments",
        "Payments.audit",
    ];
    for name in created {
        create_bus(&emulator, name).await;
    }
    let (status, configured) = call(
        &emulator,
        "UpdateEventBus",
        &json!({ "Name": "orders", "Description": "the orders bus" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "configuring a listed bus answered {status} {configured}"
    );
    let (status, granted) = call(
        &emulator,
        "PutPermission",
        &json!({
            "EventBusName": "orders",
            "StatementId": "Listed",
            "Action": "events:PutEvents",
            "Principal": "111122223333",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "granting on a listed bus answered {status} {granted}"
    );

    // The store's own order: the names sorted, the account's default bus
    // included, since "lists all the event buses in your account, including the
    // default event bus".
    let mut all: Vec<String> = created
        .iter()
        .map(|name| (*name).to_string())
        .chain([DEFAULT_EVENT_BUS_NAME.to_string()])
        .collect();
    all.sort();

    let (status, listed) = call(&emulator, "ListEventBuses", &json!({})).await;
    assert_eq!(
        status, 200,
        "ListEventBuses answered {status} {listed}"
    );
    let buses = listed["EventBuses"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an EventBuses list, got {listed}"))
        .clone();
    let names: Vec<&str> = buses
        .iter()
        .map(|bus| bus["Name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        all.iter().map(String::as_str).collect::<Vec<_>>(),
        "expected every stored bus once in name order, got {listed}"
    );
    assert_eq!(
        listed.get("NextToken").cloned().unwrap_or(Value::Null),
        Value::Null,
        "a page holding every bus carries no token, got {listed}"
    );

    // Every member of a listed bus is the one `DescribeEventBus` reports.
    for bus in &buses {
        let name = bus["Name"].as_str().expect("the listed bus is named");
        let described = describe(&emulator, name, name).await;
        for member in [
            "Name",
            "Arn",
            "Description",
            "Policy",
            "CreationTime",
            "LastModifiedTime",
        ] {
            assert_eq!(
                bus.get(member).cloned().unwrap_or(Value::Null),
                described.get(member).cloned().unwrap_or(Value::Null),
                "case {name}: ListEventBuses and DescribeEventBus must agree on {member}, \
                 got {bus} then {described}"
            );
        }
    }

    // `NamePrefix` "limits the results to only those event buses with names that
    // start with the specified prefix" — a prefix comparison, so case matters.
    let prefixes = [
        ("orders", vec!["orders", "orders.eu", "orders_us"]),
        ("orders.", vec!["orders.eu"]),
        ("Payments", vec!["Payments.audit"]),
        ("def", vec![DEFAULT_EVENT_BUS_NAME]),
        ("never-created", vec![]),
    ];
    for (prefix, expected) in &prefixes {
        let (status, listed) =
            call(&emulator, "ListEventBuses", &json!({ "NamePrefix": prefix })).await;
        assert_eq!(
            status, 200,
            "case NamePrefix={prefix}: ListEventBuses answered {status} {listed}"
        );
        let names: Vec<&str> = listed["EventBuses"]
            .as_array()
            .unwrap_or_else(|| panic!("case NamePrefix={prefix}: expected a list, got {listed}"))
            .iter()
            .map(|bus| bus["Name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            &names, expected,
            "case NamePrefix={prefix}: expected {expected:?}, got {listed}"
        );
    }

    // Paging: whatever the page size, the pages concatenate to the whole list
    // exactly once.
    for limit in [1_usize, 2, 3, 100] {
        let mut paged: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "Limit": limit });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let (status, listed) = call(&emulator, "ListEventBuses", &request).await;
            assert_eq!(
                status, 200,
                "case Limit={limit}: ListEventBuses answered {status} {listed}"
            );
            let page = listed["EventBuses"]
                .as_array()
                .unwrap_or_else(|| panic!("case Limit={limit}: expected a list, got {listed}"))
                .clone();
            assert!(
                page.len() <= limit,
                "case Limit={limit}: a page holds at most {limit} buses, got {}",
                page.len()
            );
            paged.extend(
                page.iter()
                    .map(|bus| bus["Name"].as_str().unwrap_or_default().to_string()),
            );
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case Limit={limit}: paging must end, got {pages} pages"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            paged, all,
            "case Limit={limit}: the pages must concatenate to the whole list"
        );
    }

    // A token this service did not issue is refused rather than silently
    // answered with the first page.
    let (status, body) = call(
        &emulator,
        "ListEventBuses",
        &json!({ "NextToken": "not-a-token" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a token this service did not issue must be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException for a foreign token, got {body}"
    );
}

/// A deleted bus is gone from every reader, deleting is idempotent, and the
/// account's `default` bus cannot be deleted.
#[tokio::test]
async fn a_deleted_bus_is_gone_from_every_reader() {
    let emulator = emulator();
    for name in ["kept", "removed"] {
        create_bus(&emulator, name).await;
    }
    let (status, configured) = call(
        &emulator,
        "UpdateEventBus",
        &json!({ "Name": "removed", "Description": "about to go" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "configuring the bus answered {status} {configured}"
    );

    // Deleting is idempotent: this operation models no `ResourceNotFound`, so a
    // name no record carries is a success that removes nothing.
    for attempt in ["the first delete", "the same delete again"] {
        let (status, body) = call(
            &emulator,
            "DeleteEventBus",
            &json!({ "Name": "removed" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {attempt}: DeleteEventBus answered {status} {body}"
        );
        let (status, described) =
            call(&emulator, "DescribeEventBus", &json!({ "Name": "removed" })).await;
        assert_eq!(
            status, 400,
            "case {attempt}: a deleted bus must be gone, got {status} {described}"
        );
        assert_eq!(
            error_code(&described),
            "ResourceNotFoundException",
            "case {attempt}: expected ResourceNotFoundException, got {described}"
        );
    }

    let (status, listed) = call(&emulator, "ListEventBuses", &json!({})).await;
    assert_eq!(status, 200, "ListEventBuses answered {status} {listed}");
    let names: Vec<&str> = listed["EventBuses"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list, got {listed}"))
        .iter()
        .map(|bus| bus["Name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec![DEFAULT_EVENT_BUS_NAME, "kept"],
        "a deleted bus must be gone from the list too, got {listed}"
    );
    assert_eq!(
        emulator
            .engine
            .state::<ComAmazonawsEventbridgeAwseventsState>()
            .lock()
            .buses
            .keys()
            .map(|key| key.name.clone())
            .collect::<Vec<String>>(),
        vec![DEFAULT_EVENT_BUS_NAME.to_string(), "kept".to_string()],
        "a deleted bus must be gone from the store"
    );

    // "You can't delete your account's default event bus."
    let (status, body) = call(
        &emulator,
        "DeleteEventBus",
        &json!({ "Name": DEFAULT_EVENT_BUS_NAME }),
    )
    .await;
    assert_eq!(
        status, 400,
        "deleting the default bus must be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException for the default bus, got {body}"
    );
    let (status, described) = call(&emulator, "DescribeEventBus", &json!({})).await;
    assert_eq!(
        status, 200,
        "the default bus must still be there, got {status} {described}"
    );
}
