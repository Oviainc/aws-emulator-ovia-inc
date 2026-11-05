// SPDX-License-Identifier: CC0-1.0

//! `ListCommands` end to end: the Run Command history a Region holds, narrowed by the
//! criteria a request names and paged by `MaxResults`.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol") — and every command read back
//! here was persisted by a `SendCommand` through that same door.
//!
//! Two parts of the fixture are seeded through the service's own state rather than through
//! an API. The node inventory is, because a node enters it only by REGISTERING and no
//! public API of this service registers one (the sibling `run_command` suite seeds it the
//! same way). The command STATUSES are, because a command is minted `Pending` and only an
//! SSM Agent's poll from the node moves it on, and this emulator runs no agent — so the
//! statuses a `Status` or `ExecutionStage` filter spans are written onto the records the
//! sends persisted, and the read is still reading what the store holds.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — filter key, filter value, the published bounds of a value set, an
//! instant on and past a boundary, node, Region, page size, token — rather than the single
//! list the recorded units happened to make.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::state::{
    commands, ComAmazonawsSsmAmazonssmState, ManagedNodeRecord, RESOURCE_TYPE_EC2_INSTANCE,
    RESOURCE_TYPE_MANAGED_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for unless a case says otherwise. Deliberately not the
/// emulator's default, so a history served here proves the Region came from the request's
/// SigV4 credential scope.
const REGION: &str = "us-west-2";

/// A second Region, to pin that a command is reported only through the Region it was sent
/// to.
const OTHER_REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AmazonSSM";

/// An online EC2 node running Linux.
const LINUX: &str = "i-0abc1234";

/// An online EC2 node running Windows.
const WINDOWS: &str = "i-0123456789abcdef0";

/// An online hybrid node running macOS.
const HYBRID: &str = "mi-01234567890123456";

/// A registered, online node no command below was issued against.
const IDLE: &str = "i-0fff5555";

/// A well-formed node id no node is registered under.
const UNREGISTERED: &str = "i-0dead999";

/// A node registered only in [`OTHER_REGION`].
const OTHER_ONLY: &str = "i-0eeee222";

/// A command document Systems Manager publishes whose steps are restricted to Linux and
/// macOS, and which declares one parameter with no default.
const SHELL: &str = "AWS-RunShellScript";

/// A command document Systems Manager publishes whose step is restricted to Windows.
const POWERSHELL: &str = "AWS-RunPowerShellScript";

/// A command document Systems Manager publishes that declares no step precondition, so it
/// runs on every platform, and whose every parameter has a default.
const AGENT: &str = "AWS-UpdateSSMAgent";

/// A document of the published catalogue (`state::PUBLISHED_DOCUMENTS`) that no command
/// below runs, so a `DocumentName` filter naming it selects none.
const UNUSED_DOCUMENT: &str = "AWS-ConfigureS3BucketVersioning";

/// One hour, the spacing between the fixture's commands.
const HOUR: Duration = Duration::from_secs(3600);

/// A router with just this service mounted, and the engine it was mounted on — the handle
/// a case seeds the node inventory and the command statuses through.
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

/// One registered node: its id, kind, platform and tags, online so a send resolves it.
fn node(
    id: &str,
    resource_type: &str,
    platform: &str,
    tags: &[(&str, &str)],
) -> ManagedNodeRecord {
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut record = ManagedNodeRecord::registering(id, resource_type, at);
    record.properties.platform_type = Some(platform.to_string());
    record.properties.ping_status = Some("Online".to_string());
    record.properties.computer_name = Some(format!("{id}.compute.internal"));
    record.tags = tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<BTreeMap<String, String>>();
    record
}

/// The inventory the fixture's sends resolve against: three online nodes spanning node
/// kind, platform and tags, one online node nothing is sent to, and one node of another
/// Region.
fn seed_inventory(engine: &Arc<engine::InMemoryEngine>) {
    register(
        engine,
        REGION,
        node(
            LINUX,
            RESOURCE_TYPE_EC2_INSTANCE,
            "Linux",
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
            &[("env", "prod")],
        ),
    );
    register(
        engine,
        REGION,
        node(HYBRID, RESOURCE_TYPE_MANAGED_INSTANCE, "MacOS", &[("env", "dev")]),
    );
    register(
        engine,
        REGION,
        node(IDLE, RESOURCE_TYPE_EC2_INSTANCE, "Linux", &[("env", "qa")]),
    );
    register(
        engine,
        OTHER_REGION,
        node(
            OTHER_ONLY,
            RESOURCE_TYPE_EC2_INSTANCE,
            "Linux",
            &[("env", "dev")],
        ),
    );
}

/// Now, truncated to a whole second, so an instant the fixture dates a command with
/// round-trips through the ISO 8601 spelling a filter value carries.
fn whole_second_now() -> SystemTime {
    let secs = DateTime::from(SystemTime::now()).secs();
    SystemTime::UNIX_EPOCH + Duration::from_secs(u64::try_from(secs).expect("a positive epoch"))
}

/// `at` in the spelling `CommandFilter$value` documents for `InvokedAfter` and
/// `InvokedBefore` ("for example, specify 2024-07-07T00:00:00Z").
fn iso8601(at: SystemTime) -> String {
    DateTime::from(at)
        .fmt(Format::DateTime)
        .expect("an instant renders in ISO 8601")
}

/// One command the fixture sent: the id the send minted and the instant it is dated at.
struct Sent {
    id: String,
    at: SystemTime,
}

/// The history every suite below reads: four commands of [`REGION`] spanning document,
/// node selection, invocation count and status, one command of [`OTHER_REGION`], and the
/// instants they are dated at.
struct History {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    /// Targets that select nothing, so the command holds no invocation. Newest.
    empty: Sent,
    /// Named a Windows node; dated an hour older, and seeded `TimedOut`.
    powershell: Sent,
    /// Selected a node by tag; dated two hours older, and seeded `Success`.
    agent: Sent,
    /// Named two nodes; dated three hours older, and left `Pending`. Oldest.
    shell: Sent,
    /// A command of [`OTHER_REGION`], which [`REGION`]'s history never reports.
    other: Sent,
}

impl History {
    /// The ids of the Region's commands in the order a history reports them: newest
    /// requested first.
    fn newest_first(&self) -> Vec<String> {
        vec![
            self.empty.id.clone(),
            self.powershell.id.clone(),
            self.agent.id.clone(),
            self.shell.id.clone(),
        ]
    }

    /// Call `ListCommands` signed for [`REGION`].
    async fn list(&self, body: &Value) -> (u16, Value) {
        call_in_region(&self.router, REGION, "ListCommands", body).await
    }

    /// The command ids a list that must have succeeded reported, in its own order.
    async fn listed(&self, case: &str, body: &Value) -> Vec<String> {
        let (status, parsed) = self.list(body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {parsed:?}"
        );
        listed_ids(case, &parsed)
    }
}

/// The command ids of a `ListCommands` body, in the order it reported them.
fn listed_ids(case: &str, body: &Value) -> Vec<String> {
    body["Commands"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Commands list, got {body:?}"))
        .iter()
        .map(|command| {
            command["CommandId"]
                .as_str()
                .unwrap_or("<unset>")
                .to_string()
        })
        .collect()
}

/// Send one command through the wire, date the record it persisted at `at`, and answer the
/// id it minted.
async fn send_dated(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    region: &str,
    body: &Value,
    at: SystemTime,
) -> Sent {
    let (status, parsed) = call_in_region(router, region, "SendCommand", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {parsed:?}"
    );
    let id = parsed["Command"]["CommandId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a CommandId, got {parsed:?}"))
        .to_string();
    with_record(engine, region, &id, |record| record.requested_at = at);
    Sent { id, at }
}

/// Reach the stored command `id` of `region` and let `edit` write on it.
fn with_record(
    engine: &Arc<engine::InMemoryEngine>,
    region: &str,
    id: &str,
    edit: impl FnOnce(&mut commands::CommandRecord),
) {
    let state = engine.state::<ComAmazonawsSsmAmazonssmState>();
    let mut store = state.lock();
    let record = store
        .commands
        .get_mut(&(region.to_string(), id.to_string()))
        .unwrap_or_else(|| panic!("the send persisted command {id:?} in {region:?}"));
    edit(record);
}

/// Move a stored command to the status an agent's poll would have moved it to.
fn set_status(
    engine: &Arc<engine::InMemoryEngine>,
    region: &str,
    id: &str,
    status: &str,
    status_details: &str,
) {
    with_record(engine, region, id, |record| {
        record.status = status.to_string();
        record.status_details = status_details.to_string();
    });
}

/// The fixture: four commands of [`REGION`] and one of [`OTHER_REGION`], each dated so the
/// order a history reports them in is unambiguous.
async fn history() -> History {
    let (router, engine) = service();
    seed_inventory(&engine);
    let anchor = whole_second_now();

    let shell = send_dated(
        &router,
        &engine,
        "shell",
        REGION,
        &json!({
            "DocumentName": SHELL,
            "InstanceIds": [LINUX, HYBRID],
            "Parameters": {"commands": ["echo one"]},
            "Comment": "the oldest command",
        }),
        anchor - 3 * HOUR,
    )
    .await;
    let agent = send_dated(
        &router,
        &engine,
        "agent",
        REGION,
        &json!({
            "DocumentName": AGENT,
            "Targets": [{"Key": "tag:env", "Values": ["prod"]}],
        }),
        anchor - 2 * HOUR,
    )
    .await;
    let powershell = send_dated(
        &router,
        &engine,
        "powershell",
        REGION,
        &json!({
            "DocumentName": POWERSHELL,
            "InstanceIds": [WINDOWS],
            "Parameters": {"commands": ["Get-Date"]},
        }),
        anchor - HOUR,
    )
    .await;
    let empty = send_dated(
        &router,
        &engine,
        "empty",
        REGION,
        &json!({
            "DocumentName": AGENT,
            "Targets": [{"Key": "tag:env", "Values": ["nothing-is-tagged-this"]}],
        }),
        anchor,
    )
    .await;
    let other = send_dated(
        &router,
        &engine,
        "other-region",
        OTHER_REGION,
        &json!({
            "DocumentName": SHELL,
            "InstanceIds": [OTHER_ONLY],
            "Parameters": {"commands": ["echo other"]},
        }),
        anchor,
    )
    .await;

    // The statuses a filter spans: an agent's poll is what moves a command off `Pending`,
    // and nothing polls here, so they are written onto the records the sends persisted.
    set_status(&engine, REGION, &agent.id, "Success", "Success");
    set_status(&engine, REGION, &powershell.id, "TimedOut", "Delivery Timed Out");

    History {
        router,
        engine,
        empty,
        powershell,
        agent,
        shell,
        other,
    }
}

/// A history reports every command the Region holds, newest requested first, with the
/// members the send persisted — and no command of another Region.
#[tokio::test]
async fn the_history_reports_every_command_of_the_region_newest_first() {
    let history = history().await;

    let listed = history.listed("no criteria", &json!({})).await;
    assert_eq!(
        listed,
        history.newest_first(),
        "case {:?}: expected the Region's commands newest first, got {listed:?}",
        "no criteria"
    );
    assert!(
        !listed.contains(&history.other.id),
        "case {:?}: expected no command of {OTHER_REGION}, got {listed:?}",
        "no criteria"
    );

    // Every member of one reported command, against what the send persisted.
    let (status, parsed) = history.list(&json!({})).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200, got {status} with {parsed:?}",
        "members"
    );
    let oldest = parsed["Commands"][3].clone();
    assert_eq!(
        oldest["CommandId"].as_str(),
        Some(history.shell.id.as_str()),
        "case {:?}: expected the oldest command last, got {oldest:?}",
        "members"
    );
    assert_eq!(
        oldest["DocumentName"].as_str(),
        Some(SHELL),
        "case {:?}: expected the document the send ran, got {oldest:?}",
        "members"
    );
    assert_eq!(
        oldest["Comment"].as_str(),
        Some("the oldest command"),
        "case {:?}: expected the comment the send carried, got {oldest:?}",
        "members"
    );
    assert_eq!(
        oldest["Status"].as_str(),
        Some("Pending"),
        "case {:?}: expected the status the store holds, got {oldest:?}",
        "members"
    );
    assert_eq!(
        oldest["TargetCount"].as_i64(),
        Some(2),
        "case {:?}: expected one target per node the send resolved, got {oldest:?}",
        "members"
    );
    assert_eq!(
        oldest["InstanceIds"],
        json!([LINUX, HYBRID]),
        "case {:?}: expected the node ids the send named, got {oldest:?}",
        "members"
    );
    let requested = oldest["RequestedDateTime"].as_f64().unwrap_or_default();
    let dated = DateTime::from(history.shell.at).secs() as f64;
    assert!(
        (requested - dated).abs() < 1.0,
        "case {:?}: expected RequestedDateTime {dated}, got {requested}",
        "members"
    );

    // The other Region's endpoint reports its own one command and nothing else.
    let (status, parsed) =
        call_in_region(&history.router, OTHER_REGION, "ListCommands", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200, got {status} with {parsed:?}",
        "other region"
    );
    let listed = listed_ids("other region", &parsed);
    assert_eq!(
        listed,
        vec![history.other.id.clone()],
        "case {:?}: expected only the command sent there, got {listed:?}",
        "other region"
    );
}

/// One row of the filter table: the filters to send and the commands they must select, in
/// the order the history reports them.
struct FilterRow {
    case: &'static str,
    filters: Value,
    expected: Vec<String>,
}

/// Each filter selects the commands whose property it names, and two filters narrow
/// together.
#[tokio::test]
async fn each_filter_selects_the_commands_whose_property_it_names() {
    let history = history().await;
    let (empty, powershell, agent, shell) = (
        history.empty.id.clone(),
        history.powershell.id.clone(),
        history.agent.id.clone(),
        history.shell.id.clone(),
    );

    // One row per filter key, and per axis the key's own values span: a document the
    // history ran and one it did not, a status reading and a detailed-status reading, both
    // execution stages, and an instant ON a boundary and one past it.
    let rows = vec![
        FilterRow {
            case: "DocumentName the oldest command ran",
            filters: json!([{"key": "DocumentName", "value": SHELL}]),
            expected: vec![shell.clone()],
        },
        FilterRow {
            case: "DocumentName two commands ran",
            filters: json!([{"key": "DocumentName", "value": AGENT}]),
            expected: vec![empty.clone(), agent.clone()],
        },
        FilterRow {
            case: "DocumentName nothing ran",
            filters: json!([{"key": "DocumentName", "value": UNUSED_DOCUMENT}]),
            expected: Vec::new(),
        },
        FilterRow {
            case: "Status Pending",
            filters: json!([{"key": "Status", "value": "Pending"}]),
            expected: vec![empty.clone(), shell.clone()],
        },
        FilterRow {
            case: "Status Success",
            filters: json!([{"key": "Status", "value": "Success"}]),
            expected: vec![agent.clone()],
        },
        FilterRow {
            case: "Status TimedOut",
            filters: json!([{"key": "Status", "value": "TimedOut"}]),
            expected: vec![powershell.clone()],
        },
        FilterRow {
            // The detailed status spelled without its spaces, which is how
            // `CommandFilter$value` lists it.
            case: "Status DeliveryTimedOut",
            filters: json!([{"key": "Status", "value": "DeliveryTimedOut"}]),
            expected: vec![powershell.clone()],
        },
        FilterRow {
            case: "Status no command holds",
            filters: json!([{"key": "Status", "value": "Incomplete"}]),
            expected: Vec::new(),
        },
        FilterRow {
            case: "ExecutionStage Executing",
            filters: json!([{"key": "ExecutionStage", "value": "Executing"}]),
            expected: vec![empty.clone(), shell.clone()],
        },
        FilterRow {
            case: "ExecutionStage Complete",
            filters: json!([{"key": "ExecutionStage", "value": "Complete"}]),
            expected: vec![powershell.clone(), agent.clone()],
        },
        FilterRow {
            case: "InvokedAfter the instant a command was requested",
            filters: json!([{"key": "InvokedAfter", "value": iso8601(history.agent.at)}]),
            expected: vec![empty.clone(), powershell.clone(), agent.clone()],
        },
        FilterRow {
            case: "InvokedAfter one second past that instant",
            filters: json!([
                {"key": "InvokedAfter", "value": iso8601(history.agent.at + Duration::from_secs(1))}
            ]),
            expected: vec![empty.clone(), powershell.clone()],
        },
        FilterRow {
            case: "InvokedBefore the instant a command was requested",
            filters: json!([{"key": "InvokedBefore", "value": iso8601(history.agent.at)}]),
            expected: vec![agent.clone(), shell.clone()],
        },
        FilterRow {
            case: "InvokedBefore one second before the oldest command",
            filters: json!([
                {"key": "InvokedBefore", "value": iso8601(history.shell.at - Duration::from_secs(1))}
            ]),
            expected: Vec::new(),
        },
        FilterRow {
            case: "an instant window",
            filters: json!([
                {"key": "InvokedAfter", "value": iso8601(history.shell.at)},
                {"key": "InvokedBefore", "value": iso8601(history.agent.at)},
            ]),
            expected: vec![agent.clone(), shell.clone()],
        },
        FilterRow {
            case: "a document and a status",
            filters: json!([
                {"key": "DocumentName", "value": AGENT},
                {"key": "Status", "value": "Pending"},
            ]),
            expected: vec![empty.clone()],
        },
        FilterRow {
            case: "a document and a stage",
            filters: json!([
                {"key": "DocumentName", "value": AGENT},
                {"key": "ExecutionStage", "value": "Complete"},
            ]),
            expected: vec![agent.clone()],
        },
        FilterRow {
            case: "criteria that cannot hold together",
            filters: json!([
                {"key": "DocumentName", "value": SHELL},
                {"key": "Status", "value": "Success"},
            ]),
            expected: Vec::new(),
        },
    ];

    for row in rows {
        let listed = history
            .listed(row.case, &json!({"Filters": row.filters}))
            .await;
        assert_eq!(
            listed, row.expected,
            "case {:?}: expected {:?}, got {listed:?}",
            row.case, row.expected
        );
    }
}

/// A `CommandId` lists only the command it names, and an id the Region holds no command
/// under is refused.
#[tokio::test]
async fn a_command_id_lists_only_the_command_it_names() {
    let history = history().await;

    for (case, id) in [
        ("the newest command", history.empty.id.clone()),
        ("a command in the middle", history.agent.id.clone()),
        ("the oldest command", history.shell.id.clone()),
    ] {
        let listed = history.listed(case, &json!({"CommandId": id})).await;
        assert_eq!(
            listed,
            vec![id.clone()],
            "case {case:?}: expected only {id:?}, got {listed:?}"
        );
    }

    // A `CommandId` NAMES a command, so an id the Region holds none under — one nothing
    // minted, and one minted by the other Region's endpoint — names none to list.
    for (case, id) in [
        ("an id nothing minted", "00000000-0000-4000-8000-000000000000"),
        ("an id of another Region", history.other.id.as_str()),
    ] {
        let (status, parsed) = history.list(&json!({"CommandId": id})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {parsed:?}"
        );
        assert_eq!(
            error_code(&parsed),
            "InvalidCommandId",
            "case {case:?}: expected InvalidCommandId, got {parsed:?}"
        );
    }

    // A command id narrows TOGETHER with the filters, so one that cannot hold with them
    // selects nothing rather than everything the id names.
    let case = "a command id beside a filter it fails";
    let listed = history
        .listed(
            case,
            &json!({
                "CommandId": history.shell.id,
                "Filters": [{"key": "Status", "value": "Success"}],
            }),
        )
        .await;
    assert!(
        listed.is_empty(),
        "case {case:?}: expected no command, got {listed:?}"
    );
}

/// An `InstanceId` selects the commands that hold an invocation for that node.
#[tokio::test]
async fn an_instance_id_selects_the_commands_issued_against_that_node() {
    let history = history().await;

    // One row per node the inventory holds: nodes a send NAMED, a node a target SELECTED,
    // a registered node nothing was issued against, an id no node is registered under, and
    // a node of another Region.
    let rows = vec![
        ("a named Linux node", LINUX, vec![history.shell.id.clone()]),
        ("a named hybrid node", HYBRID, vec![history.shell.id.clone()]),
        (
            "a node both named and selected by a target",
            WINDOWS,
            vec![history.powershell.id.clone(), history.agent.id.clone()],
        ),
        ("a registered node with no commands", IDLE, Vec::new()),
        ("an unregistered node id", UNREGISTERED, Vec::new()),
        ("a node of another Region", OTHER_ONLY, Vec::new()),
    ];

    for (case, id, expected) in rows {
        let listed = history.listed(case, &json!({"InstanceId": id})).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // An `InstanceId` narrows together with the filters, the same way a command id does.
    let case = "a node and the document one of its commands ran";
    let listed = history
        .listed(
            case,
            &json!({
                "InstanceId": WINDOWS,
                "Filters": [{"key": "DocumentName", "value": AGENT}],
            }),
        )
        .await;
    assert_eq!(
        listed,
        vec![history.agent.id.clone()],
        "case {case:?}: expected the agent command, got {listed:?}"
    );
}

/// A page is bounded by `MaxResults` and resumed by the token the previous page handed
/// back, and walking the tokens reports every command once, in the history's own order.
#[tokio::test]
async fn a_page_is_bounded_by_max_results_and_resumed_by_its_token() {
    let history = history().await;
    let whole = history.newest_first();

    // One row per page size: each size below the count, the count itself, and the modelled
    // maximum (`CommandMaxResults`, range 1..50), which is also what a request naming no
    // `MaxResults` is served up to.
    for size in [
        1usize,
        2,
        3,
        whole.len(),
        commands::MAX_COMMAND_PAGE_SIZE as usize,
    ] {
        let case = format!("pages of {size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({"MaxResults": size});
            if let Some(token) = token.as_deref() {
                body["NextToken"] = Value::String(token.to_string());
            }
            let (status, parsed) = history.list(&body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {parsed:?}"
            );
            let page = listed_ids(&case, &parsed);
            assert!(
                page.len() <= size,
                "case {case:?}: expected at most {size} commands in a page, got {page:?}"
            );
            walked.extend(page);
            token = parsed["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: expected the walk to end, got {pages} pages and {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: expected every command once, newest first, got {walked:?}"
        );
        assert_eq!(
            pages,
            whole.len().div_ceil(size),
            "case {case:?}: expected {} pages, got {pages}",
            whole.len().div_ceil(size)
        );
    }

    // The token is a cursor on the result, so it resumes a NARROWED result too.
    let case = "a narrowed result paged one at a time";
    let (status, parsed) = history
        .list(&json!({
            "MaxResults": 1,
            "Filters": [{"key": "DocumentName", "value": AGENT}],
        }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {parsed:?}"
    );
    assert_eq!(
        listed_ids(case, &parsed),
        vec![history.empty.id.clone()],
        "case {case:?}: expected the newest of the two, got {parsed:?}"
    );
    assert_eq!(
        parsed["NextToken"].as_str(),
        Some(history.agent.id.as_str()),
        "case {case:?}: expected the next page's first command id as the token, got {parsed:?}"
    );
}

/// One row of the refusal table: the request to make, the HTTP status it must carry and
/// the error code it must name.
struct RefusalRow {
    case: &'static str,
    body: Value,
    code: Option<&'static str>,
}

/// A request outside what the read accepts is refused, and nothing of the history is
/// reported for it.
#[tokio::test]
async fn a_request_outside_what_the_read_accepts_is_refused() {
    let history = history().await;

    let rows = vec![
        // Each published bound of `CommandMaxResults` (range 1..50) and one past it.
        RefusalRow {
            case: "a page size below the modelled minimum",
            body: json!({"MaxResults": commands::MIN_COMMAND_PAGE_SIZE - 1}),
            code: None,
        },
        RefusalRow {
            case: "a page size past the modelled maximum",
            body: json!({"MaxResults": commands::MAX_COMMAND_PAGE_SIZE + 1}),
            code: None,
        },
        RefusalRow {
            case: "a token this operation never handed out",
            body: json!({"NextToken": "not-a-token-this-service-minted"}),
            code: Some("InvalidNextToken"),
        },
        RefusalRow {
            case: "a token shaped right that names no command of the result",
            body: json!({"NextToken": "00000000-0000-4000-8000-000000000000"}),
            code: Some("InvalidNextToken"),
        },
        RefusalRow {
            case: "a filter key the model does not declare",
            body: json!({"Filters": [{"key": "Comment", "value": "anything"}]}),
            code: None,
        },
        RefusalRow {
            case: "a status no command may hold",
            body: json!({"Filters": [{"key": "Status", "value": "Sleeping"}]}),
            code: Some("ValidationException"),
        },
        RefusalRow {
            case: "an execution stage that is neither",
            body: json!({"Filters": [{"key": "ExecutionStage", "value": "Halfway"}]}),
            code: Some("ValidationException"),
        },
        RefusalRow {
            case: "an InvokedAfter that is not a timestamp",
            body: json!({"Filters": [{"key": "InvokedAfter", "value": "last Tuesday"}]}),
            code: Some("ValidationException"),
        },
        RefusalRow {
            case: "an InvokedBefore that is not a timestamp",
            body: json!({"Filters": [{"key": "InvokedBefore", "value": "2024-13-45"}]}),
            code: Some("ValidationException"),
        },
    ];

    for row in rows {
        let (status, parsed) = history.list(&row.body).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {parsed:?}",
            row.case
        );
        if let Some(code) = row.code {
            assert_eq!(
                error_code(&parsed),
                code,
                "case {:?}: expected {code}, got {parsed:?}",
                row.case
            );
        }
        assert!(
            parsed["Commands"].is_null(),
            "case {:?}: expected no history in a refusal, got {parsed:?}",
            row.case
        );
    }

    // A filter-value refusal states the value, the member it was carried IN — the filter's
    // own place in the list the request sent, counted from one — and the constraint the
    // value failed.
    let case = "the wording of a filter-value refusal";
    let (status, parsed) = history
        .list(&json!({"Filters": [
            {"key": "DocumentName", "value": SHELL},
            {"key": "Status", "value": "Sleeping"},
        ]}))
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {parsed:?}"
    );
    let message = error_message(&parsed);
    for expected in [
        "Value 'Sleeping'",
        "at 'filters.2.member.value'",
        "failed to satisfy constraint",
        "Pending",
    ] {
        assert!(
            message.contains(expected),
            "case {case:?}: expected the refusal to carry {expected:?}, got {message:?}"
        );
    }

    // "You can't specify a managed node ID in the same command that you specify Status =
    // Pending" (`ListCommandsRequest$InstanceId`), while the same node id beside any other
    // status is served.
    let case = "a node id beside a Pending status filter";
    let (status, parsed) = history
        .list(&json!({
            "InstanceId": LINUX,
            "Filters": [{"key": "Status", "value": "Pending"}],
        }))
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {parsed:?}"
    );
    let message = error_message(&parsed);
    assert!(
        message.contains("Pending"),
        "case {case:?}: expected the refusal to state the rule, got {message:?}"
    );
    let case = "a node id beside another status filter";
    let listed = history
        .listed(
            case,
            &json!({
                "InstanceId": WINDOWS,
                "Filters": [{"key": "Status", "value": "Success"}],
            }),
        )
        .await;
    assert_eq!(
        listed,
        vec![history.agent.id.clone()],
        "case {case:?}: expected the Success command of that node, got {listed:?}"
    );
}

/// A command Run Command no longer retains is not reported, and the read is what drops it:
/// the retention window is owed by the clock whether or not anybody called.
#[tokio::test]
async fn a_command_past_its_retention_window_is_not_reported() {
    let history = history().await;

    // One row per side of the window: just inside it, and just past it.
    let inside = SystemTime::now() - commands::COMMAND_HISTORY_RETENTION + HOUR;
    with_record(&history.engine, REGION, &history.shell.id, |record| {
        record.requested_at = inside;
    });
    let case = "just inside the retention window";
    let listed = history.listed(case, &json!({})).await;
    assert!(
        listed.contains(&history.shell.id),
        "case {case:?}: expected the command still reported, got {listed:?}"
    );

    let past = SystemTime::now() - commands::COMMAND_HISTORY_RETENTION - HOUR;
    with_record(&history.engine, REGION, &history.shell.id, |record| {
        record.requested_at = past;
    });
    let case = "just past the retention window";
    let listed = history.listed(case, &json!({})).await;
    assert!(
        !listed.contains(&history.shell.id),
        "case {case:?}: expected the command no longer reported, got {listed:?}"
    );
    assert!(
        history
            .engine
            .state::<ComAmazonawsSsmAmazonssmState>()
            .lock()
            .command(REGION, &history.shell.id)
            .is_none(),
        "case {case:?}: expected the store to have dropped the command the clock owed"
    );
    // The command's id now names no command of the Region, which is the refusal a
    // `CommandId` naming none carries.
    let (status, parsed) = history
        .list(&json!({"CommandId": history.shell.id}))
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {parsed:?}"
    );
    assert_eq!(
        error_code(&parsed),
        "InvalidCommandId",
        "case {case:?}: expected InvalidCommandId, got {parsed:?}"
    );
}

/// Every invocation a listed command reports a target count for reads back through
/// `GetCommandInvocation`, so the history and the per-node read report the one command.
#[tokio::test]
async fn a_listed_command_resolves_the_invocations_it_counts() {
    let history = history().await;

    let case = "the command sent to two named nodes";
    let (status, parsed) = history
        .list(&json!({"CommandId": history.shell.id}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {parsed:?}"
    );
    let command = parsed["Commands"][0].clone();
    assert_eq!(
        command["TargetCount"].as_i64(),
        Some(2),
        "case {case:?}: expected two targets, got {command:?}"
    );

    for node in [LINUX, HYBRID] {
        let (status, invocation) = call_in_region(
            &history.router,
            REGION,
            "GetCommandInvocation",
            &json!({"CommandId": history.shell.id, "InstanceId": node}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {node:?}: expected HTTP 200, got {status} with {invocation:?}"
        );
        assert_eq!(
            invocation["DocumentName"].as_str(),
            command["DocumentName"].as_str(),
            "case {node:?}: expected the document the history reported, got {invocation:?}"
        );
        assert_eq!(
            invocation["Status"].as_str(),
            Some("Pending"),
            "case {node:?}: expected the invocation nothing has delivered, got {invocation:?}"
        );
    }

    // A node the command holds no invocation for is not one of the targets it counted.
    let (status, invocation) = call_in_region(
        &history.router,
        REGION,
        "GetCommandInvocation",
        &json!({"CommandId": history.shell.id, "InstanceId": IDLE}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {IDLE:?}: expected HTTP 400, got {status} with {invocation:?}"
    );
    assert_eq!(
        error_code(&invocation),
        "InvocationDoesNotExist",
        "case {IDLE:?}: expected InvocationDoesNotExist, got {invocation:?}"
    );
}
