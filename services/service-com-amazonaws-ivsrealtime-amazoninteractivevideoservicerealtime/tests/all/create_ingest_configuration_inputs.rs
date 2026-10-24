// SPDX-License-Identifier: CC0-1.0

//! `CreateIngestConfiguration`'s INPUT SURFACE over the wire.
//!
//! `tests/all/ingest_configurations.rs` pins the create/read round trip and the two
//! rules the model carries no trait for (the `RTMP`/`insecureIngest` pairing and the
//! documented 1 KB `attributes` bound). This suite pins the complementary rule, over
//! the whole class of constrained input members rather than one of them:
//!
//! > A value that satisfies the input member's modelled constraint (`@length`,
//! > `@pattern`, enum, `@required`) is PERSISTED and reported verbatim by
//! > `GetIngestConfiguration`; a value outside it is refused with the modelled
//! > `ValidationException` (HTTP 400) naming that member, and the refusal persists
//! > nothing and disturbs nothing already persisted.
//!
//! Every constraint quoted below comes from
//! `smithy_query(query="operation",operation="CreateIngestConfiguration")` and
//! `smithy_query(query="shape",shape="IngestConfigurationName,UserId,IngestConfigurationStageArn,IngestProtocol")`;
//! each member is exercised on its published bound and one value past it, and a
//! configuration created here is always read back through `GetIngestConfiguration`, so
//! no assertion looks at an internal call.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The account id every ARN this emulator mints carries (`engine::emulator_account_id`),
/// and so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// `@length` maximum of `IngestConfigurationName` and of `UserId` (both `0..128`), and of
/// `IngestConfigurationStageArn`.
const MAX_NAME_LENGTH: usize = 128;

/// `@length` maximum of `Tags` — at most 50 entries.
const MAX_TAGS: usize = 50;

/// The values `IngestProtocol` accepts, from
/// `smithy_query(query="shape",shape="IngestProtocol")`.
const INGEST_PROTOCOLS: [&str; 2] = ["RTMP", "RTMPS"];

/// What the model reports for a configuration associated with no stage: the empty string,
/// which `IngestConfigurationStageArn`'s own `@pattern` (`^^$|^arn:...$$`) admits.
const NO_STAGE: &str = "";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential scope
    /// naming `ivs` in [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{REGION}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a configuration, asserting the create succeeded, and return the
    /// `ingestConfiguration` the service reported.
    async fn create(&self, case: &str, body: &Value) -> Value {
        let (status, response) = self.call("CreateIngestConfiguration", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateIngestConfiguration {body} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// Read `arn` back, asserting the read succeeded.
    async fn get(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self
            .call("GetIngestConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetIngestConfiguration on {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// A string member, read as the empty string when absent.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A stage ARN in [`REGION`] naming `id` — the form
/// `IngestConfigurationStageArn`'s `@pattern` admits.
fn stage_arn(id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:stage/{id}")
}

/// A stage ARN of exactly `length` characters, so the `@length` bound of
/// `IngestConfigurationStageArn` can be probed with a value that violates NOTHING ELSE.
fn stage_arn_of_length(length: usize) -> String {
    let prefix = stage_arn("");
    let id_length = length
        .checked_sub(prefix.len())
        .expect("a stage ARN cannot be shorter than its own prefix");
    format!("{prefix}{}", "a".repeat(id_length))
}

/// A `tags` map with `count` entries — the measure `Tags`'s `@length` is stated in.
fn tags_of_size(count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("k{index}"), Value::String(format!("v{index}")));
    }
    Value::Object(map)
}

/// What a row expects of the service.
#[derive(Debug)]
enum Expect {
    /// The create is accepted and `GetIngestConfiguration` reports this value for the
    /// member under test (`Value::Null` when the member is reported absent).
    Persisted(Value),
    /// The create is refused with the modelled `ValidationException`, naming the member.
    Refused,
}

/// One row of the input class: a request, the member it varies, and what the service owes.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// The input member under test, as the refusal message and the reported
    /// `IngestConfiguration` name it.
    member: &'static str,
    request: Value,
    expect: Expect,
}

/// The input class, one row per (member, constraint, side of the bound).
///
/// Constraints, all from `smithy_query` as the module doc records:
/// `name` → `IngestConfigurationName` (`@length` 0..128, `@pattern ^[a-zA-Z0-9-_]*$`);
/// `userId` → `UserId` (`@length` 0..128, NO pattern — "can be any UTF-8 encoded text");
/// `stageArn` → `IngestConfigurationStageArn` (`@length` 0..128,
/// `@pattern ^^$|^arn:aws:ivs:[a-z0-9-]+:[0-9]+:stage/[a-zA-Z0-9-]+$$`);
/// `ingestProtocol` → `IngestProtocol` (`@required`, enum `RTMP | RTMPS`);
/// `tags` → `Tags` (`@length` 0..50).
fn rows() -> Vec<Row> {
    let mut rows = vec![
        // ---- name: @length 0..128 ----
        Row {
            case: "a name at the @length minimum (the empty string)",
            member: "name",
            request: json!({ "ingestProtocol": "RTMPS", "name": "" }),
            expect: Expect::Persisted(json!("")),
        },
        Row {
            case: "a name inside the character class, mixing every admitted kind",
            member: "name",
            request: json!({ "ingestProtocol": "RTMPS", "name": "Studio-Feed_07" }),
            expect: Expect::Persisted(json!("Studio-Feed_07")),
        },
        Row {
            case: "a name at the @length maximum",
            member: "name",
            request: json!({ "ingestProtocol": "RTMPS", "name": "n".repeat(MAX_NAME_LENGTH) }),
            expect: Expect::Persisted(json!("n".repeat(MAX_NAME_LENGTH))),
        },
        Row {
            case: "a name one character past the @length maximum",
            member: "name",
            request: json!({ "ingestProtocol": "RTMPS", "name": "n".repeat(MAX_NAME_LENGTH + 1) }),
            expect: Expect::Refused,
        },
        Row {
            case: "a name carrying a space, outside the @pattern character class",
            member: "name",
            request: json!({ "ingestProtocol": "RTMPS", "name": "studio feed" }),
            expect: Expect::Refused,
        },
        Row {
            case: "a name carrying a dot, outside the @pattern character class",
            member: "name",
            request: json!({ "ingestProtocol": "RTMPS", "name": "studio.feed" }),
            expect: Expect::Refused,
        },
        // ---- userId: @length 0..128, no pattern ----
        Row {
            case: "a userId at the @length minimum (the empty string)",
            member: "userId",
            request: json!({ "ingestProtocol": "RTMPS", "userId": "" }),
            expect: Expect::Persisted(json!("")),
        },
        Row {
            case: "a userId of non-ASCII UTF-8 text, which UserId declares no pattern against",
            member: "userId",
            request: json!({ "ingestProtocol": "RTMPS", "userId": "üser:7 ✨" }),
            expect: Expect::Persisted(json!("üser:7 ✨")),
        },
        Row {
            case: "a userId at the @length maximum",
            member: "userId",
            request: json!({ "ingestProtocol": "RTMPS", "userId": "u".repeat(MAX_NAME_LENGTH) }),
            expect: Expect::Persisted(json!("u".repeat(MAX_NAME_LENGTH))),
        },
        Row {
            case: "a userId one character past the @length maximum",
            member: "userId",
            request: json!({
                "ingestProtocol": "RTMPS",
                "userId": "u".repeat(MAX_NAME_LENGTH + 1),
            }),
            expect: Expect::Refused,
        },
        // ---- stageArn: @length 0..128 and the ARN @pattern ----
        Row {
            case: "no stage association at all (the member omitted)",
            member: "stageArn",
            request: json!({ "ingestProtocol": "RTMPS" }),
            expect: Expect::Persisted(json!(NO_STAGE)),
        },
        Row {
            case: "an explicitly empty stage association, which the @pattern admits",
            member: "stageArn",
            request: json!({ "ingestProtocol": "RTMPS", "stageArn": NO_STAGE }),
            expect: Expect::Persisted(json!(NO_STAGE)),
        },
        Row {
            case: "a well-formed stage ARN no stage of this store carries",
            member: "stageArn",
            request: json!({ "ingestProtocol": "RTMPS", "stageArn": stage_arn("Absent-9") }),
            expect: Expect::Persisted(json!(stage_arn("Absent-9"))),
        },
        Row {
            case: "a stage ARN at the @length maximum",
            member: "stageArn",
            request: json!({
                "ingestProtocol": "RTMPS",
                "stageArn": stage_arn_of_length(MAX_NAME_LENGTH),
            }),
            expect: Expect::Persisted(json!(stage_arn_of_length(MAX_NAME_LENGTH))),
        },
        Row {
            case: "a stage ARN one character past the @length maximum",
            member: "stageArn",
            request: json!({
                "ingestProtocol": "RTMPS",
                "stageArn": stage_arn_of_length(MAX_NAME_LENGTH + 1),
            }),
            expect: Expect::Refused,
        },
        Row {
            case: "a stage ARN naming another resource type",
            member: "stageArn",
            request: json!({
                "ingestProtocol": "RTMPS",
                "stageArn": format!("arn:aws:ivs:{REGION}:{ACCOUNT}:ingest-configuration/abc"),
            }),
            expect: Expect::Refused,
        },
        Row {
            case: "a stage ARN that is not an ARN at all",
            member: "stageArn",
            request: json!({ "ingestProtocol": "RTMPS", "stageArn": "stage/abc" }),
            expect: Expect::Refused,
        },
        // ---- ingestProtocol: @required, enum RTMP | RTMPS ----
        Row {
            case: "the required ingestProtocol omitted",
            member: "ingestProtocol",
            request: json!({ "name": "no-protocol" }),
            expect: Expect::Refused,
        },
        Row {
            case: "an ingestProtocol outside the enum",
            member: "ingestProtocol",
            request: json!({ "ingestProtocol": "RTMPZ" }),
            expect: Expect::Refused,
        },
        Row {
            case: "an ingestProtocol in the wrong case, which the enum does not admit",
            member: "ingestProtocol",
            request: json!({ "ingestProtocol": "rtmps" }),
            expect: Expect::Refused,
        },
        // ---- tags: @length 0..50 ----
        Row {
            case: "no tags at all",
            member: "tags",
            request: json!({ "ingestProtocol": "RTMPS" }),
            expect: Expect::Persisted(tags_of_size(0)),
        },
        Row {
            case: "tags at the @length maximum",
            member: "tags",
            request: json!({ "ingestProtocol": "RTMPS", "tags": tags_of_size(MAX_TAGS) }),
            expect: Expect::Persisted(tags_of_size(MAX_TAGS)),
        },
        Row {
            case: "one tag past the @length maximum",
            member: "tags",
            request: json!({ "ingestProtocol": "RTMPS", "tags": tags_of_size(MAX_TAGS + 1) }),
            expect: Expect::Refused,
        },
    ];

    // Every value the `IngestProtocol` enum admits is accepted, cleartext `RTMP` with the
    // `insecureIngest` its own doc requires beside it.
    for protocol in INGEST_PROTOCOLS {
        rows.push(Row {
            case: "an ingestProtocol the enum admits",
            member: "ingestProtocol",
            request: json!({ "ingestProtocol": protocol, "insecureIngest": protocol == "RTMP" }),
            expect: Expect::Persisted(json!(protocol)),
        });
    }

    rows
}

/// The member under test, as `GetIngestConfiguration` reports it — `Value::Null` for a
/// member the wire omits, and a map read as itself so an omitted `tags` compares equal to
/// the empty map.
fn reported(configuration: &Value, member_name: &str) -> Value {
    match configuration.get(member_name) {
        Some(value) => value.clone(),
        None if member_name == "tags" => json!({}),
        None => Value::Null,
    }
}

/// Every modelled constraint on the input is enforced on BOTH sides of its bound: a value
/// the constraint admits is persisted and read back verbatim, a value it does not is the
/// modelled `ValidationException` (HTTP 400) naming the member.
#[tokio::test]
async fn every_modelled_input_constraint_decides_the_create() {
    let emulator = RealTime::new();
    for row in rows() {
        let Row {
            case,
            member: member_name,
            request,
            expect,
        } = row;
        let case = format!("{case} ({member_name})");
        let (status, response) = emulator.call("CreateIngestConfiguration", &request).await;
        match expect {
            Expect::Persisted(expected) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200 for {request}, got {status} {response}"
                );
                let created = response["ingestConfiguration"].clone();
                let created_arn = member(&created, "arn");
                let read = emulator.get(&case, &created_arn).await;
                let actual = reported(&read, member_name);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected GetIngestConfiguration to report \
                     {member_name} as {expected}, got {actual}"
                );
                let created_member = reported(&created, member_name);
                assert_eq!(
                    created_member, expected,
                    "case {case:?}: expected CreateIngestConfiguration to report \
                     {member_name} as {expected}, got {created_member}"
                );
            }
            Expect::Refused => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 for {request}, got {status} {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {response}"
                );
                let message = member(&response, "message");
                assert!(
                    message.starts_with(&format!("CreateIngestConfiguration.{member_name} ")),
                    "case {case:?}: expected a refusal naming the operation and \
                     {member_name}, got {message:?}"
                );
            }
        }
    }
}

/// A refused create persists NOTHING and disturbs nothing already persisted: after every
/// refusal of the class, each configuration created before them reads back byte for byte,
/// and a well-formed create still succeeds.
#[tokio::test]
async fn a_refused_create_leaves_every_persisted_configuration_as_it_was() {
    let emulator = RealTime::new();

    // Two configurations to be disturbed, each read back before the refusals.
    let mut before: BTreeMap<String, Value> = BTreeMap::new();
    for (case, request) in [
        ("a plain configuration", json!({ "ingestProtocol": "RTMPS" })),
        (
            "a configuration with every optional member",
            json!({
                "ingestProtocol": "RTMPS",
                "name": "kept",
                "userId": "user-1",
                "redundantIngest": true,
                "tags": { "env": "test" },
            }),
        ),
    ] {
        let created = emulator.create(case, &request).await;
        let arn = member(&created, "arn");
        let read = emulator.get(case, &arn).await;
        before.insert(arn, read);
    }

    let refused_rows: Vec<Row> = rows()
        .into_iter()
        .filter(|row| matches!(row.expect, Expect::Refused))
        .collect();
    assert!(
        !refused_rows.is_empty(),
        "the input class must carry at least one refused row, got none"
    );
    for row in &refused_rows {
        let (status, response) = emulator.call("CreateIngestConfiguration", &row.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 for {}, got {status} {response}",
            row.case, row.request
        );
    }

    for (arn, expected) in &before {
        let actual = emulator.get("after the refusals", arn).await;
        assert_eq!(
            &actual, expected,
            "case {arn:?}: expected the configuration to be untouched by the refused \
             creates, expected {expected}, got {actual}"
        );
    }

    // And the store still accepts a well-formed create afterwards, with an ARN of its own.
    let created = emulator
        .create(
            "a well-formed create after the refusals",
            &json!({ "ingestProtocol": "RTMPS", "name": "after" }),
        )
        .await;
    let arn = member(&created, "arn");
    assert!(
        !before.contains_key(&arn),
        "expected a fresh ARN after the refusals, got one already in the store: {arn:?}"
    );
    let read = emulator.get("a well-formed create after the refusals", &arn).await;
    let name = member(&read, "name");
    assert_eq!(
        name, "after",
        "case \"a well-formed create after the refusals\": expected name \"after\", got {name:?}"
    );
}
