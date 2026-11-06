// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Step Functions state-machine and execution lifecycle,
//! driven over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rules under test:
//!
//! 1. every operation answers from the ONE persisted record — what a create or a
//!    start stored is what a later describe, list or stop reports, and what was
//!    never stored (or has since been deleted) is the operation's own modelled
//!    error;
//! 2. an execution's outcome is COMPUTED by interpreting the state machine's Amazon
//!    States Language definition: the terminal status, `output` and `error`/`cause`
//!    follow from the definition and the input, and a definition this emulator
//!    cannot run ends `FAILED` with ASL's `States.Runtime` rather than a fabricated
//!    success;
//! 3. a transition the clock owes — an execution reaching its terminal status, a
//!    started deletion removing its state machine — is not applied by the call that
//!    starts it, so a caller acting straight afterwards still sees the transient
//!    state.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    asl::{MAX_CAUSE_BYTES, MAX_ERROR_BYTES, MAX_PAYLOAD_BYTES, MAX_STATE_NAME_CHARS, MAX_WAIT_SECONDS},
    DELETE_SETTLE_DELAY, EXECUTION_SETTLE_DELAY, MAX_TAGS,
};
use service_com_amazonaws_sfn_awsstepfunctions::service::wire;
use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AWSStepFunctions";

/// The sigv4 signing name every request below is signed with (`OPERATIONS.md`).
const SIGNING_NAME: &str = "states";

/// The account segment every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region every request below signs for; every ARN must carry exactly this one.
pub(crate) const REGION: &str = "us-west-2";

/// A role ARN inside the modelled `Arn` length (1..256).
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/StepFunctionsRole";

/// Long enough that any outcome the clock owed has become due.
///
/// The waits below use `std::thread::sleep`: the workspace's `tokio` carries only
/// `macros`/`rt`/`rt-multi-thread`, not `time`, so `tokio::time::sleep` is not
/// available. Each test runs on its own single-threaded runtime with no concurrent
/// task, so blocking the thread holds nothing up.
pub(crate) const PAST_THE_EXECUTION_DELAY: Duration =
    EXECUTION_SETTLE_DELAY.saturating_add(Duration::from_millis(300));

/// Long enough that a started deletion has removed its state machine.
pub(crate) const PAST_THE_DELETE_DELAY: Duration =
    DELETE_SETTLE_DELAY.saturating_add(Duration::from_millis(300));

/// A definition that succeeds immediately with a static result.
pub(crate) const HELLO_DEFINITION: &str =
    r#"{"StartAt":"Hello","States":{"Hello":{"Type":"Pass","Result":{"msg":"hi"},"End":true}}}"#;

/// One emulator: a router that claims `states` requests, a view of the store it
/// serves, and the region its requests are signed for.
///
/// `pub(crate)`, not private: `tests/all.rs` is this test binary's crate root, so a
/// sibling suite drives the same emulator through `crate::state_machine_lifecycle`
/// rather than carrying a second copy of this harness.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    region: String,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::in_region(REGION)
    }

    pub(crate) fn in_region(region: &str) -> Self {
        Self::on_engine(Arc::new(engine::InMemoryEngine::new()), region)
    }

    /// Another client of the SAME store, signing for a different region.
    pub(crate) fn in_region_on(other: &Self, region: &str) -> Self {
        Self::on_engine(Arc::clone(&other.engine), region)
    }

    fn on_engine(engine: Arc<engine::InMemoryEngine>, region: &str) -> Self {
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self {
            router,
            engine,
            region: region.to_string(),
        }
    }

    /// Call one operation and return `(status, body)`. The `Authorization` header is
    /// what routes the request to this service and names the region its ARNs carry.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let response = self.dispatch(operation, body).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "case {operation}: expected a JSON body, got {error} for {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// Call one operation and return the WHOLE response, headers included.
    ///
    /// `call` above reads the status and the body off it; a suite asserting something
    /// about a response that is not in its body — the header a response names the
    /// request it answers in — drives this one instead.
    pub(crate) async fn dispatch(
        &self,
        operation: &str,
        body: &Value,
    ) -> http::Response<Bytes> {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{}/{SIGNING_NAME}/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef",
            self.region
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        self.router.dispatch(request).await
    }

    /// Call one operation, requiring a 200, and return the body.
    pub(crate) async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with body {parsed:?}"
        );
        parsed
    }

    /// Call one operation, requiring a failure, and return `(status, errorCode)`.
    pub(crate) async fn err(&self, operation: &str, body: &Value) -> (u16, String) {
        let (status, parsed) = self.call(operation, body).await;
        assert_ne!(
            status, 200,
            "case {operation}: expected a failure, got 200 with body {parsed:?}"
        );
        (status, error_code(&parsed))
    }

    /// This emulator's own store.
    ///
    /// For a suite that has to assert what a call PERSISTED where no operation of this
    /// API reports it — a synchronous Express execution, which `DescribeExecution` and
    /// `ListExecutions` each refuse to report.
    pub(crate) fn store(
        &self,
    ) -> engine::ServiceState<
        service_com_amazonaws_sfn_awsstepfunctions::ComAmazonawsSfnAwsstepfunctionsState,
    > {
        service_com_amazonaws_sfn_awsstepfunctions::state::service_state(&*self.engine)
            .expect("this engine carries the Step Functions store")
    }
}

/// The `__type` discriminator of an awsJson error body, without its namespace.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// One nested structure as this service puts it on the wire: the members given
/// plus the SWF type discriminator, which every awsJson recording of this API
/// shows on a shared base-model structure.
///
/// The suites here assert what a structure's members HOLD, so they name the
/// discriminator through this one helper; the rule itself — which shapes carry
/// one and what it says — is `tests/all/nested_type_discriminator.rs`.
pub(crate) fn discriminated(shape_name: &str, members: Value) -> Value {
    let Value::Object(mut members) = members else {
        panic!("expected an object of members, got {members:?}")
    };
    members.insert(
        "__type".to_string(),
        Value::String(wire::discriminator(shape_name)),
    );
    Value::Object(members)
}

/// The string member `name` of `body`, which must be present.
pub(crate) fn string(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string '{name}' in {body:?}"))
        .to_string()
}

/// Create a state machine that must succeed, and return its ARN.
pub(crate) async fn create_machine(emulator: &Emulator, name: &str, definition: &str) -> String {
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({ "name": name, "definition": definition, "roleArn": ROLE_ARN }),
        )
        .await;
    string(&created, "stateMachineArn")
}

/// Start an execution that must succeed, and return its ARN.
pub(crate) async fn start_execution(
    emulator: &Emulator,
    state_machine_arn: &str,
    name: &str,
    input: &str,
) -> String {
    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": state_machine_arn, "name": name, "input": input }),
        )
        .await;
    string(&started, "executionArn")
}

pub(crate) async fn describe_execution(emulator: &Emulator, execution_arn: &str) -> Value {
    emulator
        .ok(
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
        )
        .await
}

// ---------------------------------------------------------------------------
// Create -> Describe: the record, not the request
// ---------------------------------------------------------------------------

/// A create stores the state machine and a later describe reports exactly what was
/// stored, over a table that varies the members the record carries.
#[tokio::test]
async fn describe_state_machine_reports_the_created_record() {
    let emulator = Emulator::new();

    // Each row differs along what the record carries: the workflow type, the shape
    // of the definition, and whether the two configuration structures were supplied.
    let cases: Vec<(&str, Value)> = vec![
        (
            "a minimal STANDARD machine",
            json!({
                "name": "minimal",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
            }),
        ),
        (
            "an explicit STANDARD machine with tags",
            json!({
                "name": "tagged",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "type": "STANDARD",
                "tags": [{ "key": "team", "value": "platform" }],
            }),
        ),
        (
            "an EXPRESS machine with both configurations",
            json!({
                "name": "express-with-config",
                "definition": r#"{"StartAt":"Done","States":{"Done":{"Type":"Succeed"}}}"#,
                "roleArn": ROLE_ARN,
                "type": "EXPRESS",
                "loggingConfiguration": {
                    "level": "ALL",
                    "includeExecutionData": true,
                    "destinations": [{
                        "cloudWatchLogsLogGroup": {
                            "logGroupArn": "arn:aws:logs:us-west-2:000000000000:log-group:/sfn:*"
                        }
                    }],
                },
                "tracingConfiguration": { "enabled": true },
            }),
        ),
        (
            "a multi-state definition",
            json!({
                "name": "multi.state_machine-1",
                "definition": r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Next":"B"},"B":{"Type":"Succeed"}}}"#,
                "roleArn": ROLE_ARN,
            }),
        ),
    ];

    for (case, request) in cases {
        let created = emulator.ok("CreateStateMachine", &request).await;
        let name = string(&request, "name");
        let arn = string(&created, "stateMachineArn");
        assert_eq!(
            arn,
            format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:{name}"),
            "case {case:?}: expected the ARN of '{name}' in {REGION}, got {arn:?}"
        );
        assert!(
            created.get("stateMachineVersionArn").is_none()
                || created["stateMachineVersionArn"].is_null(),
            "case {case:?}: expected no version ARN without publish, got {created:?}"
        );

        let described = emulator
            .ok("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
            .await;
        assert_eq!(
            described.get("stateMachineArn"),
            created.get("stateMachineArn"),
            "case {case:?}: describe must report the created ARN, got {described:?}"
        );
        assert_eq!(
            described.get("creationDate"),
            created.get("creationDate"),
            "case {case:?}: describe must report the stored creationDate, got {described:?}"
        );
        assert_eq!(
            described.get("name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected name {name:?}, got {described:?}"
        );
        assert_eq!(
            described.get("definition"),
            request.get("definition"),
            "case {case:?}: expected the stored definition, got {described:?}"
        );
        assert_eq!(
            described.get("roleArn").and_then(Value::as_str),
            Some(ROLE_ARN),
            "case {case:?}: expected the stored roleArn, got {described:?}"
        );
        // `type`'s own documentation: "The default is STANDARD."
        let expected_type = request
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("STANDARD");
        assert_eq!(
            described.get("type").and_then(Value::as_str),
            Some(expected_type),
            "case {case:?}: expected type {expected_type:?}, got {described:?}"
        );
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: a created state machine is ACTIVE, got {described:?}"
        );
        // A configuration the create SUPPLIED is reported as supplied; one it did
        // not is reported at its own modelled default, which
        // `tests/all/state_machine_projection.rs` pins over the whole class.
        let expected_logging = discriminated(
            "LoggingConfiguration",
            request.get("loggingConfiguration").cloned().unwrap_or(
                json!({ "level": "OFF", "includeExecutionData": false }),
            ),
        );
        assert_eq!(
            described.get("loggingConfiguration"),
            Some(&expected_logging),
            "case {case:?}: expected loggingConfiguration {expected_logging}, got {described:?}"
        );
        let expected_tracing = discriminated(
            "TracingConfiguration",
            request
                .get("tracingConfiguration")
                .cloned()
                .unwrap_or(json!({ "enabled": false })),
        );
        assert_eq!(
            described.get("tracingConfiguration"),
            Some(&expected_tracing),
            "case {case:?}: expected tracingConfiguration {expected_tracing}, got {described:?}"
        );
    }
}

/// A describe of an ARN nothing created is the modelled 400, not a hollow 200.
#[tokio::test]
async fn describe_state_machine_refuses_an_unknown_or_malformed_arn() {
    let emulator = Emulator::new();
    create_machine(&emulator, "present", HELLO_DEFINITION).await;

    // Rows vary along the SHAPE of the ARN: well formed but naming nothing, naming
    // another region/account/partition, a qualifier this emulator never mints, and
    // text that is not an ARN of this service at all. An ARN of another REGION is a
    // defect in the request, not a resource that is absent — see
    // `tests/all/arn_validation.rs` for that rule over every operation.
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a well-formed ARN naming no state machine",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:absent"),
            "StateMachineDoesNotExist",
        ),
        (
            "another region",
            "arn:aws:states:eu-west-1:000000000000:stateMachine:present".to_string(),
            "InvalidArn",
        ),
        (
            "another account",
            format!("arn:aws:states:{REGION}:999999999999:stateMachine:present"),
            "StateMachineDoesNotExist",
        ),
        (
            "another partition",
            format!("arn:aws-cn:states:{REGION}:{ACCOUNT}:stateMachine:present"),
            "StateMachineDoesNotExist",
        ),
        (
            "a version qualifier no publish minted",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:present:1"),
            "StateMachineDoesNotExist",
        ),
        (
            "an activity ARN",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:present"),
            "InvalidArn",
        ),
        (
            "an ARN of another service",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:present"),
            "InvalidArn",
        ),
        ("not an ARN at all", "present".to_string(), "InvalidArn"),
        (
            "an ARN with no resource field",
            "arn:aws:states:us-west-2:000000000000".to_string(),
            "InvalidArn",
        ),
    ];

    for (case, arn, expected) in cases {
        let (status, code) = emulator
            .err("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected}, got {status} {code}"
        );
    }
}

// ---------------------------------------------------------------------------
// Create: the definition and the name are checked, and the create is idempotent
// ---------------------------------------------------------------------------

/// A definition that breaks one of the Amazon States Language's own structural
/// rules is refused with `InvalidDefinition`, and a definition that obeys them is
/// accepted even when it uses states this emulator does not execute.
#[tokio::test]
async fn create_state_machine_checks_the_definition_structure() {
    let emulator = Emulator::new();

    // Rows vary along WHICH structural rule is broken.
    let refused: Vec<(&str, &str)> = vec![
        ("not JSON", "{ this is not json"),
        ("a JSON array", r#"[{"Type":"Pass"}]"#),
        ("no StartAt", r#"{"States":{"A":{"Type":"Succeed"}}}"#),
        ("a non-string StartAt", r#"{"StartAt":7,"States":{"A":{"Type":"Succeed"}}}"#),
        ("no States", r#"{"StartAt":"A"}"#),
        ("an empty States map", r#"{"StartAt":"A","States":{}}"#),
        (
            "a StartAt no state declares",
            r#"{"StartAt":"Missing","States":{"A":{"Type":"Succeed"}}}"#,
        ),
        (
            "a state that is not an object",
            r#"{"StartAt":"A","States":{"A":"Succeed"}}"#,
        ),
        (
            "a state with no Type",
            r#"{"StartAt":"A","States":{"A":{"End":true}}}"#,
        ),
        (
            "a Next no state declares",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Next":"Nowhere"}}}"#,
        ),
        (
            "a non-string Next",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Next":4}}}"#,
        ),
        (
            "a Succeed state carrying Next",
            r#"{"StartAt":"A","States":{"A":{"Type":"Succeed","Next":"A"}}}"#,
        ),
        (
            "a Fail state carrying Next",
            r#"{"StartAt":"A","States":{"A":{"Type":"Fail","Next":"A"}}}"#,
        ),
        (
            "a Pass state with neither Next nor End",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass"}}}"#,
        ),
        (
            "a Wait state with neither Next nor End",
            r#"{"StartAt":"A","States":{"A":{"Type":"Wait","Seconds":1}}}"#,
        ),
        (
            "a Task state with neither Next nor End",
            r#"{"StartAt":"A","States":{"A":{"Type":"Task","Resource":"arn:aws:lambda:us-west-2:000000000000:function:f"}}}"#,
        ),
        (
            "a Map state with neither Next nor End",
            r#"{"StartAt":"A","States":{"A":{"Type":"Map","ItemProcessor":{"StartAt":"B","States":{"B":{"Type":"Succeed"}}}}}}"#,
        ),
        (
            "a Parallel state with neither Next nor End",
            r#"{"StartAt":"A","States":{"A":{"Type":"Parallel","Branches":[{"StartAt":"B","States":{"B":{"Type":"Succeed"}}}]}}}"#,
        ),
    ];

    for (index, (case, definition)) in refused.into_iter().enumerate() {
        let (status, code) = emulator
            .err(
                "CreateStateMachine",
                &json!({
                    "name": format!("bad-{index}"),
                    "definition": definition,
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected 400 InvalidDefinition, got {status} {code}"
        );
    }

    // A caller-sized nesting depth is bounded by the JSON parser rather than by the
    // walk: `serde_json` refuses a document nested past its own recursion limit, and
    // the operation reports that as InvalidDefinition instead of letting it reach the
    // interpreter. Rows vary along the depth, either side of that limit.
    for (case, depth, expected) in [
        ("nested within the parser's limit", 32_usize, 200_u16),
        ("nested past the parser's limit", 400, 400),
    ] {
        let definition = format!(
            r#"{{"StartAt":"A","States":{{"A":{{"Type":"Pass","Result":{}{},"End":true}}}}}}"#,
            "[".repeat(depth),
            "]".repeat(depth)
        );
        let (status, body) = emulator
            .call(
                "CreateStateMachine",
                &json!({
                    "name": format!("deep-{depth}"),
                    "definition": definition,
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with body {body:?}"
        );
        if status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidDefinition",
                "case {case:?}: a definition the parser refuses is InvalidDefinition, got {body:?}"
            );
        }
    }

    // Valid ASL is accepted whether or not this emulator EXECUTES every state it
    // uses: the create succeeds, and what cannot be run surfaces as a FAILED
    // execution instead.
    let accepted: Vec<(&str, &str)> = vec![
        (
            "a Task state whose Resource names no integration call",
            r#"{"StartAt":"A","States":{"A":{"Type":"Task","Resource":"arn:aws:lambda:us-west-2:000000000000:function:f","End":true}}}"#,
        ),
        (
            "a Choice state",
            r#"{"StartAt":"A","States":{"A":{"Type":"Choice","Choices":[{"Variable":"$.x","NumericEquals":1,"Next":"B"}],"Default":"B"},"B":{"Type":"Succeed"}}}"#,
        ),
        (
            "a Pass state with a JSONPath member",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","ResultPath":"$.out","End":true}}}"#,
        ),
        (
            "a definition whose Next chain is a cycle out of which a state ends",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Next":"B"},"B":{"Type":"Choice","Choices":[{"Variable":"$.go","IsPresent":true,"Next":"A"}],"Default":"Done"},"Done":{"Type":"Succeed"}}}"#,
        ),
    ];
    for (index, (case, definition)) in accepted.into_iter().enumerate() {
        let created = emulator
            .call(
                "CreateStateMachine",
                &json!({
                    "name": format!("unrun-{index}"),
                    "definition": definition,
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        assert_eq!(
            created.0, 200,
            "case {case:?}: valid ASL must be accepted, got {} {:?}",
            created.0, created.1
        );
    }
}

/// The bounds on what a definition may carry, tested at each published bound and
/// one past it: a state name, a `Fail` state's `Error` and `Cause`, and the payload
/// a state's output may carry.
#[tokio::test]
async fn definition_and_payload_bounds_are_enforced_at_their_edges() {
    let emulator = Emulator::new();

    // Rows vary along which bound is tested and whether the value sits on it or one
    // unit past it. Each bound's source is the constant's own documentation: the
    // Amazon States Language's 80-character state name, and the modelled
    // `SensitiveError` (0..256) and `SensitiveCause` (0..32768) maxima.
    let cases: Vec<(&str, String, u16)> = vec![
        (
            "a state name at the 80-character bound",
            format!(
                r#"{{"StartAt":"{0}","States":{{"{0}":{{"Type":"Succeed"}}}}}}"#,
                "n".repeat(MAX_STATE_NAME_CHARS)
            ),
            200,
        ),
        (
            "a state name one character past the bound",
            format!(
                r#"{{"StartAt":"{0}","States":{{"{0}":{{"Type":"Succeed"}}}}}}"#,
                "n".repeat(MAX_STATE_NAME_CHARS + 1)
            ),
            400,
        ),
        (
            // A nested state machine's state names reach the same execution history,
            // so the same bound holds for them, at the same create-time refusal.
            "a nested state name at the bound",
            format!(
                r#"{{"StartAt":"M","States":{{"M":{{"Type":"Map","ItemProcessor":{{"StartAt":"{0}","States":{{"{0}":{{"Type":"Succeed"}}}}}},"End":true}}}}}}"#,
                "n".repeat(MAX_STATE_NAME_CHARS)
            ),
            200,
        ),
        (
            "a nested state name one character past the bound",
            format!(
                r#"{{"StartAt":"M","States":{{"M":{{"Type":"Map","ItemProcessor":{{"StartAt":"{0}","States":{{"{0}":{{"Type":"Succeed"}}}}}},"End":true}}}}}}"#,
                "n".repeat(MAX_STATE_NAME_CHARS + 1)
            ),
            400,
        ),
        (
            "a Parallel branch's state name one character past the bound",
            format!(
                r#"{{"StartAt":"P","States":{{"P":{{"Type":"Parallel","Branches":[{{"StartAt":"{0}","States":{{"{0}":{{"Type":"Succeed"}}}}}}],"End":true}}}}}}"#,
                "n".repeat(MAX_STATE_NAME_CHARS + 1)
            ),
            400,
        ),
        (
            "a Fail Error at its bound",
            format!(
                r#"{{"StartAt":"A","States":{{"A":{{"Type":"Fail","Error":"{}"}}}}}}"#,
                "e".repeat(MAX_ERROR_BYTES)
            ),
            200,
        ),
        (
            "a Fail Error one byte past its bound",
            format!(
                r#"{{"StartAt":"A","States":{{"A":{{"Type":"Fail","Error":"{}"}}}}}}"#,
                "e".repeat(MAX_ERROR_BYTES + 1)
            ),
            400,
        ),
        (
            "a Fail Cause at its bound",
            format!(
                r#"{{"StartAt":"A","States":{{"A":{{"Type":"Fail","Cause":"{}"}}}}}}"#,
                "c".repeat(MAX_CAUSE_BYTES)
            ),
            200,
        ),
        (
            "a Fail Cause one byte past its bound",
            format!(
                r#"{{"StartAt":"A","States":{{"A":{{"Type":"Fail","Cause":"{}"}}}}}}"#,
                "c".repeat(MAX_CAUSE_BYTES + 1)
            ),
            400,
        ),
    ];

    for (index, (case, definition, expected)) in cases.into_iter().enumerate() {
        let (status, body) = emulator
            .call(
                "CreateStateMachine",
                &json!({
                    "name": format!("bound-{index}"),
                    "definition": definition,
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with body {body:?}"
        );
        if status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidDefinition",
                "case {case:?}: a definition past a published bound is InvalidDefinition, \
                 got {body:?}"
            );
        }
    }

    // A state's OUTPUT is bounded by what the service reports it through, so a run
    // whose value outgrew that ends with ASL's own data-limit error rather than
    // storing a payload no response could carry. Rows: just inside, just past.
    let payload_cases = [
        ("an output just inside the payload bound", 1_000, "SUCCEEDED"),
        (
            "an output just past the payload bound",
            MAX_PAYLOAD_BYTES + 64,
            "FAILED",
        ),
    ];
    let mut started = Vec::new();
    for (index, (_, size, _)) in payload_cases.iter().enumerate() {
        let definition = format!(
            r#"{{"StartAt":"A","States":{{"A":{{"Type":"Pass","Result":"{}","End":true}}}}}}"#,
            "p".repeat(*size)
        );
        let machine = create_machine(&emulator, &format!("payload-{index}"), &definition).await;
        started.push(start_execution(&emulator, &machine, "only", "{}").await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for ((case, _, expected), execution) in payload_cases.iter().zip(started) {
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some(*expected),
            "case {case:?}: expected status {expected}, got status {:?}",
            described.get("status")
        );
        if *expected == "FAILED" {
            assert_eq!(
                described.get("error").and_then(Value::as_str),
                Some("States.DataLimitExceeded"),
                "case {case:?}: an over-large output fails with States.DataLimitExceeded, got {:?}",
                described.get("error")
            );
            assert!(
                described.get("output").is_none_or(Value::is_null),
                "case {case:?}: the over-large payload must not be stored, got an output"
            );
        }
    }
}

/// The documented name rule, over a table of characters the rule names and
/// characters it allows, on both operations that take a `name`.
#[tokio::test]
async fn names_are_checked_against_the_documented_character_rule() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "name-rule", HELLO_DEFINITION).await;

    // From the `name` member's own documentation: a name must not contain white
    // space, brackets `< > { } [ ]`, wildcard characters `? *`, the special
    // characters `" # % \ ^ | ~ ` $ & , ; : /` or control characters. `-` and `_`
    // are the characters it explicitly recommends, and `.` is neither forbidden nor
    // a control character.
    let refused = [
        ("a space", "two words"),
        ("a tab", "two\twords"),
        ("an angle bracket", "a<b"),
        ("a brace", "a{b}"),
        ("a square bracket", "a[0]"),
        ("a question mark", "a?b"),
        ("a star", "a*b"),
        ("a quote", "a\"b"),
        ("a hash", "a#b"),
        ("a percent", "a%b"),
        ("a backslash", "a\\b"),
        ("a caret", "a^b"),
        ("a pipe", "a|b"),
        ("a tilde", "a~b"),
        ("a backtick", "a`b"),
        ("a dollar", "a$b"),
        ("an ampersand", "a&b"),
        ("a comma", "a,b"),
        ("a semicolon", "a;b"),
        ("a colon", "a:b"),
        ("a slash", "a/b"),
        ("a control character", "a\u{1}b"),
        ("a C1 control character", "a\u{85}b"),
    ];
    for (case, name) in refused {
        let (status, code) = emulator
            .err(
                "CreateStateMachine",
                &json!({ "name": name, "definition": HELLO_DEFINITION, "roleArn": ROLE_ARN }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidName"),
            "case {case:?}: CreateStateMachine must refuse {name:?}, got {status} {code}"
        );

        let (status, code) = emulator
            .err(
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": name }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidName"),
            "case {case:?}: StartExecution must refuse {name:?}, got {status} {code}"
        );
    }

    let accepted = [
        ("digits and letters", "Run42"),
        ("a hyphen", "my-machine"),
        ("an underscore", "my_machine"),
        ("a dot", "my.machine"),
        ("a non-ASCII letter", "café"),
        ("the longest modelled name", &"n".repeat(80)),
    ];
    for (case, name) in accepted {
        let (status, body) = emulator
            .call(
                "CreateStateMachine",
                &json!({ "name": name, "definition": HELLO_DEFINITION, "roleArn": ROLE_ARN }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {name:?} must be accepted, got {status} {body:?}"
        );
    }
}

/// `CreateStateMachine`'s documented idempotency: the check reads the name and the
/// configuration, ignores `roleArn` and `tags`, and anything else is a conflict.
#[tokio::test]
async fn create_state_machine_is_idempotent_on_the_members_the_check_reads() {
    let emulator = Emulator::new();
    let request = json!({
        "name": "idem",
        "definition": HELLO_DEFINITION,
        "roleArn": ROLE_ARN,
        "type": "STANDARD",
        "tags": [{ "key": "a", "value": "1" }],
    });
    let first = emulator.ok("CreateStateMachine", &request).await;

    // Rows the check treats as the SAME request: an exact repeat, and a repeat whose
    // `roleArn` or `tags` differ ("Step Functions will ignore these differences").
    let same: Vec<(&str, Value)> = vec![
        ("an exact repeat", request.clone()),
        (
            "a different roleArn",
            json!({
                "name": "idem",
                "definition": HELLO_DEFINITION,
                "roleArn": "arn:aws:iam::000000000000:role/Another",
                "type": "STANDARD",
                "tags": [{ "key": "a", "value": "1" }],
            }),
        ),
        (
            "different tags",
            json!({
                "name": "idem",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "type": "STANDARD",
                "tags": [{ "key": "b", "value": "2" }],
            }),
        ),
        (
            "an omitted type, which defaults to the stored STANDARD",
            json!({
                "name": "idem",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
            }),
        ),
    ];
    for (case, repeat) in same {
        let again = emulator.ok("CreateStateMachine", &repeat).await;
        assert_eq!(
            again, first,
            "case {case:?}: an idempotent repeat must answer the first response"
        );
    }

    // And the stored record kept the FIRST request's roleArn: the repeats above must
    // not have updated it.
    let described = emulator
        .ok(
            "DescribeStateMachine",
            &json!({ "stateMachineArn": string(&first, "stateMachineArn") }),
        )
        .await;
    assert_eq!(
        described.get("roleArn").and_then(Value::as_str),
        Some(ROLE_ARN),
        "an idempotent repeat must not update roleArn, got {described:?}"
    );

    // Rows the check treats as a CONFLICT: each differs in a member the check reads.
    let conflicting: Vec<(&str, Value)> = vec![
        (
            "a different definition",
            json!({
                "name": "idem",
                "definition": r#"{"StartAt":"Done","States":{"Done":{"Type":"Succeed"}}}"#,
                "roleArn": ROLE_ARN,
            }),
        ),
        (
            "a different type",
            json!({
                "name": "idem",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "type": "EXPRESS",
            }),
        ),
        (
            "an added tracingConfiguration",
            json!({
                "name": "idem",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "tracingConfiguration": { "enabled": true },
            }),
        ),
        (
            "an added loggingConfiguration",
            json!({
                "name": "idem",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                // A level that logs needs the one destination the member allows:
                // "Limited to size 1. Required, if your log level is not set to OFF."
                "loggingConfiguration": {
                    "level": "ALL",
                    "includeExecutionData": false,
                    "destinations": [{ "cloudWatchLogsLogGroup": { "logGroupArn":
                        format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:/sfn:*") } }],
                },
            }),
        ),
    ];
    for (case, conflict) in conflicting {
        let (status, code) = emulator.err("CreateStateMachine", &conflict).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "StateMachineAlreadyExists"),
            "case {case:?}: expected 400 StateMachineAlreadyExists, got {status} {code}"
        );
    }
}

/// The requests this slice cannot serve are refused with the operation's own
/// modelled error, never with an empty success.
#[tokio::test]
async fn create_state_machine_refuses_what_this_slice_does_not_serve() {
    let emulator = Emulator::new();

    // "if you set versionDescription, but publish to false, this API action throws
    // ValidationException".
    let (status, code) = emulator
        .err(
            "CreateStateMachine",
            &json!({
                "name": "described-without-publish",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "versionDescription": "v1",
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "versionDescription without publish must be refused, got {status} {code}"
    );

    // More tags than a resource accepts is the modelled TooManyTags.
    let tags: Vec<Value> = (0..=MAX_TAGS)
        .map(|index| json!({ "key": format!("k{index}"), "value": "v" }))
        .collect();
    let (status, code) = emulator
        .err(
            "CreateStateMachine",
            &json!({
                "name": "over-tagged",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "tags": tags,
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "TooManyTags"),
        "more than {MAX_TAGS} tags must be refused, got {status} {code}"
    );
}

// ---------------------------------------------------------------------------
// The interpreter: the outcome follows from the definition and the input
// ---------------------------------------------------------------------------

/// One row of the interpreter table.
struct RunCase {
    case: &'static str,
    definition: &'static str,
    input: &'static str,
    status: &'static str,
    /// The expected `output`, as JSON, or `None` when the execution must report none.
    output: Option<&'static str>,
    /// The expected `error`, or `None` when the execution must report none.
    error: Option<&'static str>,
}

/// An execution's terminal status, `output` and `error` are COMPUTED from the
/// definition and the input, over a table that varies the state types, the chain
/// length, the shape of the input and whether the definition can be run at all.
#[tokio::test]
async fn an_execution_runs_the_definition_to_its_own_outcome() {
    let cases = [
        RunCase {
            case: "a Pass state with a static Result",
            definition: HELLO_DEFINITION,
            input: r#"{"a":1}"#,
            status: "SUCCEEDED",
            output: Some(r#"{"msg":"hi"}"#),
            error: None,
        },
        RunCase {
            case: "a Pass state with no Result carries its input through",
            definition: r#"{"StartAt":"Echo","States":{"Echo":{"Type":"Pass","End":true}}}"#,
            input: r#"{"a":1,"b":[2,3]}"#,
            status: "SUCCEEDED",
            output: Some(r#"{"a":1,"b":[2,3]}"#),
            error: None,
        },
        RunCase {
            case: "a Pass state carrying a JSON scalar through",
            definition: r#"{"StartAt":"Echo","States":{"Echo":{"Type":"Pass","End":true}}}"#,
            input: "42",
            status: "SUCCEEDED",
            output: Some("42"),
            error: None,
        },
        RunCase {
            case: "a chain of Pass states ends on the last Result",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Result":{"step":"a"},"Next":"B"},"B":{"Type":"Pass","Result":{"step":"b"},"Next":"C"},"C":{"Type":"Succeed"}}}"#,
            input: "{}",
            status: "SUCCEEDED",
            output: Some(r#"{"step":"b"}"#),
            error: None,
        },
        RunCase {
            case: "a Succeed state alone",
            definition: r#"{"StartAt":"Done","States":{"Done":{"Type":"Succeed"}}}"#,
            input: r#"[1,2]"#,
            status: "SUCCEEDED",
            output: Some("[1,2]"),
            error: None,
        },
        RunCase {
            case: "a Wait state of zero seconds then Succeed",
            definition: r#"{"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":0,"Next":"Done"},"Done":{"Type":"Succeed"}}}"#,
            input: r#"{"k":"v"}"#,
            status: "SUCCEEDED",
            output: Some(r#"{"k":"v"}"#),
            error: None,
        },
        RunCase {
            case: "a Wait state whose Timestamp has already passed",
            definition: r#"{"StartAt":"W","States":{"W":{"Type":"Wait","Timestamp":"2000-01-01T00:00:00Z","End":true}}}"#,
            input: "{}",
            status: "SUCCEEDED",
            output: Some("{}"),
            error: None,
        },
        RunCase {
            case: "a Fail state with an Error and a Cause",
            definition: r#"{"StartAt":"Bang","States":{"Bang":{"Type":"Fail","Error":"MyError","Cause":"it went wrong"}}}"#,
            input: "{}",
            status: "FAILED",
            output: None,
            error: Some("MyError"),
        },
        RunCase {
            case: "a Fail state with no Error",
            definition: r#"{"StartAt":"Bang","States":{"Bang":{"Type":"Fail"}}}"#,
            input: "{}",
            status: "FAILED",
            output: None,
            error: None,
        },
        RunCase {
            case: "a Pass state that reaches a Fail state",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Next":"Bang"},"Bang":{"Type":"Fail","Error":"Downstream"}}}"#,
            input: "{}",
            status: "FAILED",
            output: None,
            error: Some("Downstream"),
        },
        RunCase {
            case: "a Task state whose Resource names no integration call",
            definition: r#"{"StartAt":"T","States":{"T":{"Type":"Task","Resource":"arn:aws:dynamodb:us-west-2:000000000000:table/t","End":true}}}"#,
            input: "{}",
            status: "FAILED",
            output: None,
            error: Some("States.Runtime"),
        },
        RunCase {
            case: "a Choice state that takes a matching rule",
            definition: r#"{"StartAt":"C","States":{"C":{"Type":"Choice","Choices":[{"Variable":"$.x","NumericEquals":1,"Next":"B"}],"Default":"B"},"B":{"Type":"Succeed"}}}"#,
            input: r#"{"x":1}"#,
            status: "SUCCEEDED",
            output: Some(r#"{"x":1}"#),
            error: None,
        },
        RunCase {
            case: "a Pass state whose ResultPath places its result in its input",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Result":{"ok":true},"ResultPath":"$.out","End":true}}}"#,
            input: r#"{"in":1}"#,
            status: "SUCCEEDED",
            output: Some(r#"{"in":1,"out":{"ok":true}}"#),
            error: None,
        },
        RunCase {
            case: "a Next chain that is a cycle the execution never leaves",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Next":"B"},"B":{"Type":"Choice","Choices":[{"Variable":"$.stop","IsPresent":true,"Next":"Done"}],"Default":"A"},"Done":{"Type":"Succeed"}}}"#,
            input: "{}",
            status: "FAILED",
            output: None,
            error: Some("States.Runtime"),
        },
    ];

    let emulator = Emulator::new();
    // Start every row's execution first, then wait once for the outcomes the clock
    // owes, so the table costs one settle span rather than one per row.
    let mut started = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("run-{index}"), case.definition).await;
        let execution = start_execution(&emulator, &machine, "only", case.input).await;
        started.push(execution);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (case, execution_arn) in cases.iter().zip(started) {
        let described = describe_execution(&emulator, &execution_arn).await;
        let label = case.case;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some(case.status),
            "case {label:?}: expected status {}, got {described:?}",
            case.status
        );
        assert!(
            described.get("stopDate").is_some_and(|date| !date.is_null()),
            "case {label:?}: a terminal execution reports a stopDate, got {described:?}"
        );
        // `input` must come back byte-identical to what StartExecution was given.
        assert_eq!(
            described.get("input").and_then(Value::as_str),
            Some(case.input),
            "case {label:?}: expected the input byte-identical, got {described:?}"
        );
        match case.output {
            Some(expected) => {
                let expected: Value =
                    serde_json::from_str(expected).expect("the expected output is JSON");
                let actual: Value = described
                    .get("output")
                    .and_then(Value::as_str)
                    .map(|text| serde_json::from_str(text).expect("the output is JSON"))
                    .unwrap_or_else(|| panic!("case {label:?}: expected an output in {described:?}"));
                assert_eq!(
                    actual, expected,
                    "case {label:?}: expected output {expected}, got {actual}"
                );
            }
            None => assert!(
                described.get("output").is_none_or(Value::is_null),
                "case {label:?}: a failed execution reports no output, got {described:?}"
            ),
        }
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            case.error,
            "case {label:?}: expected error {:?}, got {described:?}",
            case.error
        );
        if case.error == Some("States.Runtime") {
            let cause = described.get("cause").and_then(Value::as_str).unwrap_or("");
            assert!(
                !cause.is_empty(),
                "case {label:?}: a States.Runtime failure must say what was not run, got {described:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What the clock owes
// ---------------------------------------------------------------------------

/// A started execution is `RUNNING` until the clock makes its outcome due, and the
/// `stopDate` it then reports is the instant the outcome was due — the same value
/// for every later reader.
#[tokio::test]
async fn an_execution_is_running_until_its_outcome_comes_due() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "settling", HELLO_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "run1", r#"{"a":1}"#).await;

    let immediately = describe_execution(&emulator, &execution).await;
    assert_eq!(
        immediately.get("status").and_then(Value::as_str),
        Some("RUNNING"),
        "a caller describing straight after the start still sees RUNNING, got {immediately:?}"
    );
    assert!(
        immediately.get("stopDate").is_none_or(Value::is_null),
        "a running execution reports no stopDate, got {immediately:?}"
    );
    assert!(
        immediately.get("output").is_none_or(Value::is_null),
        "a running execution reports no output, got {immediately:?}"
    );

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let settled = describe_execution(&emulator, &execution).await;
    assert_eq!(
        settled.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "the outcome the clock owed must be reported, got {settled:?}"
    );
    let again = describe_execution(&emulator, &execution).await;
    assert_eq!(
        again, settled,
        "a second read must report the same record, got {again:?}"
    );
    assert_eq!(
        settled.get("startDate"),
        again.get("startDate"),
        "startDate is stable across reads"
    );
}

/// A `Wait` state holds its execution for the duration the definition asks for, so
/// the outcome is not due until that duration has passed — and a duration beyond
/// the longest `Wait` the service waits for is out of range rather than shortened.
#[tokio::test]
async fn a_wait_state_holds_the_execution_for_its_own_duration() {
    let emulator = Emulator::new();

    /// An ISO 8601 instant `offset` seconds from now, so a "future" row does not
    /// depend on the year the suite runs in.
    fn iso_from_now(offset: i64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_secs() as i64;
        aws_smithy_types::DateTime::from_secs(now + offset)
            .fmt(aws_smithy_types::date_time::Format::DateTime)
            .expect("an ISO 8601 rendering")
    }

    // Rows vary along the duration the definition asks for, the form it is given in
    // (`Seconds` and `Timestamp` are the two this emulator honours) and whether it
    // is inside Step Functions' published one-year maximum, tested at the bound and
    // one second past it.
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a Wait of zero seconds is due with the start",
            r#"{"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":0,"End":true}}}"#.to_string(),
            "SUCCEEDED",
        ),
        (
            "a Wait of a minute is still running",
            r#"{"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":60,"End":true}}}"#.to_string(),
            "RUNNING",
        ),
        (
            "a Wait at the one-year maximum is still running",
            format!(
                r#"{{"StartAt":"W","States":{{"W":{{"Type":"Wait","Seconds":{MAX_WAIT_SECONDS},"End":true}}}}}}"#
            ),
            "RUNNING",
        ),
        (
            "a Wait one second past the maximum is out of range",
            format!(
                r#"{{"StartAt":"W","States":{{"W":{{"Type":"Wait","Seconds":{},"End":true}}}}}}"#,
                MAX_WAIT_SECONDS + 1
            ),
            "FAILED",
        ),
        (
            "a Wait for a past Timestamp is due with the start",
            r#"{"StartAt":"W","States":{"W":{"Type":"Wait","Timestamp":"2001-02-03T04:05:06Z","End":true}}}"#.to_string(),
            "SUCCEEDED",
        ),
        (
            "a Wait for a Timestamp inside the maximum is still running",
            format!(
                r#"{{"StartAt":"W","States":{{"W":{{"Type":"Wait","Timestamp":"{}","End":true}}}}}}"#,
                iso_from_now(200 * 86_400)
            ),
            "RUNNING",
        ),
        (
            "a Wait for a Timestamp past the maximum is out of range",
            r#"{"StartAt":"W","States":{"W":{"Type":"Wait","Timestamp":"2999-01-01T00:00:00Z","End":true}}}"#.to_string(),
            "FAILED",
        ),
        (
            "two Waits add up",
            r#"{"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":0,"Next":"X"},"X":{"Type":"Wait","Seconds":120,"End":true}}}"#.to_string(),
            "RUNNING",
        ),
    ];

    let mut started = Vec::new();
    for (index, (_, definition, _)) in cases.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("wait-{index}"), definition).await;
        started.push(start_execution(&emulator, &machine, "only", "{}").await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for ((case, _, expected), execution) in cases.iter().zip(started) {
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some(*expected),
            "case {case:?}: expected status {expected}, got {described:?}"
        );
        if *expected == "FAILED" {
            // An out-of-range wait is the interpreter's own failure, not a shortened
            // wait that reports a success the definition does not describe.
            assert_eq!(
                described.get("error").and_then(Value::as_str),
                Some("States.Runtime"),
                "case {case:?}: an out-of-range wait fails with States.Runtime, got {described:?}"
            );
        }
    }
}

/// `StartExecution`'s documented idempotency for STANDARD workflows: the same name
/// and input while the execution runs answers the first response; a closed
/// execution or a different input is `ExecutionAlreadyExists`.
#[tokio::test]
async fn start_execution_is_idempotent_only_while_the_execution_runs() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "idem-exec", HELLO_DEFINITION).await;
    let request = json!({ "stateMachineArn": machine, "name": "run1", "input": r#"{"a":1}"# });

    let first = emulator.ok("StartExecution", &request).await;
    let repeat = emulator.ok("StartExecution", &request).await;
    assert_eq!(
        repeat, first,
        "the same name and input while running must answer the first response"
    );
    assert_eq!(
        string(&first, "executionArn"),
        format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:idem-exec:run1"),
        "the execution ARN names the state machine and the execution"
    );

    // A different input under the same name is a conflict even while it runs.
    let (status, code) = emulator
        .err(
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "run1", "input": r#"{"a":2}"# }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ExecutionAlreadyExists"),
        "a different input under a running name must conflict, got {status} {code}"
    );

    // Once the execution has closed, the name cannot be reused.
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let (status, code) = emulator.err("StartExecution", &request).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ExecutionAlreadyExists"),
        "a closed execution's name must conflict, got {status} {code}"
    );

    // Only one execution was ever stored under that name.
    let listed = emulator
        .ok("ListExecutions", &json!({ "stateMachineArn": machine }))
        .await;
    assert_eq!(
        listed["executions"].as_array().map(Vec::len),
        Some(1),
        "the repeats must not have stored a second execution, got {listed:?}"
    );
}

/// An omitted execution name is minted by the service, and the record is readable
/// back under the ARN it reported.
#[tokio::test]
async fn start_execution_mints_a_name_when_the_request_omits_one() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "unnamed", HELLO_DEFINITION).await;

    let mut arns = Vec::new();
    for _ in 0..3 {
        let started = emulator
            .ok("StartExecution", &json!({ "stateMachineArn": machine }))
            .await;
        arns.push(string(&started, "executionArn"));
    }
    arns.sort();
    arns.dedup();
    assert_eq!(
        arns.len(),
        3,
        "each start with no name must mint its own execution, got {arns:?}"
    );

    for arn in &arns {
        let described = describe_execution(&emulator, arn).await;
        let name = string(&described, "name");
        let prefix = format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:unnamed:");
        assert_eq!(
            *arn,
            format!("{prefix}{name}"),
            "the minted name must be the last ARN segment, got {arn:?} and {name:?}"
        );
        // The service documents the minted name as a UUID: 36 characters, 8-4-4-4-12.
        let groups: Vec<usize> = name.split('-').map(str::len).collect();
        assert_eq!(
            groups,
            vec![8, 4, 4, 4, 12],
            "a minted name must be a UUID, got {name:?}"
        );
        // And the input defaults to the empty JSON object.
        assert_eq!(
            described.get("input").and_then(Value::as_str),
            Some("{}"),
            "an omitted input is the empty JSON object, got {described:?}"
        );
    }
}

/// An execution cannot be started on, described from or listed for something that
/// was never created, and an `input` that is not JSON is refused.
#[tokio::test]
async fn execution_operations_refuse_what_was_never_stored() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "present", HELLO_DEFINITION).await;
    let absent_machine = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:absent");
    let absent_execution = format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:present:absent");

    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "StartExecution on an unknown state machine",
            "StartExecution",
            json!({ "stateMachineArn": absent_machine }),
            "StateMachineDoesNotExist",
        ),
        (
            "StartExecution on a malformed ARN",
            "StartExecution",
            json!({ "stateMachineArn": "not-an-arn" }),
            "InvalidArn",
        ),
        (
            "DescribeExecution on an unknown execution",
            "DescribeExecution",
            json!({ "executionArn": absent_execution }),
            "ExecutionDoesNotExist",
        ),
        (
            "DescribeExecution on a state machine ARN",
            "DescribeExecution",
            json!({ "executionArn": machine }),
            "InvalidArn",
        ),
        (
            "StopExecution on an unknown execution",
            "StopExecution",
            json!({ "executionArn": absent_execution }),
            "ExecutionDoesNotExist",
        ),
        (
            "StopExecution on a malformed ARN",
            "StopExecution",
            json!({ "executionArn": "nope" }),
            "InvalidArn",
        ),
        (
            "ListExecutions on an unknown state machine",
            "ListExecutions",
            json!({ "stateMachineArn": absent_machine }),
            "StateMachineDoesNotExist",
        ),
        (
            "ListExecutions on a malformed ARN",
            "ListExecutions",
            json!({ "stateMachineArn": "nope" }),
            "InvalidArn",
        ),
        (
            "ListExecutions with neither selector",
            "ListExecutions",
            json!({}),
            "ValidationException",
        ),
        (
            "ListExecutions with both selectors",
            "ListExecutions",
            json!({
                "stateMachineArn": machine,
                "mapRunArn": format!("arn:aws:states:{REGION}:{ACCOUNT}:mapRun:present/x:1"),
            }),
            "ValidationException",
        ),
        (
            "ListExecutions with a page token this service never issued",
            "ListExecutions",
            json!({ "stateMachineArn": machine, "nextToken": "not-a-token" }),
            "InvalidToken",
        ),
        (
            "StartExecution with an input that is not JSON",
            "StartExecution",
            json!({ "stateMachineArn": machine, "name": "bad-input", "input": "{oops" }),
            "InvalidExecutionInput",
        ),
    ];

    for (case, operation, body, expected) in cases {
        let (status, code) = emulator.err(operation, &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected}, got {status} {code}"
        );
    }

    // A Map Run ARN the store holds no record for is a `ResourceNotFound`.
    let (status, code) = emulator
        .err(
            "ListExecutions",
            &json!({ "mapRunArn": format!("arn:aws:states:{REGION}:{ACCOUNT}:mapRun:present/x:1") }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ResourceNotFound"),
        "a map run that does not exist is a ResourceNotFound, got {status} {code}"
    );
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// `ListExecutions` reports the stored executions of one state machine, most recent
/// first, honouring `statusFilter` and paging with `nextToken`.
#[tokio::test]
async fn list_executions_pages_the_stored_executions_newest_first() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "listing", HELLO_DEFINITION).await;
    let other = create_machine(&emulator, "other", HELLO_DEFINITION).await;
    start_execution(&emulator, &other, "elsewhere", "{}").await;

    let names = ["run-1", "run-2", "run-3", "run-4", "run-5"];
    for name in names {
        start_execution(&emulator, &machine, name, "{}").await;
        std::thread::sleep(Duration::from_millis(2));
    }

    // The order is newest first, by startDate while running and by stopDate once
    // terminal; both keys are monotonic in the start order, so the order is the same
    // either way.
    let listed = emulator
        .ok("ListExecutions", &json!({ "stateMachineArn": machine }))
        .await;
    let reported: Vec<String> = listed["executions"]
        .as_array()
        .expect("executions is a list")
        .iter()
        .map(|item| string(item, "name"))
        .collect();
    let mut newest_first: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    newest_first.reverse();
    assert_eq!(
        reported, newest_first,
        "expected the executions newest first, got {reported:?}"
    );
    for item in listed["executions"].as_array().expect("a list") {
        assert_eq!(
            item.get("stateMachineArn").and_then(Value::as_str),
            Some(machine.as_str()),
            "only this state machine's executions are listed, got {item:?}"
        );
        // Deliberately not pinned to RUNNING here: whether an outcome has come due
        // by now depends on how long the five starts above took. The exact
        // RUNNING-until-due window is pinned by
        // `an_execution_is_running_until_its_outcome_comes_due`.
        let status = item.get("status").and_then(Value::as_str);
        assert!(
            matches!(status, Some("RUNNING" | "SUCCEEDED")),
            "a started execution of this definition is RUNNING or SUCCEEDED, got {item:?}"
        );
    }

    // A filter that selects no stored execution is an empty list, never an error.
    // This definition can only succeed, so none of these statuses can ever occur.
    for status in ["FAILED", "ABORTED", "TIMED_OUT"] {
        let filtered = emulator
            .ok(
                "ListExecutions",
                &json!({ "stateMachineArn": machine, "statusFilter": status }),
            )
            .await;
        assert_eq!(
            filtered["executions"].as_array().map(Vec::len),
            Some(0),
            "case {status:?}: no execution of this definition can reach that status, got {filtered:?}"
        );
    }

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let succeeded = emulator
        .ok(
            "ListExecutions",
            &json!({ "stateMachineArn": machine, "statusFilter": "SUCCEEDED" }),
        )
        .await;
    assert_eq!(
        succeeded["executions"].as_array().map(Vec::len),
        Some(names.len()),
        "every execution has succeeded by now, got {succeeded:?}"
    );
    for item in succeeded["executions"].as_array().expect("a list") {
        assert!(
            item.get("stopDate").is_some_and(|date| !date.is_null()),
            "a terminal list item reports a stopDate, got {item:?}"
        );
    }

    // Paging: rows vary along the page size, including one larger than the store.
    for page in [1_usize, 2, 5, 9] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "stateMachineArn": machine, "maxResults": page });
            if let Some(token) = &token {
                request["nextToken"] = Value::String(token.clone());
            }
            let body = emulator.ok("ListExecutions", &request).await;
            let items = body["executions"].as_array().expect("a list");
            assert!(
                items.len() <= page,
                "case page={page}: a page carries at most maxResults items, got {body:?}"
            );
            seen.extend(items.iter().map(|item| string(item, "name")));
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen.len(),
            names.len(),
            "case page={page}: paging must visit every execution exactly once, got {seen:?}"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            names.len(),
            "case page={page}: paging must not repeat an execution, got {seen:?}"
        );
    }
}

/// `ListStateMachines` reports the stored state machines and pages them.
#[tokio::test]
async fn list_state_machines_reports_and_pages_the_stored_records() {
    let emulator = Emulator::new();
    let empty = emulator.ok("ListStateMachines", &json!({})).await;
    assert_eq!(
        empty["stateMachines"].as_array().map(Vec::len),
        Some(0),
        "an empty store lists nothing, got {empty:?}"
    );

    let names = ["sm-a", "sm-b", "sm-c", "sm-d"];
    for name in names {
        create_machine(&emulator, name, HELLO_DEFINITION).await;
    }

    let listed = emulator.ok("ListStateMachines", &json!({})).await;
    let mut reported: Vec<String> = listed["stateMachines"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|item| string(item, "name"))
        .collect();
    reported.sort();
    assert_eq!(
        reported,
        names.to_vec(),
        "every created state machine is listed, got {reported:?}"
    );
    for item in listed["stateMachines"].as_array().expect("a list") {
        assert_eq!(
            item.get("type").and_then(Value::as_str),
            Some("STANDARD"),
            "the listed type comes from the record, got {item:?}"
        );
        assert!(
            item.get("creationDate").is_some_and(|date| !date.is_null()),
            "the listed item carries the stored creationDate, got {item:?}"
        );
    }

    for page in [1_usize, 3, 10] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "maxResults": page });
            if let Some(token) = &token {
                request["nextToken"] = Value::String(token.clone());
            }
            let body = emulator.ok("ListStateMachines", &request).await;
            let items = body["stateMachines"].as_array().expect("a list");
            assert!(
                items.len() <= page,
                "case page={page}: a page carries at most maxResults items, got {body:?}"
            );
            seen.extend(items.iter().map(|item| string(item, "name")));
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        seen.sort();
        assert_eq!(
            seen,
            names.to_vec(),
            "case page={page}: paging must visit every state machine exactly once"
        );
    }
}

// ---------------------------------------------------------------------------
// Stopping
// ---------------------------------------------------------------------------

/// `StopExecution` moves a running execution to `ABORTED` with the caller's
/// `error`/`cause`, and the outcome the interpreter had computed is discarded.
#[tokio::test]
async fn stop_execution_aborts_a_running_execution() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "stopping", HELLO_DEFINITION).await;

    // Rows vary along what the caller supplies, which the doc says may be nothing.
    let cases: Vec<(&str, Value, Option<&str>, Option<&str>)> = vec![
        (
            "an error and a cause",
            json!({ "error": "Cancelled", "cause": "the operator asked" }),
            Some("Cancelled"),
            Some("the operator asked"),
        ),
        ("neither error nor cause", json!({}), None, None),
    ];

    for (index, (case, extra, expected_error, expected_cause)) in cases.into_iter().enumerate() {
        let execution =
            start_execution(&emulator, &machine, &format!("stop-{index}"), "{}").await;
        let mut request = json!({ "executionArn": execution });
        for (key, value) in extra.as_object().expect("an object") {
            request[key] = value.clone();
        }
        let stopped = emulator.ok("StopExecution", &request).await;
        let stop_date = stopped
            .get("stopDate")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a stopDate in {stopped:?}"));

        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("ABORTED"),
            "case {case:?}: a stopped execution is ABORTED, got {described:?}"
        );
        assert_eq!(
            described.get("stopDate"),
            Some(&stop_date),
            "case {case:?}: describe must report the stopDate the stop returned"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            expected_error,
            "case {case:?}: expected error {expected_error:?}, got {described:?}"
        );
        assert_eq!(
            described.get("cause").and_then(Value::as_str),
            expected_cause,
            "case {case:?}: expected cause {expected_cause:?}, got {described:?}"
        );

        // Stopping again reports the same instant rather than moving the record on.
        let again = emulator.ok("StopExecution", &request).await;
        assert_eq!(
            again.get("stopDate"),
            Some(&stop_date),
            "case {case:?}: a second stop reports the first stopDate, got {again:?}"
        );

        // And the outcome the interpreter had computed never lands: the execution was
        // stopped before the clock made it due.
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let settled = describe_execution(&emulator, &execution).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some("ABORTED"),
            "case {case:?}: a stopped execution stays ABORTED, got {settled:?}"
        );
        assert!(
            settled.get("output").is_none_or(Value::is_null),
            "case {case:?}: a stopped execution has no output, got {settled:?}"
        );
    }

    let listed = emulator
        .ok(
            "ListExecutions",
            &json!({ "stateMachineArn": machine, "statusFilter": "ABORTED" }),
        )
        .await;
    assert_eq!(
        listed["executions"].as_array().map(Vec::len),
        Some(2),
        "both stopped executions are listed as ABORTED, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Deleting
// ---------------------------------------------------------------------------

/// `DeleteStateMachine` is asynchronous: the record reports `DELETING` from the
/// call and is gone — with its executions — once the clock makes the removal due.
#[tokio::test]
async fn delete_state_machine_reports_deleting_then_removes_the_record() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "doomed", HELLO_DEFINITION).await;
    let survivor = create_machine(&emulator, "survivor", HELLO_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "run1", "{}").await;

    let deleted = emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": machine }))
        .await;
    assert!(
        deleted.as_object().is_some_and(serde_json::Map::is_empty) || deleted.is_null(),
        "DeleteStateMachine answers an empty body, got {deleted:?}"
    );

    // "It sets the state machine's status to DELETING and begins the deletion
    // process": a caller acting straight afterwards still sees the record.
    let described = emulator
        .ok(
            "DescribeStateMachine",
            &json!({ "stateMachineArn": machine }),
        )
        .await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("DELETING"),
        "a state machine being deleted reports DELETING, got {described:?}"
    );

    // And while it is deleting, neither a new execution nor a re-create is accepted.
    let (status, code) = emulator
        .err("StartExecution", &json!({ "stateMachineArn": machine }))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "StateMachineDeleting"),
        "a deleting state machine cannot start an execution, got {status} {code}"
    );
    let (status, code) = emulator
        .err(
            "CreateStateMachine",
            &json!({
                "name": "doomed",
                "definition": r#"{"StartAt":"Done","States":{"Done":{"Type":"Succeed"}}}"#,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "StateMachineDeleting"),
        "a deleting name cannot be re-created, got {status} {code}"
    );

    std::thread::sleep(PAST_THE_DELETE_DELAY);

    let (status, code) = emulator
        .err(
            "DescribeStateMachine",
            &json!({ "stateMachineArn": machine }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "StateMachineDoesNotExist"),
        "a removed state machine does not exist, got {status} {code}"
    );
    let (status, code) = emulator
        .err("DescribeExecution", &json!({ "executionArn": execution }))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ExecutionDoesNotExist"),
        "a removed state machine's executions go with it, got {status} {code}"
    );

    let listed = emulator.ok("ListStateMachines", &json!({})).await;
    let reported: Vec<String> = listed["stateMachines"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|item| string(item, "stateMachineArn"))
        .collect();
    assert_eq!(
        reported,
        vec![survivor.clone()],
        "only the survivor is still listed, got {reported:?}"
    );

    // Delete is idempotent: the ARN is gone, and the post-condition already holds.
    let repeat = emulator
        .call("DeleteStateMachine", &json!({ "stateMachineArn": machine }))
        .await;
    assert_eq!(
        repeat.0, 200,
        "deleting an absent state machine succeeds, got {} {:?}",
        repeat.0, repeat.1
    );

    // And the name is free again once the removal has happened.
    let recreated = create_machine(&emulator, "doomed", HELLO_DEFINITION).await;
    assert_eq!(
        recreated, machine,
        "the freed name mints the same ARN, got {recreated:?}"
    );
}

/// A qualified ARN naming a Distributed Map state is refused by
/// `DeleteStateMachine` with `ValidationException`, as its own documentation says.
#[tokio::test]
async fn delete_state_machine_refuses_a_distributed_map_arn() {
    let emulator = Emulator::new();
    create_machine(&emulator, "mapped", HELLO_DEFINITION).await;

    let (status, code) = emulator
        .err(
            "DeleteStateMachine",
            &json!({
                "stateMachineArn":
                    format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:mapped/mapStateLabel"),
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "a Distributed Map ARN must be refused, got {status} {code}"
    );

    // A version or alias qualifier names a resource this emulator cannot hold, so it
    // is refused rather than answered with a success that deleted nothing. Rows vary
    // along the qualifier's form.
    for (case, qualifier) in [("a version", "1"), ("an alias", "PROD")] {
        let (status, code) = emulator
            .err(
                "DeleteStateMachine",
                &json!({
                    "stateMachineArn":
                        format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:mapped:{qualifier}"),
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a qualified ARN must be refused, got {status} {code}"
        );
    }

    // And the unqualified ARN of the same state machine is still accepted.
    let (status, body) = emulator
        .call(
            "DeleteStateMachine",
            &json!({
                "stateMachineArn":
                    format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:mapped"),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the unqualified ARN must still delete, got {status} {body:?}"
    );

    let (status, code) = emulator
        .err("DeleteStateMachine", &json!({ "stateMachineArn": "nope" }))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidArn"),
        "a malformed ARN must be refused, got {status} {code}"
    );
}

// ---------------------------------------------------------------------------
// EXPRESS workflows
// ---------------------------------------------------------------------------

/// The three operations that document they do not serve `EXPRESS` executions refuse
/// them, each with its own modelled error, while the state machine itself is served.
#[tokio::test]
async fn express_executions_are_refused_by_the_operations_that_exclude_them() {
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "fast",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "type": "EXPRESS",
            }),
        )
        .await;
    let machine = string(&created, "stateMachineArn");

    let described = emulator
        .ok(
            "DescribeStateMachine",
            &json!({ "stateMachineArn": machine }),
        )
        .await;
    assert_eq!(
        described.get("type").and_then(Value::as_str),
        Some("EXPRESS"),
        "the stored type is reported, got {described:?}"
    );

    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "quick" }),
        )
        .await;
    let execution = string(&started, "executionArn");

    let cases: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "DescribeExecution does not report EXPRESS executions",
            "DescribeExecution",
            json!({ "executionArn": execution }),
            400,
            "ExecutionDoesNotExist",
        ),
        (
            "ListExecutions is not supported by EXPRESS state machines",
            "ListExecutions",
            json!({ "stateMachineArn": machine }),
            400,
            "StateMachineTypeNotSupported",
        ),
        (
            "StopExecution is not supported by EXPRESS state machines",
            "StopExecution",
            json!({ "executionArn": execution }),
            400,
            "InvalidArn",
        ),
    ];
    for (case, operation, body, expected_status, expected) in cases {
        let (status, code) = emulator.err(operation, &body).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected),
            "case {case:?}: expected {expected_status} {expected}, got {status} {code}"
        );
    }

    // "StartExecution isn't idempotent for EXPRESS workflows": there is no same-name
    // hit, and this emulator keys an execution by its ARN, so a reused name is
    // refused rather than overwriting the stored execution.
    let (status, code) = emulator
        .err(
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "quick" }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ExecutionAlreadyExists"),
        "an EXPRESS start under a name in use must not overwrite it, got {status} {code}"
    );
}

/// A state machine name is scoped per region: the same name signed for two regions
/// is two records with two ARNs, and neither answers for the other.
#[tokio::test]
async fn state_machine_names_are_scoped_per_region() {
    let west = Emulator::in_region("us-west-2");
    let east = Emulator::in_region_on(&west, "eu-west-1");

    let west_arn = create_machine(&west, "shared-name", HELLO_DEFINITION).await;
    let east_arn = create_machine(&east, "shared-name", HELLO_DEFINITION).await;
    assert_eq!(
        west_arn, "arn:aws:states:us-west-2:000000000000:stateMachine:shared-name",
        "the us-west-2 create must mint a us-west-2 ARN, got {west_arn:?}"
    );
    assert_eq!(
        east_arn, "arn:aws:states:eu-west-1:000000000000:stateMachine:shared-name",
        "the eu-west-1 create must mint its own ARN, got {east_arn:?}"
    );

    // Both records exist side by side, each describable under its own ARN.
    for (case, emulator, arn) in [
        ("the us-west-2 record", &west, &west_arn),
        ("the eu-west-1 record", &east, &east_arn),
    ] {
        let described = emulator
            .ok("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
            .await;
        assert_eq!(
            described.get("stateMachineArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: each ARN describes its own record, got {described:?}"
        );
    }
    // A regional endpoint lists only its own region's records.
    for (case, emulator, arn) in [
        ("the us-west-2 endpoint", &west, &west_arn),
        ("the eu-west-1 endpoint", &east, &east_arn),
    ] {
        let listed = emulator.ok("ListStateMachines", &json!({})).await;
        let reported: Vec<String> = listed["stateMachines"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|item| string(item, "stateMachineArn"))
            .collect();
        assert_eq!(
            reported,
            vec![arn.clone()],
            "case {case:?}: a regional endpoint lists only its own region, got {reported:?}"
        );
    }
}
