// SPDX-License-Identifier: CC0-1.0

//! Run Command end to end: `SendCommand` resolving a document and the managed nodes a
//! send names or describes, and `GetCommandInvocation` reporting back the invocation that
//! send persisted.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol").
//!
//! The node inventory is seeded through the service's own state
//! (`state::ComAmazonawsSsmAmazonssmState::insert_managed_node`, the one node store the
//! node describes read too), because a node enters it only by REGISTERING — an SSM Agent
//! installed on an instance, or a hybrid node activated with an activation code — and no
//! public API of this service registers one. Seeding it is what lets these cases prove a
//! send is resolved against persisted state: the nodes that are registered are the nodes a
//! command gets an invocation for, per Region, and nothing else.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — document, document version, document spelling, parameter set, node
//! kind, node platform, node state, Region, target key, target value, target count, node
//! count — rather than the single send the recorded units happened to make.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::state::{
    commands, ComAmazonawsSsmAmazonssmState, ManagedNodeRecord, RESOURCE_TYPE_EC2_INSTANCE,
    RESOURCE_TYPE_MANAGED_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for unless a case says otherwise. Deliberately not the
/// emulator's default, so a command served here proves the Region came from the request's
/// SigV4 credential scope.
const REGION: &str = "us-west-2";

/// A second Region, to pin that a node is reachable only through the Region it registered
/// in and a command only through the Region it was sent to.
const OTHER_REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AmazonSSM";

/// An online EC2 node running Linux (`InstanceId` admits an 8-character suffix).
const LINUX: &str = "i-0abc1234";

/// An online EC2 node running Windows (`InstanceId` admits a 17-character suffix).
const WINDOWS: &str = "i-0123456789abcdef0";

/// An online hybrid node running macOS (`mi-` and 17 characters).
const HYBRID: &str = "mi-01234567890123456";

/// A registered node whose agent has stopped reporting, so it cannot take a command.
const OFFLINE: &str = "i-0dead111";

/// A well-formed node id no node is registered under.
const UNREGISTERED: &str = "i-0dead999";

/// A node registered only in [`OTHER_REGION`], so a send through [`REGION`] resolves
/// nothing for it.
const OTHER_ONLY: &str = "i-0eeee222";

/// A command document Systems Manager publishes whose steps are restricted to Linux and
/// macOS, and which declares one parameter with no default (`commands`).
const SHELL: &str = "AWS-RunShellScript";

/// A command document Systems Manager publishes whose step is restricted to Windows.
const POWERSHELL: &str = "AWS-RunPowerShellScript";

/// A command document Systems Manager publishes that declares no step precondition, so it
/// runs on every platform, and whose every parameter has a default.
const AGENT: &str = "AWS-UpdateSSMAgent";

/// A step of [`SHELL`], from that document's content (`mainSteps[].name`).
const SHELL_STEP: &str = "runShellScriptOnLinux";

/// A router with just this service mounted, and the engine it was mounted on — the handle
/// a case seeds the node inventory through.
fn service() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope below, which is where the Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Call `operation` signed for `region`, returning `(status, body)`.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/ssm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
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
    // The error shape travels in `__type`; fold in the header the framework also sets so
    // one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Call `operation` signed for [`REGION`].
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// Send a command signed for [`REGION`].
async fn send(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "SendCommand", body).await
}

/// The `Command` of a send that must have succeeded.
async fn sent(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, parsed) = send(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {parsed:?}"
    );
    parsed["Command"].clone()
}

/// Read one node's invocation of a command, signed for [`REGION`].
async fn invocation(
    router: &MultiServiceRouter,
    command_id: &str,
    instance_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GetCommandInvocation",
        &json!({"CommandId": command_id, "InstanceId": instance_id}),
    )
    .await
}

/// The wire error code of a failure body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// The refusal message of a failure body.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// A string member of a response body.
fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or("<unset>").to_string()
}

/// How many commands the store holds, to pin that a refused send persisted nothing.
fn stored_commands(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<ComAmazonawsSsmAmazonssmState>()
        .lock()
        .commands
        .len()
}

/// Register `record` in `region` the way a node's own registration would.
fn register(engine: &Arc<engine::InMemoryEngine>, region: &str, record: ManagedNodeRecord) {
    assert!(
        engine
            .state::<ComAmazonawsSsmAmazonssmState>()
            .lock()
            .insert_managed_node(region, record),
        "a seeded node must carry the node id it is stored under"
    );
}

/// One registered node: its id, kind, platform, ping status and tags.
fn node(
    id: &str,
    resource_type: &str,
    platform: &str,
    ping_status: &str,
    tags: &[(&str, &str)],
) -> ManagedNodeRecord {
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut record = ManagedNodeRecord::registering(id, resource_type, at);
    record.properties.platform_type = Some(platform.to_string());
    record.properties.ping_status = Some(ping_status.to_string());
    record.properties.computer_name = Some(format!("{id}.compute.internal"));
    record.tags = tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<BTreeMap<String, String>>();
    record
}

/// The inventory every suite below sends against: three online nodes spanning node kind,
/// platform and tags, one node whose agent has stopped reporting, and one node of another
/// Region.
fn seed_inventory(engine: &Arc<engine::InMemoryEngine>) {
    register(
        engine,
        REGION,
        node(
            LINUX,
            RESOURCE_TYPE_EC2_INSTANCE,
            "Linux",
            "Online",
            &[("env", "dev"), ("Name", "web")],
        ),
    );
    register(
        engine,
        REGION,
        node(
            WINDOWS,
            RESOURCE_TYPE_EC2_INSTANCE,
            "Windows",
            "Online",
            &[("env", "prod")],
        ),
    );
    register(
        engine,
        REGION,
        node(
            HYBRID,
            RESOURCE_TYPE_MANAGED_INSTANCE,
            "MacOS",
            "Online",
            &[("env", "dev")],
        ),
    );
    register(
        engine,
        REGION,
        node(
            OFFLINE,
            RESOURCE_TYPE_EC2_INSTANCE,
            "Linux",
            "ConnectionLost",
            &[("env", "dev")],
        ),
    );
    register(
        engine,
        OTHER_REGION,
        node(
            OTHER_ONLY,
            RESOURCE_TYPE_EC2_INSTANCE,
            "Linux",
            "Online",
            &[("env", "dev")],
        ),
    );
}

/// One row of the named-node table: the send to make, the nodes it must resolve to, and
/// the defaults or echoes the command it mints must report.
struct SendRow {
    case: &'static str,
    body: Value,
    nodes: Vec<&'static str>,
    version: &'static str,
    concurrency: &'static str,
    errors: &'static str,
    timeout: i32,
}

/// A send that names the nodes to run on persists the command and one invocation per node,
/// and the response reports what was persisted.
#[tokio::test]
async fn a_send_to_named_nodes_persists_a_command_and_one_invocation_per_node() {
    let (router, engine) = service();
    seed_inventory(&engine);

    // One row per shape of send: the node kinds and platforms the inventory holds, one and
    // several nodes at a time, with and without the members that carry a default.
    let rows = vec![
        SendRow {
            case: "one linux node, defaults",
            body: json!({
                "DocumentName": SHELL,
                "InstanceIds": [LINUX],
                "Parameters": {"commands": ["echo hello"]},
            }),
            nodes: vec![LINUX],
            version: "$DEFAULT",
            concurrency: "50",
            errors: "0",
            timeout: 3600,
        },
        SendRow {
            case: "a hybrid node, defaults",
            body: json!({
                "DocumentName": SHELL,
                "InstanceIds": [HYBRID],
                "Parameters": {"commands": ["echo hello"]},
            }),
            nodes: vec![HYBRID],
            version: "$DEFAULT",
            concurrency: "50",
            errors: "0",
            timeout: 3600,
        },
        SendRow {
            case: "three nodes of three platforms",
            body: json!({
                "DocumentName": AGENT,
                "InstanceIds": [LINUX, WINDOWS, HYBRID],
            }),
            nodes: vec![LINUX, WINDOWS, HYBRID],
            version: "$DEFAULT",
            concurrency: "50",
            errors: "0",
            timeout: 3600,
        },
        SendRow {
            case: "every member the request may set",
            body: json!({
                "DocumentName": AGENT,
                "InstanceIds": [WINDOWS],
                "DocumentVersion": "$LATEST",
                "MaxConcurrency": "10%",
                "MaxErrors": "2",
                "TimeoutSeconds": 60,
                "Comment": "patch window",
            }),
            nodes: vec![WINDOWS],
            version: "$LATEST",
            concurrency: "10%",
            errors: "2",
            timeout: 60,
        },
    ];

    for SendRow {
        case,
        body,
        nodes,
        version,
        concurrency,
        errors,
        timeout,
    } in rows
    {
        let command = sent(&router, case, &body).await;
        let command_id = text(&command, "CommandId");
        assert_eq!(
            command_id.len(),
            36,
            "case {case:?}: a CommandId is 36 characters, got {command_id:?}"
        );
        assert_eq!(
            command_id.matches('-').count(),
            4,
            "case {case:?}: a CommandId is a hyphenated UUID, got {command_id:?}"
        );
        assert_eq!(
            text(&command, "Status"),
            "Pending",
            "case {case:?}: expected a Pending command, got {command:?}"
        );
        assert_eq!(
            text(&command, "StatusDetails"),
            "Pending",
            "case {case:?}: expected Pending status details, got {command:?}"
        );
        assert_eq!(
            command["TargetCount"].as_i64(),
            Some(nodes.len() as i64),
            "case {case:?}: expected one target per named node, got {command:?}"
        );
        for counted in ["CompletedCount", "ErrorCount", "DeliveryTimedOutCount"] {
            assert_eq!(
                command[counted].as_i64(),
                Some(0),
                "case {case:?}: nothing has run, so {counted} is 0, got {command:?}"
            );
        }
        assert_eq!(
            text(&command, "DocumentVersion"),
            version,
            "case {case:?}: expected version {version:?}, got {command:?}"
        );
        assert_eq!(
            text(&command, "MaxConcurrency"),
            concurrency,
            "case {case:?}: expected MaxConcurrency {concurrency:?}, got {command:?}"
        );
        assert_eq!(
            text(&command, "MaxErrors"),
            errors,
            "case {case:?}: expected MaxErrors {errors:?}, got {command:?}"
        );
        assert_eq!(
            command["TimeoutSeconds"].as_i64(),
            Some(i64::from(timeout)),
            "case {case:?}: expected TimeoutSeconds {timeout}, got {command:?}"
        );
        let requested = command["RequestedDateTime"].as_f64().unwrap_or_default();
        let expires = command["ExpiresAfter"].as_f64().unwrap_or_default();
        assert!(
            (expires - requested - f64::from(timeout)).abs() < 1.0,
            "case {case:?}: ExpiresAfter is the request plus the timeout, got \
             {requested} and {expires}"
        );

        // The round trip: every named node holds an invocation of the command that was
        // persisted, and no other registered node does.
        for id in &nodes {
            let (status, reported) = invocation(&router, &command_id, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected node {id} to hold an invocation, got {status} \
                 with {reported:?}"
            );
            assert_eq!(
                text(&reported, "CommandId"),
                command_id,
                "case {case:?}: expected the parent command id, got {reported:?}"
            );
            assert_eq!(
                text(&reported, "DocumentName"),
                text(&command, "DocumentName"),
                "case {case:?}: expected the command's document, got {reported:?}"
            );
            assert_eq!(
                text(&reported, "Status"),
                "Pending",
                "case {case:?}: expected a Pending invocation, got {reported:?}"
            );
            assert_eq!(
                reported["ResponseCode"].as_i64(),
                Some(-1),
                "case {case:?}: nothing has run on {id}, so the response code is -1, got \
                 {reported:?}"
            );
            assert_eq!(
                text(&reported, "StandardOutputContent"),
                "",
                "case {case:?}: nothing has run on {id}, so there is no output, got \
                 {reported:?}"
            );
        }
        let untargeted = [LINUX, WINDOWS, HYBRID]
            .into_iter()
            .find(|id| !nodes.contains(id));
        if let Some(id) = untargeted {
            let (status, refused) = invocation(&router, &command_id, id).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "InvocationDoesNotExist"),
                "case {case:?}: node {id} was not targeted, got {status} with {refused:?}"
            );
        }
    }
}

/// A node the send NAMES has to be a node of that Region that can take a command.
#[tokio::test]
async fn a_named_node_that_is_not_registered_or_cannot_take_a_command_is_refused() {
    let (router, engine) = service();
    seed_inventory(&engine);

    // One row per way a named id resolves to no usable node: unregistered, registered in
    // another Region, registered but not reporting, and one of each beside a usable node.
    let rows = vec![
        ("no node is registered under the id", vec![UNREGISTERED]),
        ("the node is registered in another Region", vec![OTHER_ONLY]),
        ("the node's agent has stopped reporting", vec![OFFLINE]),
        ("one usable node and one unregistered", vec![LINUX, UNREGISTERED]),
        ("one usable node and one not reporting", vec![LINUX, OFFLINE]),
    ];

    for (case, ids) in rows {
        let before = stored_commands(&engine);
        let (status, refused) = send(
            &router,
            &json!({
                "DocumentName": AGENT,
                "InstanceIds": ids,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidInstanceId"),
            "case {case:?}: expected the send to be refused, got {status} with {refused:?}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message,
            commands::NODES_NOT_IN_A_VALID_STATE,
            "case {case:?}: expected the refusal this service words for an id it cannot \
             resolve, got {message:?}"
        );
        assert_eq!(
            stored_commands(&engine),
            before,
            "case {case:?}: a refused send persists no command"
        );
    }
}

/// A node named twice is refused, and nothing is persisted.
#[tokio::test]
async fn a_node_named_more_than_once_is_refused() {
    let (router, engine) = service();
    seed_inventory(&engine);

    // One row per shape of repetition: a node twice, a node twice beside another, and a
    // node three times.
    let rows = vec![
        ("the same node twice", vec![LINUX, LINUX]),
        ("a repeat beside another node", vec![LINUX, HYBRID, LINUX]),
        ("the same node three times", vec![HYBRID, HYBRID, HYBRID]),
    ];

    for (case, ids) in rows {
        let before = stored_commands(&engine);
        let (status, refused) = send(
            &router,
            &json!({"DocumentName": AGENT, "InstanceIds": ids}),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "DuplicateInstanceId"),
            "case {case:?}: expected the send to be refused, got {status} with {refused:?}"
        );
        assert_eq!(
            stored_commands(&engine),
            before,
            "case {case:?}: a refused send persists no command"
        );
    }
}

/// Targets are criteria: they select the registered nodes that meet them, and criteria no
/// node meets leave the command with no invocation rather than refusing the send.
#[tokio::test]
async fn targets_select_the_nodes_that_meet_them_and_none_is_served() {
    let (router, engine) = service();
    seed_inventory(&engine);

    // One row per target key the `Target` doc lists for a command, each with the nodes the
    // seeded inventory holds for it: the node-id forms, the two tag forms, the
    // every-node value, several targets at once, and criteria nothing meets.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "one tag value",
            json!([{"Key": "tag:env", "Values": ["prod"]}]),
            vec![WINDOWS],
        ),
        (
            "a tag value two nodes hold",
            json!([{"Key": "tag:env", "Values": ["dev"]}]),
            vec![LINUX, HYBRID, OFFLINE],
        ),
        (
            "two tag values union",
            json!([{"Key": "tag:env", "Values": ["dev", "prod"]}]),
            vec![LINUX, WINDOWS, HYBRID, OFFLINE],
        ),
        (
            "two targets narrow one another",
            json!([
                {"Key": "tag:env", "Values": ["dev"]},
                {"Key": "tag:Name", "Values": ["web"]},
            ]),
            vec![LINUX],
        ),
        (
            "a tag key, whatever its value",
            json!([{"Key": "tag-key", "Values": ["Name"]}]),
            vec![LINUX],
        ),
        (
            "node ids as criteria",
            json!([{"Key": "InstanceIds", "Values": [LINUX, HYBRID]}]),
            vec![LINUX, HYBRID],
        ),
        (
            "every node of the Region",
            json!([{"Key": "InstanceIds", "Values": ["*"]}]),
            vec![LINUX, WINDOWS, HYBRID, OFFLINE],
        ),
        (
            "every EC2 instance of the Region",
            json!([{"Key": "AWS::EC2::Instance", "Values": ["*"]}]),
            vec![LINUX, WINDOWS, OFFLINE],
        ),
        (
            "a tag value no node holds",
            json!([{"Key": "tag:env", "Values": ["staging"]}]),
            vec![],
        ),
        (
            "a tag key no node holds",
            json!([{"Key": "tag:absent", "Values": ["dev"]}]),
            vec![],
        ),
        (
            "a node id of another Region",
            json!([{"Key": "InstanceIds", "Values": [OTHER_ONLY]}]),
            vec![],
        ),
        (
            "a resource group, of which this account holds none",
            json!([{"Key": "resource-groups:Name", "Values": ["fleet"]}]),
            vec![],
        ),
    ];

    for (case, targets, expected) in rows {
        let command = sent(
            &router,
            case,
            &json!({"DocumentName": AGENT, "Targets": targets}),
        )
        .await;
        let command_id = text(&command, "CommandId");
        assert_eq!(
            command["TargetCount"].as_i64(),
            Some(expected.len() as i64),
            "case {case:?}: expected {} targets, got {command:?}",
            expected.len()
        );
        // Which nodes were selected, read back one node at a time: a selected node holds
        // an invocation and every other registered node does not.
        for id in [LINUX, WINDOWS, HYBRID, OFFLINE] {
            let (status, reported) = invocation(&router, &command_id, id).await;
            if expected.contains(&id) {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected node {id} to be selected, got {status} with \
                     {reported:?}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&reported).as_str()),
                    (400, "InvocationDoesNotExist"),
                    "case {case:?}: expected node {id} not to be selected, got {status} \
                     with {reported:?}"
                );
            }
        }
    }
}

/// A send has to name or describe the nodes to run on, by a key a command selects nodes
/// by.
#[tokio::test]
async fn a_send_that_resolves_no_node_at_all_is_refused() {
    let (router, engine) = service();
    seed_inventory(&engine);

    // One row per way a send names no selection: neither member, both empty, and target
    // keys that are not ones a command selects nodes by.
    let rows = vec![
        ("neither member", json!({"DocumentName": AGENT})),
        (
            "both members empty",
            json!({"DocumentName": AGENT, "InstanceIds": [], "Targets": []}),
        ),
        (
            "a key no tool targets by",
            json!({"DocumentName": AGENT, "Targets": [{"Key": "Fleet", "Values": ["web"]}]}),
        ),
        (
            "a tag form with no tag key",
            json!({"DocumentName": AGENT, "Targets": [{"Key": "tag:", "Values": ["dev"]}]}),
        ),
    ];

    for (case, body) in rows {
        let before = stored_commands(&engine);
        let (status, refused) = send(&router, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the send to be refused, got {status} with {refused:?}"
        );
        assert_eq!(
            stored_commands(&engine),
            before,
            "case {case:?}: a refused send persists no command"
        );
    }
}

/// The document a send runs is resolved among the documents the Region holds and the ones
/// Systems Manager publishes, at a version that document carries.
#[tokio::test]
async fn a_document_the_region_holds_none_of_or_a_version_it_lacks_is_refused() {
    let (router, engine) = service();
    seed_inventory(&engine);

    // A document of this account, created through the API that registers one, so a send
    // resolves what another operation persisted.
    let content = json!({
        "schemaVersion": "2.2",
        "description": "Restart the agent.",
        "mainSteps": [{
            "name": "restart",
            "action": "aws:runShellScript",
            "inputs": {"commands": ["systemctl restart amazon-ssm-agent"]},
        }],
    })
    .to_string();
    let (status, created) = call(
        &router,
        "CreateDocument",
        &json!({"Name": "Restart-Agent", "Content": content}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the document the send resolves must be created first, got {status} with {created:?}"
    );

    // One row per spelling of the document and per version: a published name, a created
    // name, the created document's ARN, the two version aliases and the one version it
    // carries, against names and versions neither holds.
    let arn = format!(
        "arn:aws:ssm:{REGION}:{}:document/Restart-Agent",
        engine::emulator_account_id()
    );
    // The hash a send may carry to pin the document's content, read back through the
    // report that publishes it rather than computed here a second way.
    let (status, described) = call(&router, "DescribeDocument", &json!({"Name": AGENT})).await;
    assert_eq!(
        status, 200,
        "the document's own hash comes from its report, got {status} with {described:?}"
    );
    let hash = text(&described["Document"], "Hash");
    assert_eq!(
        hash.len(),
        64,
        "a Sha256 hash is 64 hexadecimal characters, got {hash:?}"
    );
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a published document",
            json!({"DocumentName": AGENT, "InstanceIds": [LINUX]}),
            None,
        ),
        (
            "a document this account created",
            json!({"DocumentName": "Restart-Agent", "InstanceIds": [LINUX]}),
            None,
        ),
        (
            "that document by its ARN",
            json!({"DocumentName": arn, "InstanceIds": [LINUX]}),
            None,
        ),
        (
            "the version alias $LATEST",
            json!({"DocumentName": AGENT, "DocumentVersion": "$LATEST", "InstanceIds": [LINUX]}),
            None,
        ),
        (
            "the one version the document carries",
            json!({"DocumentName": AGENT, "DocumentVersion": "1", "InstanceIds": [LINUX]}),
            None,
        ),
        (
            "a name neither the account nor the catalogue holds",
            json!({"DocumentName": "No-Such-Document", "InstanceIds": [LINUX]}),
            Some("InvalidDocument"),
        ),
        (
            "a document ARN of another account",
            json!({
                "DocumentName": "arn:aws:ssm:us-west-2:999988887777:document/Restart-Agent",
                "InstanceIds": [LINUX],
            }),
            Some("InvalidDocument"),
        ),
        (
            "a document ARN of another Region",
            json!({
                "DocumentName": format!("arn:aws:ssm:eu-west-1:{}:document/Restart-Agent", engine::emulator_account_id()),
                "InstanceIds": [LINUX],
            }),
            Some("InvalidDocument"),
        ),
        (
            "an ARN of another service",
            json!({
                "DocumentName": "arn:aws:lambda:us-west-2:000000000000:function/Restart-Agent",
                "InstanceIds": [LINUX],
            }),
            Some("InvalidDocument"),
        ),
        (
            "a version the document doesn't carry",
            json!({"DocumentName": AGENT, "DocumentVersion": "7", "InstanceIds": [LINUX]}),
            Some("InvalidDocumentVersion"),
        ),
        (
            "the Sha256 hash of the document's own content",
            json!({
                "DocumentName": AGENT,
                "DocumentHash": hash.clone(),
                "DocumentHashType": "Sha256",
                "InstanceIds": [LINUX],
            }),
            None,
        ),
        (
            "that hash with no hash type named",
            json!({"DocumentName": AGENT, "DocumentHash": hash, "InstanceIds": [LINUX]}),
            None,
        ),
        (
            "a Sha256 hash of other content",
            json!({
                "DocumentName": AGENT,
                "DocumentHash": "0".repeat(64),
                "DocumentHashType": "Sha256",
                "InstanceIds": [LINUX],
            }),
            Some("InvalidDocument"),
        ),
        (
            "a Sha1 hash, of which this service holds none",
            json!({
                "DocumentName": AGENT,
                "DocumentHash": "1".repeat(40),
                "DocumentHashType": "Sha1",
                "InstanceIds": [LINUX],
            }),
            None,
        ),
    ];

    for (case, body, expected) in rows {
        let (status, parsed) = send(&router, &body).await;
        match expected {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the send to be served, got {status} with {parsed:?}"
                );
                let reported = text(&parsed["Command"], "DocumentName");
                assert!(
                    !reported.starts_with("arn:"),
                    "case {case:?}: a command reports the document's NAME, got {reported:?}"
                );
            }
            Some(code) => assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, code),
                "case {case:?}: expected {code}, got {status} with {parsed:?}"
            ),
        }
    }
}

/// A send supplies values for the parameters the document declares, and only for those.
#[tokio::test]
async fn parameters_the_document_doesnt_declare_or_requires_are_refused() {
    let (router, engine) = service();
    seed_inventory(&engine);

    // One row per shape of parameter set: the required one supplied, a defaulted one
    // supplied beside it, every parameter defaulted and none supplied, a required one left
    // out, and a name the document declares nothing of.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "the parameter the document requires",
            json!({"commands": ["echo hello"]}),
            true,
        ),
        (
            "a required and a defaulted parameter",
            json!({"commands": ["echo hello"], "workingDirectory": ["/tmp"]}),
            true,
        ),
        (
            "a required parameter with several values",
            json!({"commands": ["echo one", "echo two"]}),
            true,
        ),
        ("the required parameter left out", json!({}), false),
        (
            "only a defaulted parameter",
            json!({"workingDirectory": ["/tmp"]}),
            false,
        ),
        (
            "a parameter the document declares nothing of",
            json!({"commands": ["echo hello"], "shell": ["zsh"]}),
            false,
        ),
    ];

    for (case, parameters, served) in rows {
        let (status, parsed) = send(
            &router,
            &json!({
                "DocumentName": SHELL,
                "InstanceIds": [LINUX],
                "Parameters": parameters,
            }),
        )
        .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the send to be served, got {status} with {parsed:?}"
            );
            assert_eq!(
                parsed["Command"]["Parameters"], parameters,
                "case {case:?}: a command reports the parameter values it was sent with"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidParameters"),
                "case {case:?}: expected the send to be refused, got {status} with {parsed:?}"
            );
        }
    }

    // A document whose every parameter has a default needs none supplied at all.
    let command = sent(
        &router,
        "every parameter defaulted",
        &json!({"DocumentName": AGENT, "InstanceIds": [LINUX]}),
    )
    .await;
    assert!(
        command["Parameters"].is_null(),
        "a send that supplied no parameter reports none, got {command:?}"
    );
}

/// A document only runs on a named node whose platform its own content supports.
#[tokio::test]
async fn a_document_that_doesnt_support_a_named_nodes_platform_is_refused() {
    let (router, engine) = service();
    seed_inventory(&engine);

    // One row per (document, node platform) pair the inventory can form: each document's
    // steps declare the platforms it supports, so the pairs it declares are served and the
    // ones it does not are refused.
    let rows = vec![
        ("shell script on Linux", SHELL, LINUX, true),
        ("shell script on macOS", SHELL, HYBRID, true),
        ("shell script on Windows", SHELL, WINDOWS, false),
        ("PowerShell script on Windows", POWERSHELL, WINDOWS, true),
        ("PowerShell script on Linux", POWERSHELL, LINUX, false),
        ("PowerShell script on macOS", POWERSHELL, HYBRID, false),
        ("an every-platform document on Linux", AGENT, LINUX, true),
        ("an every-platform document on Windows", AGENT, WINDOWS, true),
        ("an every-platform document on macOS", AGENT, HYBRID, true),
    ];

    for (case, document, id, served) in rows {
        // The two script documents declare a parameter with no default; the agent update
        // declares none without one, so a send of it supplies no parameter at all.
        let mut body = json!({"DocumentName": document, "InstanceIds": [id]});
        if document != AGENT {
            body["Parameters"] = json!({"commands": ["echo hello"]});
        }
        let (status, parsed) = send(&router, &body).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the send to be served, got {status} with {parsed:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "UnsupportedPlatformType"),
                "case {case:?}: expected the send to be refused, got {status} with {parsed:?}"
            );
            assert!(
                error_message(&parsed).contains(document),
                "case {case:?}: the refusal names the document, got {:?}",
                error_message(&parsed)
            );
        }
    }

    // A node that has reported no platform restricts nothing, so every document runs on it.
    let mut unreported = node(
        "i-0fff3333",
        RESOURCE_TYPE_EC2_INSTANCE,
        "Linux",
        "Online",
        &[],
    );
    unreported.properties.platform_type = None;
    register(&engine, REGION, unreported);
    let command = sent(
        &router,
        "a node that reported no platform",
        &json!({
            "DocumentName": POWERSHELL,
            "InstanceIds": ["i-0fff3333"],
            "Parameters": {"commands": ["echo hello"]},
        }),
    )
    .await;
    assert_eq!(
        command["TargetCount"].as_i64(),
        Some(1),
        "a node that reported no platform takes any document, got {command:?}"
    );
}

/// The ARNs a send may carry have to name what the service publishes notifications with.
#[tokio::test]
async fn a_member_that_must_name_an_arn_is_refused_when_it_names_none() {
    let (router, engine) = service();
    seed_inventory(&engine);

    let role = format!(
        "arn:aws:iam::{}:role/SSMNotificationRole",
        engine::emulator_account_id()
    );
    let topic = format!(
        "arn:aws:sns:{REGION}:{}:command-status",
        engine::emulator_account_id()
    );
    // One row per member and per shape of value: the ARN the member documents, an ARN of
    // another service, an ARN of the right service but the wrong resource type, and text
    // that is no ARN at all.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "the service role as an IAM role ARN",
            json!({"ServiceRoleArn": role}),
            None,
        ),
        (
            "the notification topic as an SNS topic ARN",
            json!({"NotificationConfig": {"NotificationArn": topic}}),
            None,
        ),
        (
            "a service role that is an IAM user",
            json!({"ServiceRoleArn": format!("arn:aws:iam::{}:user/someone", engine::emulator_account_id())}),
            Some("InvalidRole"),
        ),
        (
            "a service role that is no ARN at all",
            json!({"ServiceRoleArn": "SSMNotificationRole"}),
            Some("InvalidRole"),
        ),
        (
            "a notification ARN of another service",
            json!({"NotificationConfig": {"NotificationArn": format!("arn:aws:sqs:{REGION}:{}:queue", engine::emulator_account_id())}}),
            Some("InvalidNotificationConfig"),
        ),
        (
            "a notification ARN that is no ARN at all",
            json!({"NotificationConfig": {"NotificationArn": "command-status"}}),
            Some("InvalidNotificationConfig"),
        ),
    ];

    for (case, extra, expected) in rows {
        let mut body = json!({"DocumentName": AGENT, "InstanceIds": [LINUX]});
        for (member, value) in extra.as_object().expect("a row sets members") {
            body[member.as_str()] = value.clone();
        }
        let (status, parsed) = send(&router, &body).await;
        match expected {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the send to be served, got {status} with {parsed:?}"
                );
                for (member, value) in extra.as_object().expect("a row sets members") {
                    let reported = if member.as_str() == "ServiceRoleArn" {
                        &parsed["Command"]["ServiceRole"]
                    } else {
                        &parsed["Command"][member.as_str()]
                    };
                    assert_eq!(
                        reported, value,
                        "case {case:?}: a command reports the {member} it was sent with"
                    );
                }
            }
            Some(code) => assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, code),
                "case {case:?}: expected {code}, got {status} with {parsed:?}"
            ),
        }
    }
}

/// Each send mints its own command, and a read resolves a command only by the id it was
/// minted with, in the Region it was sent to.
#[tokio::test]
async fn a_read_resolves_a_command_by_the_id_its_send_minted() {
    let (router, engine) = service();
    seed_inventory(&engine);

    let mut minted: Vec<String> = Vec::new();
    for case in ["first send", "second send", "third send"] {
        let command = sent(
            &router,
            case,
            &json!({"DocumentName": AGENT, "InstanceIds": [LINUX]}),
        )
        .await;
        let command_id = text(&command, "CommandId");
        assert!(
            !minted.contains(&command_id),
            "case {case:?}: every send mints its own command id, got {command_id:?} twice"
        );
        minted.push(command_id);
    }
    let first = minted.first().expect("three sends minted three ids").clone();

    // One row per way a read names something the store does not hold: a well-formed id no
    // send minted, the id of a command sent to another Region, a node no node id is
    // registered under, a registered node the command did not run on, and a step the
    // document declares nothing of.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "an id no send minted",
            json!({"CommandId": "11111111-2222-3333-4444-555555555555", "InstanceId": LINUX}),
            REGION,
            "InvalidCommandId",
        ),
        (
            "a command of another Region",
            json!({"CommandId": first.clone(), "InstanceId": LINUX}),
            OTHER_REGION,
            "InvalidCommandId",
        ),
        (
            "a node that is not registered",
            json!({"CommandId": first.clone(), "InstanceId": UNREGISTERED}),
            REGION,
            "InvalidInstanceId",
        ),
        (
            "a registered node the command did not run on",
            json!({"CommandId": first.clone(), "InstanceId": WINDOWS}),
            REGION,
            "InvocationDoesNotExist",
        ),
        (
            "a step the document declares nothing of",
            json!({"CommandId": first.clone(), "InstanceId": LINUX, "PluginName": "aws:runShellScript"}),
            REGION,
            "InvalidPluginName",
        ),
    ];

    for (case, body, region, code) in rows {
        let (status, refused) =
            call_in_region(&router, region, "GetCommandInvocation", &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, code),
            "case {case:?}: expected {code}, got {status} with {refused:?}"
        );
    }

    // A step the document DOES declare is reported, by that name.
    let command = sent(
        &router,
        "a send of the shell document",
        &json!({
            "DocumentName": SHELL,
            "InstanceIds": [LINUX],
            "Parameters": {"commands": ["echo hello"]},
        }),
    )
    .await;
    let (status, reported) = call(
        &router,
        "GetCommandInvocation",
        &json!({
            "CommandId": text(&command, "CommandId"),
            "InstanceId": LINUX,
            "PluginName": SHELL_STEP,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a step the document declares is reported, got {status} with {reported:?}"
    );
    assert_eq!(
        text(&reported, "PluginName"),
        SHELL_STEP,
        "the reported step is the one that was asked for, got {reported:?}"
    );
}

/// A command stops being reported once Run Command no longer retains its history, whether
/// or not anybody called in the meantime.
#[tokio::test]
async fn a_command_is_reported_for_as_long_as_its_history_is_retained() {
    // One row per age of the same command: just sent, a day inside the retention window,
    // just inside it, and past it.
    let rows = vec![
        ("just sent", Duration::ZERO, true),
        ("a day old", Duration::from_secs(24 * 60 * 60), true),
        (
            "an hour inside the retention window",
            commands::COMMAND_HISTORY_RETENTION - Duration::from_secs(60 * 60),
            true,
        ),
        (
            "an hour past the retention window",
            commands::COMMAND_HISTORY_RETENTION + Duration::from_secs(60 * 60),
            false,
        ),
    ];

    for (case, age, retained) in rows {
        let (router, engine) = service();
        seed_inventory(&engine);
        let command = sent(
            &router,
            case,
            &json!({"DocumentName": AGENT, "InstanceIds": [LINUX]}),
        )
        .await;
        let command_id = text(&command, "CommandId");
        // Age the stored command rather than the clock: what the store holds is what a
        // read reports, so a command requested that long ago is this command.
        {
            let state = engine.state::<ComAmazonawsSsmAmazonssmState>();
            let mut store = state.lock();
            for held in store.commands.values_mut() {
                held.requested_at -= age;
            }
        }
        let (status, reported) = invocation(&router, &command_id, LINUX).await;
        if retained {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the command to be reported, got {status} with \
                 {reported:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&reported).as_str()),
                (400, "InvalidCommandId"),
                "case {case:?}: expected the command to be gone, got {status} with \
                 {reported:?}"
            );
            assert_eq!(
                stored_commands(&engine),
                0,
                "case {case:?}: a command past its retention is dropped from the store"
            );
        }
    }
}

/// The command history this store keeps is bounded, oldest dropped first, however many
/// sends arrive.
#[tokio::test]
async fn the_command_history_the_store_keeps_is_bounded() {
    let (router, engine) = service();
    seed_inventory(&engine);

    // One send past the bound, so the oldest command's history is the one dropped.
    let mut first = String::new();
    for sequence in 0..=commands::MAX_COMMANDS_RETAINED {
        let command = sent(
            &router,
            "a send inside the bound",
            &json!({"DocumentName": AGENT, "InstanceIds": [LINUX]}),
        )
        .await;
        if sequence == 0 {
            first = text(&command, "CommandId");
        }
        // The store never holds more than the bound, at any point along the way.
        assert!(
            stored_commands(&engine) <= commands::MAX_COMMANDS_RETAINED,
            "send {sequence}: the store holds at most {} commands, got {}",
            commands::MAX_COMMANDS_RETAINED,
            stored_commands(&engine)
        );
    }
    assert_eq!(
        stored_commands(&engine),
        commands::MAX_COMMANDS_RETAINED,
        "the store keeps the bound's worth of command history"
    );
    let (status, reported) = invocation(&router, &first, LINUX).await;
    assert_eq!(
        (status, error_code(&reported).as_str()),
        (400, "InvalidCommandId"),
        "the oldest command's history is the one dropped, got {status} with {reported:?}"
    );
}
