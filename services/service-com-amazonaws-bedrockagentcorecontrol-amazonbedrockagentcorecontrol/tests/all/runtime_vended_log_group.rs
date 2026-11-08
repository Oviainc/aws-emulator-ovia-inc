// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch Logs group this service VENDS for an agent runtime.
//!
//! The rule under test: a served `CreateAgentRuntime` provisions the runtime's managed log
//! group in CloudWatch Logs as part of the create, under the name derived from the
//! `agentRuntimeId` it minted — the runtimes prefix, that id, and the default endpoint the
//! create establishes — with NO retention configured, so the events never expire; and the
//! group belongs to the ACCOUNT rather than to the runtime, so `DeleteAgentRuntime` leaves
//! it (and anything in it) where it is. A client that reads the id a create answers with
//! and then tails the runtime therefore finds the group, both while the runtime lives and
//! after it is gone.
//!
//! Nothing in `CreateAgentRuntime`'s modelled output names the group
//! (`smithy_query(query="operation", operation="CreateAgentRuntime")` reports
//! `agentRuntimeArn`, `agentRuntimeId`, `agentRuntimeVersion`, `createdAt`, `status` and
//! `workloadIdentityDetails`), which is exactly why the name has to be derivable: the
//! client rebuilds it from the id.
//!
//! Every suite here mounts the emulator's own CloudWatch Logs service beside the control
//! plane on ONE engine — what the assembled emulator does (`core/router`'s `build_router`
//! registers every service crate) — and reads the group through CloudWatch Logs' own front
//! door (`DescribeLogGroups`), so what is asserted is a group that service really holds.
//!
//! The rows span what the rule generalises over: the runtime's name (the shortest and the
//! longest `AgentRuntimeName` admits, and the other characters its pattern admits), the
//! artifact union variant the runtime runs, the Region the request is signed for, and how
//! many runtimes one account holds. The recorded case is one row.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as agentcore;

/// The Region the rows are signed for unless one names another.
const REGION: &str = "us-east-1";

/// The prefix CloudWatch Logs holds an AgentCore Runtime's vended logs under, and the
/// endpoint suffix the group a create vends carries. Spelled out here rather than read off
/// the implementation, so a drift in either is a failing test.
const RUNTIME_LOG_GROUP_PREFIX: &str = "/aws/bedrock-agentcore/runtimes/";
const DEFAULT_ENDPOINT: &str = "DEFAULT";

/// The longest name `AgentRuntimeName` admits: its pattern is
/// `^[a-zA-Z][a-zA-Z0-9_]{0,47}$` (`smithy_query(query="operation",
/// operation="CreateAgentRuntime")`), so 48 characters.
const MAX_RUNTIME_NAME_LEN: usize = 48;

/// The bounds `LogGroupName` places on the name the group is created under: length 1..512,
/// pattern `^[\.\-_/#A-Za-z0-9]+$`. Source: `smithy_query(query="shape",
/// shape="LogGroupName",
/// model_path="services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/\
/// com-amazonaws-cloudwatchlogs-logs-20140328.json")`.
const MAX_LOG_GROUP_NAME_LEN: usize = 512;

/// A `clientToken` within `ClientToken`'s modelled 33..256 length and its
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$` pattern.
const CLIENT_TOKEN: &str = "vended-log-group-create-token-0001";

/// The execution role every row names. No IAM is mounted, so nothing is known about the
/// role and the role gate refuses nothing: what each row turns on is the log group.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AgentRuntimeRole";

/// The emulator the rows run against: the control plane, and CloudWatch Logs beside it on
/// the one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// An emulator with CloudWatch Logs answering, which is what the assembled emulator is.
    fn with_cloudwatch_logs() -> Self {
        Self::over_engine(true)
    }

    /// An emulator with ONLY the control plane mounted: there is no CloudWatch Logs to vend
    /// a group in.
    fn without_cloudwatch_logs() -> Self {
        Self::over_engine(false)
    }

    fn over_engine(cloudwatch_logs: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // `default()` registers no authenticator, so the suite exercises routing and the
        // handlers rather than this process's SigV4 keys.
        let mut router = MultiServiceRouter::default();
        router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
        if cloudwatch_logs {
            router.register_service(
                service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(
                    Arc::clone(&engine),
                ),
            );
        }
        Self { router }
    }

    /// One `restJson1` request to the control plane, signed for `region` — which is what
    /// the router routes on and what the handlers read the Region from.
    async fn control(
        &self,
        method: &str,
        uri: &str,
        region: &str,
        body: Option<&Value>,
    ) -> Answer {
        let payload = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
        });
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/\
             aws4_request, SignedHeaders=host, Signature=abc"
        );
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", authorization)
            .header("host", "bedrock-agentcore-control.amazonaws.com");
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request.body(payload).expect("a well-formed request");
        Answer::of(self.router.dispatch(request).await)
    }

    async fn create_runtime(&self, region: &str, body: &Value) -> Answer {
        self.control("PUT", "/runtimes/", region, Some(body)).await
    }

    async fn get_runtime(&self, region: &str, id: &str) -> Answer {
        self.control("GET", &format!("/runtimes/{id}/"), region, None)
            .await
    }

    async fn delete_runtime(&self, region: &str, id: &str) -> Answer {
        self.control("DELETE", &format!("/runtimes/{id}/"), region, None)
            .await
    }

    /// One CloudWatch Logs call (`awsJson1_1`, `Logs_20140328.<Operation>`) signed for
    /// `region`. The credential scope names `logs`, which is what the router claims on.
    async fn logs(&self, region: &str, operation: &str, body: &Value) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/logs/aws4_request, \
             SignedHeaders=host, Signature=abc"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://logs.{region}.amazonaws.com/"))
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Logs_20140328.{operation}"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        Answer::of(self.router.dispatch(request).await)
    }

    /// Every log group CloudWatch Logs reports under `prefix` in `region`, as the client
    /// reads them.
    async fn log_groups(&self, case: &str, region: &str, prefix: &str) -> Vec<Value> {
        let answer = self
            .logs(region, "DescribeLogGroups", &json!({"logGroupNamePrefix": prefix}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from DescribeLogGroups, got {} and body {}",
            answer.status, answer.body
        );
        answer.body["logGroups"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// The `logGroupName` of every group reported under `prefix`, in the order reported.
    async fn log_group_names(&self, case: &str, region: &str, prefix: &str) -> Vec<String> {
        self.log_groups(case, region, prefix)
            .await
            .iter()
            .filter_map(|group| group["logGroupName"].as_str().map(str::to_string))
            .collect()
    }
}

/// One answer from the emulator: its status and its JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn of(response: http::Response<Bytes>) -> Self {
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Self { status, body }
    }

    /// The error code `restJson1` and `awsJson1_1` both carry in `__type`.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn id(&self, case: &str) -> String {
        self.body["agentRuntimeId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a served create reports an agentRuntimeId, got body {}",
                    self.body
                )
            })
            .to_string()
    }
}

/// The artifact a row's runtime runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Artifact {
    /// A `containerConfiguration`, whose URI satisfies `RuntimeContainerUri`'s ECR pattern.
    Container,
    /// A `codeConfiguration` naming an object-storage location. No object storage is
    /// mounted here, so nothing is known about the object and the create is served: what
    /// the row turns on is the log group, not the artifact.
    Code,
}

impl Artifact {
    fn body(self) -> Value {
        match self {
            Self::Container => json!({"containerConfiguration": {
                "containerUri": "public.ecr.aws/agentcore/demo:v1"
            }}),
            // `runtime` is a value of `AgentManagedRuntimeType` and `entryPoint` is within
            // `EntryPoints`' bound (`smithy_query(query="shape",
            // shape="CodeConfiguration")`).
            Self::Code => json!({"codeConfiguration": {
                "code": {"s3": {"bucket": "agentcore-artifacts", "prefix": "agent/code.zip"}},
                "runtime": "PYTHON_3_13",
                "entryPoint": ["main.py"],
            }}),
        }
    }
}

/// One runtime a row creates: what it is called, what it runs and where it is created.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    name: String,
    artifact: Artifact,
    region: &'static str,
}

/// The runtimes the rule is asserted over. One row per thing it generalises over: the
/// shortest name `AgentRuntimeName` admits, a name carrying the digits and underscores its
/// pattern admits, the longest name it admits, each artifact variant, and a Region that is
/// not this emulator's default.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the shortest name the model admits",
            name: "a".to_string(),
            artifact: Artifact::Container,
            region: REGION,
        },
        Row {
            case: "the digits and underscores the name pattern admits",
            name: "agent_runtime_2026_v1".to_string(),
            artifact: Artifact::Code,
            region: REGION,
        },
        Row {
            case: "the longest name the model admits",
            name: format!("l{}", "o".repeat(MAX_RUNTIME_NAME_LEN - 1)),
            artifact: Artifact::Container,
            region: REGION,
        },
        Row {
            case: "a Region that is not the emulator's default",
            name: "west_agent".to_string(),
            artifact: Artifact::Code,
            region: "eu-west-2",
        },
    ]
}

/// The create request for `name` carrying `artifact`.
fn create_request(name: &str, artifact: Artifact) -> Value {
    json!({
        "agentRuntimeName": name,
        "agentRuntimeArtifact": artifact.body(),
        "roleArn": ROLE_ARN,
        "networkConfiguration": {"networkMode": "PUBLIC"},
        "protocolConfiguration": {"serverProtocol": "HTTP"},
    })
}

/// The group a client derives for the runtime `agent_runtime_id`.
fn vended_group(agent_runtime_id: &str) -> String {
    format!("{RUNTIME_LOG_GROUP_PREFIX}{agent_runtime_id}-{DEFAULT_ENDPOINT}")
}

/// Assert `name` is a name CloudWatch Logs admits for a group.
fn assert_log_group_name_is_admissible(case: &str, name: &str) {
    assert!(
        (1..=MAX_LOG_GROUP_NAME_LEN).contains(&name.chars().count()),
        "case {case:?}: expected a name inside LogGroupName's 1..{MAX_LOG_GROUP_NAME_LEN} \
         length, got {} characters ({name:?})",
        name.chars().count()
    );
    assert!(
        name.chars()
            .all(|character| character.is_ascii_alphanumeric()
                || matches!(character, '.' | '-' | '_' | '/' | '#')),
        "case {case:?}: every character of a log group's name must satisfy LogGroupName's \
         pattern, got {name:?}"
    );
}

/// Assert exactly one group exists under the name a client derives for `id`, that it is
/// that name, and that it carries NO `retentionInDays` — the group never expires.
async fn assert_vended_group(emulator: &Emulator, case: &str, region: &str, id: &str) {
    let expected = vended_group(id);
    assert_log_group_name_is_admissible(case, &expected);
    let groups = emulator.log_groups(case, region, &expected).await;
    assert_eq!(
        groups.len(),
        1,
        "case {case:?}: expected exactly the runtime's vended group {expected:?} under that \
         prefix, got {groups:?}"
    );
    assert_eq!(
        groups[0]["logGroupName"],
        json!(expected),
        "case {case:?}: expected the vended group {expected:?}, got {}",
        groups[0]
    );
    assert!(
        groups[0].get("retentionInDays").is_none(),
        "case {case:?}: a vended runtime log group carries no retention, so its events never \
         expire, got {}",
        groups[0]
    );
}

/// A served `CreateAgentRuntime` vends the runtime's log group: CloudWatch Logs reports
/// exactly the group a client derives from the minted id, with no retention, and the
/// runtime itself reads back through `GetAgentRuntime`.
#[tokio::test]
async fn a_created_runtime_has_its_vended_log_group() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::with_cloudwatch_logs();
        // Nothing is vended before the create: the group is the create's own effect.
        let before = emulator
            .log_group_names(case, row.region, RUNTIME_LOG_GROUP_PREFIX)
            .await;
        assert!(
            before.is_empty(),
            "case {case:?}: no runtime log group may exist before the create, got {before:?}"
        );

        let created = emulator
            .create_runtime(row.region, &create_request(&row.name, row.artifact))
            .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected HTTP 202 from CreateAgentRuntime, got {} and body {}",
            created.status, created.body
        );
        let id = created.id(case);

        assert_vended_group(&emulator, case, row.region, &id).await;

        // The round trip: the runtime the group was vended for is the one the control
        // plane reports, under the very id the group's name is derived from.
        let read = emulator.get_runtime(row.region, &id).await;
        assert_eq!(
            (read.status, read.body["agentRuntimeId"].clone()),
            (200, json!(id)),
            "case {case:?}: GetAgentRuntime must report the created runtime, got {} and body {}",
            read.status,
            read.body
        );

        // One runtime, one group: nothing else was provisioned under the prefix.
        let groups = emulator
            .log_group_names(case, row.region, RUNTIME_LOG_GROUP_PREFIX)
            .await;
        assert_eq!(
            groups,
            vec![vended_group(&id)],
            "case {case:?}: exactly one group is vended for one runtime, got {groups:?}"
        );
    }
}

/// Every runtime of one account is vended its OWN group, named for its own id — including
/// two runtimes whose names are a prefix of one another, and a name created again after the
/// first runtime was deleted.
#[tokio::test]
async fn every_runtime_is_vended_its_own_group() {
    let case = "several runtimes in one account";
    let emulator = Emulator::with_cloudwatch_logs();
    let mut expected: Vec<String> = Vec::new();

    for (name, artifact) in [
        ("agent", Artifact::Container),
        // A name the first is a prefix of: the group is named for the ID, so the two
        // cannot collide.
        ("agent2", Artifact::Code),
        ("other_agent", Artifact::Container),
    ] {
        let created = emulator
            .create_runtime(REGION, &create_request(name, artifact))
            .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected HTTP 202 from CreateAgentRuntime for {name:?}, got {} and \
             body {}",
            created.status, created.body
        );
        expected.push(vended_group(&created.id(case)));
    }

    // A runtime deleted and created again under the IDENTICAL request is a NEW runtime, so
    // it is vended a group of its own — and the first one's group is still there.
    let first = expected[0].clone();
    let deleted_id = first
        .trim_start_matches(RUNTIME_LOG_GROUP_PREFIX)
        .trim_end_matches(&format!("-{DEFAULT_ENDPOINT}"))
        .to_string();
    let deleted = emulator.delete_runtime(REGION, &deleted_id).await;
    assert_eq!(
        deleted.status, 202,
        "case {case:?}: expected HTTP 202 from DeleteAgentRuntime, got {} and body {}",
        deleted.status, deleted.body
    );
    let recreated = emulator
        .create_runtime(REGION, &create_request("agent", Artifact::Container))
        .await;
    assert_eq!(
        recreated.status, 202,
        "case {case:?}: the deleted name must be free again, got {} and body {}",
        recreated.status, recreated.body
    );
    expected.push(vended_group(&recreated.id(case)));

    let mut want = expected.clone();
    want.sort();
    want.dedup();
    assert_eq!(
        want.len(),
        expected.len(),
        "case {case:?}: no two runtimes may be handed one vended group, got {expected:?}"
    );
    // Every one of them exists, and nothing else does: `DescribeLogGroups` reports the
    // groups in name order.
    let groups = emulator
        .log_group_names(case, REGION, RUNTIME_LOG_GROUP_PREFIX)
        .await;
    assert_eq!(
        groups, want,
        "case {case:?}: expected exactly the groups vended for this account's runtimes, got \
         {groups:?}"
    );
}

/// `DeleteAgentRuntime` does NOT cascade into CloudWatch Logs: the runtime goes, and its
/// log group stays, for every row — the logs of a runtime that is gone are what an
/// operator diagnosing its last session reads.
#[tokio::test]
async fn deleting_the_runtime_retains_its_vended_log_group() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::with_cloudwatch_logs();
        let created = emulator
            .create_runtime(row.region, &create_request(&row.name, row.artifact))
            .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected HTTP 202 from CreateAgentRuntime, got {} and body {}",
            created.status, created.body
        );
        let id = created.id(case);
        assert_vended_group(&emulator, case, row.region, &id).await;

        let deleted = emulator.delete_runtime(row.region, &id).await;
        assert_eq!(
            deleted.status, 202,
            "case {case:?}: expected HTTP 202 from DeleteAgentRuntime, got {} and body {}",
            deleted.status, deleted.body
        );
        // The runtime is gone...
        let read = emulator.get_runtime(row.region, &id).await;
        assert_eq!(
            (read.status, read.error_code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: a deleted runtime is not found, got {} and body {}",
            read.status,
            read.body
        );
        // ...and its group, with no retention still, is not.
        assert_vended_group(&emulator, case, row.region, &id).await;
    }
}

/// A create nothing minted a runtime for vends no group: a replayed `clientToken` answers
/// with the runtime it already made (and its one group), and a name already taken is
/// refused with no second group.
#[tokio::test]
async fn a_create_that_mints_no_runtime_vends_no_group() {
    let case = "a replayed client token and a name conflict";
    let emulator = Emulator::with_cloudwatch_logs();
    let mut request = create_request("replayed_agent", Artifact::Container);
    request["clientToken"] = json!(CLIENT_TOKEN);

    let first = emulator.create_runtime(REGION, &request).await;
    assert_eq!(
        first.status, 202,
        "case {case:?}: expected HTTP 202 from CreateAgentRuntime, got {} and body {}",
        first.status, first.body
    );
    let id = first.id(case);

    // The replay answers with the runtime the token already produced...
    let replayed = emulator.create_runtime(REGION, &request).await;
    assert_eq!(
        (replayed.status, replayed.body["agentRuntimeId"].clone()),
        (202, json!(id)),
        "case {case:?}: a replayed clientToken answers with the runtime it first made, got {} \
         and body {}",
        replayed.status,
        replayed.body
    );

    // ...and a request naming a name already taken is refused.
    let conflicting = emulator
        .create_runtime(
            REGION,
            &create_request("replayed_agent", Artifact::Container),
        )
        .await;
    assert_eq!(
        (conflicting.status, conflicting.error_code()),
        (409, "ConflictException".to_string()),
        "case {case:?}: a repeated runtime name is a conflict, got {} and body {}",
        conflicting.status,
        conflicting.body
    );

    // Through all of it: one runtime, one group.
    let groups = emulator
        .log_group_names(case, REGION, RUNTIME_LOG_GROUP_PREFIX)
        .await;
    assert_eq!(
        groups,
        vec![vended_group(&id)],
        "case {case:?}: only the one runtime that was minted may have a group, got {groups:?}"
    );
}

/// The vended group carries no retention BECAUSE none was put on it: a client that puts
/// one on the very same group is then reported with it, so the absent member is the
/// group's state and not something the read leaves out.
#[tokio::test]
async fn the_vended_group_reports_the_retention_a_client_puts_on_it() {
    let case = "retention put on the vended group";
    let emulator = Emulator::with_cloudwatch_logs();
    let created = emulator
        .create_runtime(REGION, &create_request("retained_agent", Artifact::Code))
        .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: expected HTTP 202 from CreateAgentRuntime, got {} and body {}",
        created.status, created.body
    );
    let id = created.id(case);
    assert_vended_group(&emulator, case, REGION, &id).await;

    // 30 is a value of CloudWatch Logs' own documented retention set
    // (`smithy_query(query="doc", shape="PutRetentionPolicyRequest",
    // member="retentionInDays")`).
    let group = vended_group(&id);
    let put = emulator
        .logs(
            REGION,
            "PutRetentionPolicy",
            &json!({"logGroupName": group, "retentionInDays": 30}),
        )
        .await;
    assert_eq!(
        put.status, 200,
        "case {case:?}: expected HTTP 200 from PutRetentionPolicy on the vended group, got {} \
         and body {}",
        put.status, put.body
    );
    let groups = emulator.log_groups(case, REGION, &group).await;
    assert_eq!(
        groups.len(),
        1,
        "case {case:?}: expected the one vended group, got {groups:?}"
    );
    assert_eq!(
        groups[0]["retentionInDays"],
        json!(30),
        "case {case:?}: the group a client put retention on reports it, so the absence asserted \
         elsewhere is the group's own state, got {}",
        groups[0]
    );
}

/// An emulator assembled WITHOUT CloudWatch Logs cannot vend anything, so the create is
/// still served and the runtime still reads back: the caller asked for a runtime and never
/// for a log group, and the answer CloudWatch Logs' absence produces says nothing about
/// the request.
#[tokio::test]
async fn an_emulator_without_cloudwatch_logs_still_serves_the_create() {
    let case = "no CloudWatch Logs mounted";
    let emulator = Emulator::without_cloudwatch_logs();
    let created = emulator
        .create_runtime(
            REGION,
            &create_request("unvended_agent", Artifact::Container),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: expected HTTP 202 from CreateAgentRuntime, got {} and body {}",
        created.status, created.body
    );
    let id = created.id(case);
    let read = emulator.get_runtime(REGION, &id).await;
    assert_eq!(
        (read.status, read.body["agentRuntimeId"].clone()),
        (200, json!(id)),
        "case {case:?}: the runtime must be stored whatever the emulator mounts beside this \
         service, got {} and body {}",
        read.status,
        read.body
    );
}
