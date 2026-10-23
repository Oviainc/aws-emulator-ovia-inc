// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT COMMAND store: `CreateCommand` and the
//! `GetCommand` reader that proves what it persisted.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: `CreateCommand` RECORDS a command under its `commandId`,
//! mints its ARN (`arn:aws:iot:<region>:<account>:command/<commandId>`) and registers
//! that ARN in the shared index the IoT Jobs data plane resolves a `commandArn`
//! against; a later `GetCommand` of that id reports the stored record
//! member-for-member, with the lifecycle flags a created command has
//! (`deprecated: false`, `pendingDeletion: false`) and timestamps that are real clock
//! readings stable across reads. A second create under an id the store already holds
//! is refused `ConflictException` (409) and changes nothing; an id the store does not
//! hold is refused `ResourceNotFoundException` (404). `roleArn` belongs to the
//! `AWS-IoT-FleetWise` namespace alone, so the pairing table below is refused
//! `ValidationException` (400) exactly where the member's documentation says the
//! field does not apply.
//!
//! The table spans what that rule generalises over — the form and length of a command
//! id, which namespace the command is created in, whether it carries a static payload
//! or a template plus preprocessor, how many mandatory parameters it carries and of
//! which parameter types and value forms, which optional members it carries, whether
//! it carries tags, and the region the call is signed for — rather than one recorded
//! request.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    command_registry, iot_commands::MAX_REGISTERED_COMMANDS,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and `region`: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to. The signature itself is never
/// checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// A member of the body as it was serialized, or `None` when it is absent.
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name)
    }

    /// A timestamp member as the epoch seconds the restJson1 protocol renders it as.
    fn instant(&self, name: &str) -> Option<f64> {
        self.body.get(name).and_then(Value::as_f64)
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it. The engine is kept so a test can
/// read the SHARED command index — the store the IoT Jobs data plane resolves a
/// `commandArn` against — beside the service's own.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>, region: &str) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_command(&self, command_id: &str, body: &Value) -> Reply {
        self.create_command_in(command_id, body, REGION).await
    }

    async fn create_command_in(&self, command_id: &str, body: &Value, region: &str) -> Reply {
        self.call("PUT", &format!("/commands/{command_id}"), Some(body), region)
            .await
    }

    async fn get_command(&self, command_id: &str) -> Reply {
        self.call("GET", &format!("/commands/{command_id}"), None, REGION)
            .await
    }

    /// Whether the SHARED command index holds `command_arn` — the question
    /// `StartCommandExecution` asks before it enqueues anything.
    fn index_holds(&self, command_arn: &str) -> bool {
        command_registry(&*self.engine)
            .expect("the shared IoT command index")
            .lock()
            .contains(command_arn)
    }

    /// Register `count` command ARNs directly in the shared index, filling it to its
    /// ceiling without driving `count` HTTP requests.
    fn fill_index(&self, count: usize) {
        let index = command_registry(&*self.engine).expect("the shared IoT command index");
        let mut index = index.lock();
        for ordinal in 0..count {
            index
                .register(&expected_arn(
                    REGION,
                    &format!("prefilled-command-{ordinal}"),
                ))
                .unwrap_or_else(|refusal| panic!("prefilling command {ordinal}: {refusal}"));
        }
    }
}

/// The ARN a command created in `region` must carry.
fn expected_arn(region: &str, command_id: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:command/{command_id}")
}

/// Epoch seconds now, for bounding a reported timestamp.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// One create request: a label, the command id, the region it is signed for, and the
/// `CreateCommandRequest` body.
struct Case {
    label: &'static str,
    command_id: String,
    region: &'static str,
    body: Value,
}

/// The table every case list below is built from. It spans what the rule generalises
/// over:
///
/// * the form and length of `commandId` — `CommandId` is modelled
///   `length 1..64, pattern ^[a-zA-Z0-9_-]+$`
///   (`smithy_query(query="operation",operation="CreateCommand")`), so the rows
///   include the shortest legal id, the longest, and one using every character class
///   the pattern allows;
/// * which namespace the command is created in — `CommandNamespace` admits `AWS-IoT`
///   and `AWS-IoT-FleetWise` (`smithy_query(query="shape",shape="CommandNamespace")`),
///   so the rows span both plus a request that names none;
/// * static versus dynamic commands — a row with `payload` (content and
///   `contentType`), a row with `payloadTemplate` plus a `preprocessor`, and rows with
///   neither;
/// * how many `mandatoryParameters` a command carries and of which type —
///   `CommandParameterType` admits `STRING`, `INTEGER`, `DOUBLE`, `LONG`,
///   `UNSIGNEDLONG`, `BOOLEAN` and `BINARY`
///   (`smithy_query(query="shape",shape="CommandParameterType")`), and
///   `CommandParameterValue` carries one member per type (`S`, `B`, `I`, `L`, `D`,
///   `BIN`, `UL`), so one row carries a parameter of every type with its value, its
///   `defaultValue` and its `valueConditions`;
/// * which optional members it carries — `displayName`, `description` (including the
///   empty string the `length 0..` minimum admits) and `tags`;
/// * the region the call is signed for, which is the region the command's ARN is
///   scoped to.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "only the required command id",
            command_id: "minimal-command".to_string(),
            region: REGION,
            body: json!({}),
        },
        Case {
            label: "a static AWS-IoT command with a JSON payload and a display name",
            command_id: "reboot_device".to_string(),
            region: REGION,
            body: json!({
                "namespace": "AWS-IoT",
                "displayName": "Reboot the device",
                "description": "Ask the device to reboot immediately",
                // A blob is base64 on the wire: this is `{"cmd":"reboot"}`.
                "payload": {
                    "content": "eyJjbWQiOiJyZWJvb3QifQ==",
                    "contentType": "application/json",
                },
            }),
        },
        Case {
            label: "a binary payload and an empty description",
            command_id: "raw-bytes".to_string(),
            region: REGION,
            body: json!({
                "description": "",
                // The bytes 0x00 0x01 0x02, base64 without padding.
                "payload": {
                    "content": "AAEC",
                    "contentType": "application/octet-stream",
                },
            }),
        },
        Case {
            label: "a dynamic command: payload template, preprocessor and one parameter",
            command_id: "set-temperature".to_string(),
            region: REGION,
            body: json!({
                "namespace": "AWS-IoT",
                "payloadTemplate": "{\"target\":\"${target}\"}",
                "preprocessor": { "awsJsonSubstitution": { "outputFormat": "JSON" } },
                "mandatoryParameters": [{
                    "name": "target",
                    "type": "DOUBLE",
                    "defaultValue": { "D": 21.5 },
                    "description": "the target temperature",
                }],
            }),
        },
        Case {
            label: "a parameter of every modelled type, with conditions",
            command_id: "every_parameter_type".to_string(),
            region: REGION,
            body: json!({
                "payloadTemplate": "{\"all\":\"${s}\"}",
                "preprocessor": { "awsJsonSubstitution": { "outputFormat": "CBOR" } },
                "mandatoryParameters": [
                    {
                        "name": "s",
                        "type": "STRING",
                        "value": { "S": "on" },
                        "valueConditions": [{
                            "comparisonOperator": "IN_SET",
                            "operand": { "strings": ["on", "off"] },
                        }],
                    },
                    { "name": "b", "type": "BOOLEAN", "value": { "B": true } },
                    {
                        "name": "i",
                        "type": "INTEGER",
                        "value": { "I": 42 },
                        "valueConditions": [{
                            "comparisonOperator": "IN_RANGE",
                            "operand": { "numberRange": { "min": "1", "max": "100" } },
                        }],
                    },
                    { "name": "l", "type": "LONG", "value": { "L": 9_000_000_000_i64 } },
                    { "name": "d", "type": "DOUBLE", "value": { "D": 0.25 } },
                    { "name": "ul", "type": "UNSIGNEDLONG", "value": { "UL": "18446744073709551615" } },
                    { "name": "bin", "type": "BINARY", "value": { "BIN": "AAEC" } },
                ],
            }),
        },
        Case {
            label: "an AWS-IoT-FleetWise command, which carries the role that builds its payload",
            command_id: "fleetwise-signal-read".to_string(),
            region: REGION,
            body: json!({
                "namespace": "AWS-IoT-FleetWise",
                "roleArn": "arn:aws:iam::000000000000:role/iot-fleetwise-command",
                "payloadTemplate": "{\"signal\":\"${signal}\"}",
                "preprocessor": { "awsJsonSubstitution": { "outputFormat": "JSON" } },
                "mandatoryParameters": [{ "name": "signal", "type": "STRING" }],
            }),
        },
        Case {
            label: "a tagged command signed for another region",
            command_id: "EU_Command-01".to_string(),
            region: "eu-west-1",
            body: json!({
                "displayName": "EU command",
                "tags": [
                    { "Key": "team", "Value": "platform" },
                    { "Key": "env", "Value": "test" },
                ],
            }),
        },
        Case {
            label: "the shortest legal command id",
            command_id: "a".to_string(),
            region: REGION,
            body: json!({ "namespace": "AWS-IoT" }),
        },
        Case {
            label: "the longest legal command id",
            command_id: "c".repeat(64),
            region: REGION,
            body: json!({}),
        },
    ]
}

#[tokio::test]
async fn a_created_command_round_trips_through_get_command() {
    let emulator = Emulator::new();
    let before = now_epoch_seconds();

    for case in cases() {
        let label = case.label;
        let created = emulator
            .create_command_in(&case.command_id, &case.body, case.region)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateCommand must succeed, got {created:?}"
        );
        assert_eq!(
            created.text("commandId").as_deref(),
            Some(case.command_id.as_str()),
            "case {label:?}: the create must report the id it recorded, got {created:?}"
        );
        let arn = expected_arn(case.region, &case.command_id);
        assert_eq!(
            created.text("commandArn"),
            Some(arn.clone()),
            "case {label:?}: expected commandArn {arn:?}, got {created:?}"
        );
        // The shared index is what `StartCommandExecution` resolves a `commandArn`
        // against, so a created command has to be in it.
        assert!(
            emulator.index_holds(&arn),
            "case {label:?}: the created command must be in the shared index, {arn:?} is not"
        );

        let read = emulator.get_command(&case.command_id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetCommand must find the created command, got {read:?}"
        );
        assert_eq!(
            read.text("commandId").as_deref(),
            Some(case.command_id.as_str()),
            "case {label:?}: the read must report the id the command was created under, got {read:?}"
        );
        assert_eq!(
            read.text("commandArn"),
            Some(arn.clone()),
            "case {label:?}: the read must report the minted ARN, got {read:?}"
        );
        // Every member the create carried is reported back exactly as supplied, and
        // every member it did not carry is reported by nobody.
        for member in [
            "namespace",
            "displayName",
            "description",
            "payload",
            "payloadTemplate",
            "preprocessor",
            "mandatoryParameters",
            "roleArn",
        ] {
            assert_eq!(
                read.member(member),
                case.body.get(member),
                "case {label:?}: expected {member} {:?}, got {read:?}",
                case.body.get(member)
            );
        }
        // A created command is neither deprecated nor being deleted: both flags are
        // part of the record, not of the request.
        assert_eq!(
            read.member("deprecated").and_then(Value::as_bool),
            Some(false),
            "case {label:?}: a created command is not deprecated, got {read:?}"
        );
        assert_eq!(
            read.member("pendingDeletion").and_then(Value::as_bool),
            Some(false),
            "case {label:?}: a created command is not pending deletion, got {read:?}"
        );

        // `createdAt` and `lastUpdatedAt` are the instant of the create: both lie
        // between the instant before the first create and the instant after this
        // read, so they are real clock readings rather than fixed or zero timestamps,
        // and the create IS the command's last write.
        let after = now_epoch_seconds();
        let created_at = read.instant("createdAt").unwrap_or_else(|| {
            panic!("case {label:?}: the command reported no createdAt: {read:?}")
        });
        let last_updated_at = read.instant("lastUpdatedAt").unwrap_or_else(|| {
            panic!("case {label:?}: the command reported no lastUpdatedAt: {read:?}")
        });
        assert!(
            (before - 1.0..=after + 1.0).contains(&created_at),
            "case {label:?}: createdAt {created_at} is outside [{before}, {after}], got {read:?}"
        );
        assert!(
            (created_at - 1.0..=created_at + 1.0).contains(&last_updated_at),
            "case {label:?}: expected lastUpdatedAt to be the create instant {created_at}, \
             got {last_updated_at} in {read:?}"
        );
    }
}

#[tokio::test]
async fn reading_a_command_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    // `GetCommand` reports the command; it must not re-time, re-mint or otherwise
    // rewrite it, so two reads of one command are byte-identical.
    for case in cases() {
        let label = case.label;
        let created = emulator
            .create_command_in(&case.command_id, &case.body, case.region)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateCommand must succeed, got {created:?}"
        );

        let first = emulator.get_command(&case.command_id).await;
        let second = emulator.get_command(&case.command_id).await;
        assert_eq!(
            first.status, second.status,
            "case {label:?}: both reads must answer the same status, got {second:?}"
        );
        assert_eq!(
            first.body, second.body,
            "case {label:?}: a read must not change the command, got {first:?} then {second:?}"
        );
    }
}

#[tokio::test]
async fn a_repeat_create_of_one_command_id_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();

    // The id is the command's identity, so a second create under an id the store
    // holds is refused 409 `ConflictException` whatever the second body says — and
    // the command that exists is left exactly as it was.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "the same body again",
            "repeat-same",
            json!({ "description": "first" }),
        ),
        (
            "a different description",
            "repeat-description",
            json!({ "description": "second" }),
        ),
        (
            "a payload the first create did not carry",
            "repeat-payload",
            json!({
                "description": "first",
                "payload": { "content": "AAEC", "contentType": "application/octet-stream" },
            }),
        ),
        (
            "another namespace",
            "repeat-namespace",
            json!({ "description": "first", "namespace": "AWS-IoT" }),
        ),
    ];

    for (label, command_id, second_body) in rows {
        let first_body = json!({ "description": "first" });
        let first = emulator.create_command(command_id, &first_body).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: the first CreateCommand must succeed, got {first:?}"
        );
        let before = emulator.get_command(command_id).await;

        let second = emulator.create_command(command_id, &second_body).await;
        assert_eq!(
            second.status, 409,
            "case {label:?}: a repeat create must be refused 409, got {second:?}"
        );
        assert_eq!(
            second.error_code.as_deref(),
            Some("ConflictException"),
            "case {label:?}: expected ConflictException, got {second:?}"
        );
        // The refusal is built from the request: it names the command that exists.
        assert_eq!(
            second.text("resourceId").as_deref(),
            Some(command_id),
            "case {label:?}: the refusal must name the command that exists, got {second:?}"
        );

        let after = emulator.get_command(command_id).await;
        assert_eq!(
            before.body, after.body,
            "case {label:?}: the refused create must leave the command untouched, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_command_the_store_does_not_hold_is_not_found() {
    let emulator = Emulator::new();

    // One command exists, so the refusals below are about the ID not being in the
    // store rather than the store being empty.
    let created = emulator
        .create_command("existing-command", &json!({}))
        .await;
    assert_eq!(
        created.status, 200,
        "the setup CreateCommand must succeed, got {created:?}"
    );

    for command_id in [
        "a",
        "unknown-command",
        "existing-command-2",
        "EXISTING-COMMAND",
        &"c".repeat(64),
    ] {
        let read = emulator.get_command(command_id).await;
        assert_eq!(
            read.status, 404,
            "case {command_id:?}: an unknown command id must be refused 404, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {command_id:?}: expected ResourceNotFoundException, got {read:?}"
        );
        assert!(
            read.member("commandArn").is_none(),
            "case {command_id:?}: a refusal carries no command, got {read:?}"
        );
    }

    // The command that does exist is still readable: the refusals above wrote nothing.
    let read = emulator.get_command("existing-command").await;
    assert_eq!(
        read.status, 200,
        "the existing command must still be readable, got {read:?}"
    );
}

/// One `namespace`/`roleArn` pairing: whether `CreateCommand` accepts it.
struct Pairing {
    label: &'static str,
    command_id: &'static str,
    namespace: Option<&'static str>,
    role_arn: Option<&'static str>,
    accepted: bool,
}

#[tokio::test]
async fn a_role_arn_is_accepted_only_with_the_fleetwise_namespace() {
    let emulator = Emulator::new();
    let role = "arn:aws:iam::000000000000:role/iot-fleetwise-command";

    // `CreateCommand$roleArn` is "The IAM role that you must provide when using the
    // AWS-IoT-FleetWise namespace. […] This field is not supported when you use the
    // AWS-IoT namespace", so the role and the namespace have to agree: every pairing
    // that disagrees is refused 400 `ValidationException` and records nothing.
    //
    let rows = vec![
        Pairing {
            label: "no namespace and no role",
            command_id: "pairing-none",
            namespace: None,
            role_arn: None,
            accepted: true,
        },
        Pairing {
            label: "the AWS-IoT namespace without a role",
            command_id: "pairing-awsiot",
            namespace: Some("AWS-IoT"),
            role_arn: None,
            accepted: true,
        },
        Pairing {
            label: "the AWS-IoT-FleetWise namespace with a role",
            command_id: "pairing-fleetwise",
            namespace: Some("AWS-IoT-FleetWise"),
            role_arn: Some(role),
            accepted: true,
        },
        Pairing {
            label: "the AWS-IoT-FleetWise namespace without a role",
            command_id: "pairing-fleetwise-no-role",
            namespace: Some("AWS-IoT-FleetWise"),
            role_arn: None,
            accepted: false,
        },
        Pairing {
            label: "the AWS-IoT namespace with a role",
            command_id: "pairing-awsiot-role",
            namespace: Some("AWS-IoT"),
            role_arn: Some(role),
            accepted: false,
        },
        Pairing {
            label: "no namespace but a role",
            command_id: "pairing-none-role",
            namespace: None,
            role_arn: Some(role),
            accepted: false,
        },
    ];

    for Pairing {
        label,
        command_id,
        namespace,
        role_arn,
        accepted,
    } in rows
    {
        let mut body = serde_json::Map::new();
        if let Some(namespace) = namespace {
            body.insert("namespace".to_string(), json!(namespace));
        }
        if let Some(role_arn) = role_arn {
            body.insert("roleArn".to_string(), json!(role_arn));
        }
        let body = Value::Object(body);

        let created = emulator.create_command(command_id, &body).await;
        let read = emulator.get_command(command_id).await;
        if accepted {
            assert_eq!(
                created.status, 200,
                "case {label:?}: this pairing must be accepted, got {created:?}"
            );
            assert_eq!(
                read.status, 200,
                "case {label:?}: the accepted create must record the command, got {read:?}"
            );
            assert_eq!(
                read.text("roleArn").as_deref(),
                role_arn,
                "case {label:?}: expected roleArn {role_arn:?}, got {read:?}"
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {label:?}: this pairing must be refused 400, got {created:?}"
            );
            assert_eq!(
                created.error_code.as_deref(),
                Some("ValidationException"),
                "case {label:?}: expected ValidationException, got {created:?}"
            );
            assert_eq!(
                read.status, 404,
                "case {label:?}: the refused create must record nothing, got {read:?}"
            );
            assert!(
                !emulator.index_holds(&expected_arn(REGION, command_id)),
                "case {label:?}: the refused create must register no ARN in the shared index"
            );
        }
    }
}

#[tokio::test]
async fn the_command_store_is_bounded() {
    let emulator = Emulator::new();

    // The store is one a caller grows, so it has a ceiling: with the shared index
    // filled to `MAX_REGISTERED_COMMANDS`, every further create is refused with the
    // modelled `ServiceQuotaExceededException` (HTTP 402) and records nothing, while
    // the commands already recorded stay readable.
    let recorded = emulator.create_command("under-the-ceiling", &json!({})).await;
    assert_eq!(
        recorded.status, 200,
        "a create below the ceiling must succeed, got {recorded:?}"
    );
    emulator.fill_index(MAX_REGISTERED_COMMANDS - 1);

    for command_id in ["one-too-many", "another-too-many"] {
        let refused = emulator.create_command(command_id, &json!({})).await;
        assert_eq!(
            refused.status, 402,
            "case {command_id:?}: a create past the ceiling must be refused 402, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {command_id:?}: expected ServiceQuotaExceededException, got {refused:?}"
        );
        let read = emulator.get_command(command_id).await;
        assert_eq!(
            read.status, 404,
            "case {command_id:?}: the refused create must record no command, got {read:?}"
        );
    }

    let read = emulator.get_command("under-the-ceiling").await;
    assert_eq!(
        read.status, 200,
        "the recorded command must still be readable, got {read:?}"
    );
}
