// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Network Flow Monitor scope + monitor slice, driven
//! over the service's own `restJson1` wire protocol through the same
//! `MultiServiceRouter` the production emulator dispatches with.
//!
//! Every test asserts a RULE over a SET of inputs — the shapes the bootstrap issue
//! records are one row each: a create answers from the record it just stored, a
//! read answers that same record, a delete is observable by the next reader, and a
//! page token walks the store exactly once.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_networkflowmonitor_networkflowmonitor as nfm_service;
use smithy_server_runtime::MultiServiceRouter;

/// This service's SigV4 signing name, which is also the router's route key.
pub(crate) const SIGNING_NAME: &str = "networkflowmonitor";

/// The canonical emulator account id every ARN carries.
pub(crate) const ACCOUNT_ID: &str = "000000000000";

/// The Region the request's SigV4 credential scope names in most tests.
/// Deliberately not `us-east-1` (the store's fallback), so an ARN built from the
/// request's scope is distinguishable from a hardcoded one.
pub(crate) const REGION: &str = "us-west-2";

/// The statuses `MonitorStatus` declares, from the model
/// (`src/ops/types/shared_types.rs`: `MONITOR_STATUS_VALUES`). Used to assert the
/// `monitorStatus` list filter over the whole published set rather than one value.
const MONITOR_STATUS_VALUES: &[&str] = &["ACTIVE", "DELETING", "ERROR", "INACTIVE", "PENDING"];

/// The local resource types `MonitorLocalResourceType` declares, from the model
/// (`MONITOR_LOCAL_RESOURCE_TYPE_VALUES`).
const LOCAL_RESOURCE_TYPES: &[&str] = &[
    "AWS::AvailabilityZone",
    "AWS::EC2::Subnet",
    "AWS::EC2::VPC",
    "AWS::EKS::Cluster",
    "AWS::Region",
];

/// Longer than `state::PROVISIONING_DELAY`, so a call after this wait is one the
/// clock already owes the terminal status to.
const PAST_PROVISIONING: Duration = Duration::from_millis(900);

/// One emulator holding exactly this service, dispatched over HTTP.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    region: String,
}

/// A dispatched response: status, the `x-amzn-errortype` header (empty on success)
/// and the parsed JSON body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error_type: String,
    pub(crate) body: Value,
}

impl Answer {
    pub(crate) fn member(&self, name: &str) -> &Value {
        self.body.get(name).unwrap_or(&Value::Null)
    }

    pub(crate) fn text(&self, name: &str) -> String {
        self.member(name).as_str().unwrap_or_default().to_string()
    }

    fn number(&self, name: &str) -> f64 {
        self.member(name).as_f64().unwrap_or(f64::NAN)
    }
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::new_in(REGION)
    }

    pub(crate) fn new_in(region: &str) -> Self {
        // The SINGLE shared engine, exactly as `router/src/lib.rs` builds it.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(nfm_service::EmulatedService::new(engine));
        Self {
            router,
            region: region.to_string(),
        }
    }

    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        let region = &self.region;
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{region}/{SIGNING_NAME}/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000"
        );
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(&value).expect("a serializable request body")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error_type,
            body,
        }
    }

    pub(crate) async fn create_scope(&self, body: Value) -> Answer {
        self.call("POST", "/scopes", Some(body)).await
    }

    pub(crate) async fn get_scope(&self, scope_id: &str) -> Answer {
        self.call("GET", &format!("/scopes/{scope_id}"), None).await
    }

    async fn delete_scope(&self, scope_id: &str) -> Answer {
        self.call("DELETE", &format!("/scopes/{scope_id}"), None)
            .await
    }

    pub(crate) async fn create_monitor(&self, body: Value) -> Answer {
        self.call("POST", "/monitors", Some(body)).await
    }

    pub(crate) async fn get_monitor(&self, name: &str) -> Answer {
        self.call("GET", &format!("/monitors/{name}"), None).await
    }

    async fn list_monitors(&self, query: &str) -> Answer {
        self.call("GET", &format!("/monitors{query}"), None).await
    }

    pub(crate) async fn delete_monitor(&self, name: &str) -> Answer {
        self.call("DELETE", &format!("/monitors/{name}"), None).await
    }

    /// A scope holding one target, and the `scopeArn` it was minted with — the
    /// parent every monitor test needs.
    pub(crate) async fn scope_for_monitors(&self) -> String {
        let created = self
            .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
            .await;
        assert_eq!(
            created.status, 200,
            "the parent scope must be created: got {} with body {}",
            created.status, created.body
        );
        created.text("scopeArn")
    }
}

/// A `clientToken` matching `UuidString`'s pattern
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, derived from
/// `seed` so each one in a test is distinct.
pub(crate) fn client_token(seed: u32) -> String {
    format!("{seed:08x}-abcd-4ef0-8123-{seed:012x}")
}

/// `targets` as the wire carries them: a `TargetResource` per (account, Region)
/// pair, with the only `targetType` the model declares (`ACCOUNT`) and the only
/// `TargetId` member it declares (`accountId`).
pub(crate) fn targets(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(account_id, region)| {
                json!({
                    "targetIdentifier": {
                        "targetId": { "accountId": account_id },
                        "targetType": "ACCOUNT"
                    },
                    "region": region
                })
            })
            .collect(),
    )
}

/// `ScopeId` carries no pattern, but the ids this service mints are UUID-shaped:
/// 8-4-4-4-12 lowercase hex.
fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len && group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            })
}

/// One row of the scope create-and-read-back table.
struct ScopeRow {
    case: &'static str,
    body: Value,
}

fn scope_rows() -> Vec<ScopeRow> {
    // 25 targets: the count dimension, well inside `TargetResourceList`'s 1..100.
    let many: Vec<(String, String)> = (0..25)
        .map(|index| (format!("{:012}", 100_000_000_000u64 + index), format!("us-west-{}", index % 2 + 1)))
        .collect();
    let many_refs: Vec<(&str, &str)> = many
        .iter()
        .map(|(account, region)| (account.as_str(), region.as_str()))
        .collect();
    vec![
        ScopeRow {
            case: "one target, no clientToken and no tags",
            body: json!({ "targets": targets(&[("123456789012", "us-east-1")]) }),
        },
        ScopeRow {
            case: "the issue's recorded shape, with a clientToken and tags",
            body: json!({
                "targets": targets(&[("123456789012", "us-east-1")]),
                "clientToken": client_token(1),
                "tags": { "owner": "net-team", "env": "prod" }
            }),
        },
        ScopeRow {
            case: "three targets spanning accounts and Regions",
            body: json!({
                "targets": targets(&[
                    ("111111111111", "eu-west-1"),
                    ("222222222222", "ap-southeast-2"),
                    ("333333333333", "us-east-2"),
                ]),
                "tags": { "cost-center": "4711" }
            }),
        },
        ScopeRow {
            case: "25 targets: the list's count dimension",
            body: json!({ "targets": targets(&many_refs) }),
        },
    ]
}

#[tokio::test]
async fn a_created_scope_is_read_back_member_for_member() {
    let emulator = Emulator::new();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    for row in scope_rows() {
        let created = emulator.create_scope(row.body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200, got {} with body {}",
            row.case, created.status, created.body
        );

        let scope_id = created.text("scopeId");
        assert!(
            is_uuid_shaped(&scope_id),
            "case {:?}: expected a UUID-shaped scopeId, got {scope_id:?}",
            row.case
        );
        assert!(
            seen_ids.insert(scope_id.clone()),
            "case {:?}: expected a fresh scopeId, got the already-minted {scope_id:?}",
            row.case
        );
        assert_eq!(
            created.text("scopeArn"),
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:scope/{scope_id}"),
            "case {:?}: expected the ARN of {scope_id}, got {:?}",
            row.case,
            created.text("scopeArn")
        );
        // `MonitorStatus`' PENDING is documented as "in the process of being
        // created"; `ScopeStatus`' IN_PROGRESS is its scope counterpart, so a scope
        // that was just asked for is not yet SUCCEEDED.
        assert_eq!(
            created.text("status"),
            "IN_PROGRESS",
            "case {:?}: a scope is set up before it succeeds, got {:?}",
            row.case,
            created.text("status")
        );
        assert_eq!(
            created.member("tags"),
            row.body.get("tags").unwrap_or(&Value::Null),
            "case {:?}: expected the submitted tags echoed from the record, got {:?}",
            row.case,
            created.member("tags")
        );

        let read = emulator.get_scope(&scope_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the stored scope, got {} with body {}",
            row.case, read.status, read.body
        );
        assert_eq!(
            read.text("scopeId"),
            scope_id,
            "case {:?}: expected scopeId {scope_id}, got {:?}",
            row.case,
            read.text("scopeId")
        );
        assert_eq!(
            read.text("scopeArn"),
            created.text("scopeArn"),
            "case {:?}: expected the stored scopeArn, got {:?}",
            row.case,
            read.text("scopeArn")
        );
        assert_eq!(
            read.member("targets"),
            row.body.get("targets").expect("every row carries targets"),
            "case {:?}: expected the targets exactly as submitted, got {:?}",
            row.case,
            read.member("targets")
        );
        assert_eq!(
            read.member("tags"),
            row.body.get("tags").unwrap_or(&Value::Null),
            "case {:?}: expected the stored tags, got {:?}",
            row.case,
            read.member("tags")
        );
    }
}

#[tokio::test]
async fn a_scope_arn_carries_the_region_the_request_is_signed_for() {
    // The ARN's Region segment comes from the request's credential scope, not from
    // a constant: the same create under three scopes mints three ARNs.
    for region in ["us-east-1", "eu-central-1", "ap-northeast-1"] {
        let emulator = Emulator::new_in(region);
        let created = emulator
            .create_scope(json!({ "targets": targets(&[("123456789012", region)]) }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: expected 200, got {} with body {}",
            created.status, created.body
        );
        let scope_id = created.text("scopeId");
        assert_eq!(
            created.text("scopeArn"),
            format!("arn:aws:{SIGNING_NAME}:{region}:{ACCOUNT_ID}:scope/{scope_id}"),
            "case {region:?}: expected an ARN in the signed Region, got {:?}",
            created.text("scopeArn")
        );
    }
}

#[tokio::test]
async fn a_scope_reaches_succeeded_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let created = emulator
        .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
        .await;
    let scope_id = created.text("scopeId");
    assert_eq!(
        created.text("status"),
        "IN_PROGRESS",
        "a fresh scope reports IN_PROGRESS, got {:?}",
        created.text("status")
    );
    // A caller acting straight after the create still sees the transient status.
    let immediate = emulator.get_scope(&scope_id).await;
    assert_eq!(
        immediate.text("status"),
        "IN_PROGRESS",
        "case {scope_id:?}: the transient status is observable, got {:?}",
        immediate.text("status")
    );

    tokio::time::sleep(PAST_PROVISIONING).await;

    let settled = emulator.get_scope(&scope_id).await;
    assert_eq!(
        settled.text("status"),
        "SUCCEEDED",
        "case {scope_id:?}: the clock owes the scope SUCCEEDED, got {:?}",
        settled.text("status")
    );
    // A read never changes what the next reader sees.
    let again = emulator.get_scope(&scope_id).await;
    assert_eq!(
        again.body, settled.body,
        "case {scope_id:?}: two reads of one scope must answer the same body, got {} then {}",
        settled.body, again.body
    );
}

#[tokio::test]
async fn get_scope_on_an_id_the_store_does_not_hold_is_404() {
    let emulator = Emulator::new();
    // One real scope exists, so a 404 is about the id and not about an empty store.
    let created = emulator
        .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
        .await;
    let known = created.text("scopeId");

    for case in [
        "00000000-0000-4000-8000-000000000000",
        "ffffffff-ffff-4fff-8fff-ffffffffffff",
        "not-a-scope-id",
        "0",
    ] {
        assert_ne!(case, known, "the unknown-id table must not name a real scope");
        let answer = emulator.get_scope(case).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ResourceNotFoundException")
                || answer
                    .text("__type")
                    .contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?} / body {}",
            answer.error_type,
            answer.body
        );
    }
}

#[tokio::test]
async fn one_client_token_creates_one_scope() {
    let emulator = Emulator::new();
    let token = client_token(7);
    let body = json!({
        "targets": targets(&[("444444444444", "us-east-1"), ("555555555555", "us-west-2")]),
        "clientToken": token,
        "tags": { "team": "net" }
    });

    let first = emulator.create_scope(body.clone()).await;
    assert_eq!(
        first.status, 200,
        "the first create must succeed, got {} with body {}",
        first.status, first.body
    );

    // Replaying the SAME token with the SAME parameters answers with the scope that
    // token already minted — three times over, not three scopes.
    for attempt in 0..3 {
        let replay = emulator.create_scope(body.clone()).await;
        assert_eq!(
            replay.status, 200,
            "attempt {attempt}: expected the replay to succeed, got {} with body {}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.text("scopeId"),
            first.text("scopeId"),
            "attempt {attempt}: expected the first scope {:?}, got {:?}",
            first.text("scopeId"),
            replay.text("scopeId")
        );
    }

    // A DIFFERENT token asks for another scope, so another one is minted.
    let other = emulator
        .create_scope(json!({
            "targets": targets(&[("444444444444", "us-east-1")]),
            "clientToken": client_token(8)
        }))
        .await;
    assert_ne!(
        other.text("scopeId"),
        first.text("scopeId"),
        "a different clientToken must mint a different scope, got {:?} twice",
        other.text("scopeId")
    );

    // The same token with DIFFERENT parameters is the modelled conflict, not a
    // silent replay of a scope the caller did not ask for.
    for (case, changed) in [
        (
            "different targets",
            json!({ "targets": targets(&[("666666666666", "us-east-1")]), "clientToken": token }),
        ),
        (
            "different tags",
            json!({
                "targets": targets(&[("444444444444", "us-east-1"), ("555555555555", "us-west-2")]),
                "clientToken": token,
                "tags": { "team": "other" }
            }),
        ),
    ] {
        let conflict = emulator.create_scope(changed).await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: expected 409, got {} with body {}",
            conflict.status, conflict.body
        );
    }
}

/// One row of the monitor create-and-read-back table.
struct MonitorRow {
    case: &'static str,
    name: &'static str,
    local_resources: Value,
    remote_resources: Option<Value>,
    tags: Option<Value>,
}

fn monitor_rows() -> Vec<MonitorRow> {
    vec![
        MonitorRow {
            case: "the issue's recorded shape: one VPC local resource, nothing else",
            name: "demo-monitor",
            local_resources: json!([{ "type": "AWS::EC2::VPC", "identifier": "vpc-123" }]),
            remote_resources: None,
            tags: None,
        },
        MonitorRow {
            case: "local and remote resources, plus tags",
            name: "monitor.with_remotes-2",
            local_resources: json!([
                { "type": "AWS::EC2::Subnet", "identifier": "arn:aws:ec2:us-west-2:000000000000:subnet/subnet-1" },
                { "type": "AWS::AvailabilityZone", "identifier": "us-west-2b" },
            ]),
            remote_resources: Some(json!([
                { "type": "AWS::AWSService", "identifier": "S3" },
                { "type": "AWS::Region", "identifier": "us-east-1" },
            ])),
            tags: Some(json!({ "owner": "net-team" })),
        },
        MonitorRow {
            case: "an EKS cluster local resource the issue does not cite",
            name: "eks-monitor",
            local_resources: json!([
                { "type": "AWS::EKS::Cluster", "identifier": "arn:aws:eks:us-west-2:000000000000:cluster/prod" },
            ]),
            remote_resources: Some(json!([])),
            tags: Some(json!({})),
        },
        MonitorRow {
            case: "a one-character name, the shortest ResourceName allows",
            name: "m",
            local_resources: json!([{ "type": "AWS::Region", "identifier": "us-west-2" }]),
            remote_resources: None,
            tags: None,
        },
    ]
}

#[tokio::test]
async fn a_created_monitor_is_read_back_member_for_member() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;

    for row in monitor_rows() {
        let mut request = HashMap::from([
            ("monitorName".to_string(), json!(row.name)),
            ("scopeArn".to_string(), json!(scope_arn)),
            ("localResources".to_string(), row.local_resources.clone()),
        ]);
        if let Some(remote) = row.remote_resources.clone() {
            request.insert("remoteResources".to_string(), remote);
        }
        if let Some(tags) = row.tags.clone() {
            request.insert("tags".to_string(), tags);
        }
        let created = emulator.create_monitor(json!(request)).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200, got {} with body {}",
            row.case, created.status, created.body
        );
        assert_eq!(
            created.text("monitorArn"),
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:monitor/{}",
                row.name
            ),
            "case {:?}: expected the ARN of {}, got {:?}",
            row.case,
            row.name,
            created.text("monitorArn")
        );
        // "PENDING: The monitor is in the process of being created."
        assert_eq!(
            created.text("monitorStatus"),
            "PENDING",
            "case {:?}: a fresh monitor is PENDING, got {:?}",
            row.case,
            created.text("monitorStatus")
        );
        assert_eq!(
            created.member("localResources"),
            &row.local_resources,
            "case {:?}: expected the submitted localResources, got {:?}",
            row.case,
            created.member("localResources")
        );
        // `remoteResources` is optional in, `@required` out: a monitor created
        // without any has an empty list.
        assert_eq!(
            created.member("remoteResources"),
            row.remote_resources.as_ref().unwrap_or(&json!([])),
            "case {:?}: expected the submitted remoteResources, got {:?}",
            row.case,
            created.member("remoteResources")
        );
        let created_at = created.number("createdAt");
        let modified_at = created.number("modifiedAt");
        assert!(
            created_at > 0.0,
            "case {:?}: expected a real creation timestamp, got {created_at}",
            row.case
        );
        assert_eq!(
            created_at, modified_at,
            "case {:?}: a monitor nothing has changed yet has modifiedAt == createdAt, got {created_at} and {modified_at}",
            row.case
        );

        let read = emulator.get_monitor(row.name).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the stored monitor, got {} with body {}",
            row.case, read.status, read.body
        );
        // GetMonitor answers the same record CreateMonitor answered from: every
        // member the clock cannot move is identical.
        for member in [
            "monitorArn",
            "monitorName",
            "localResources",
            "remoteResources",
            "tags",
            "createdAt",
        ] {
            assert_eq!(
                read.member(member),
                created.member(member),
                "case {:?}: expected the stored {member}, got {:?} after the create answered {:?}",
                row.case,
                read.member(member),
                created.member(member)
            );
        }
        // The only member the clock may have moved is the status, and only onto the
        // terminal one it owes — with `modifiedAt` moving exactly when it does.
        match read.text("monitorStatus").as_str() {
            "PENDING" => assert_eq!(
                read.number("modifiedAt"),
                created_at,
                "case {:?}: a still-PENDING monitor has modifiedAt == createdAt, got {}",
                row.case,
                read.number("modifiedAt")
            ),
            "ACTIVE" => assert!(
                read.number("modifiedAt") > created_at,
                "case {:?}: reaching ACTIVE moves modifiedAt past createdAt {created_at}, got {}",
                row.case,
                read.number("modifiedAt")
            ),
            other => panic!(
                "case {:?}: a monitor is PENDING and then ACTIVE, got {other:?}",
                row.case
            ),
        }
    }
}

#[tokio::test]
async fn a_monitor_reaches_active_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    let created = emulator
        .create_monitor(json!({
            "monitorName": "settling-monitor",
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        created.text("monitorStatus"),
        "PENDING",
        "a fresh monitor is PENDING, got {:?}",
        created.text("monitorStatus")
    );
    let created_at = created.number("createdAt");

    tokio::time::sleep(PAST_PROVISIONING).await;

    let settled = emulator.get_monitor("settling-monitor").await;
    assert_eq!(
        settled.text("monitorStatus"),
        "ACTIVE",
        "the clock owes the monitor ACTIVE, got {:?}",
        settled.text("monitorStatus")
    );
    assert_eq!(
        settled.number("createdAt"),
        created_at,
        "createdAt never moves, got {} then {}",
        created_at,
        settled.number("createdAt")
    );
    let modified_at = settled.number("modifiedAt");
    assert!(
        modified_at > created_at && modified_at - created_at <= 0.75,
        "the transition is due 250..750 ms after the create: createdAt {created_at}, modifiedAt {modified_at}"
    );
    let again = emulator.get_monitor("settling-monitor").await;
    assert_eq!(
        again.body, settled.body,
        "two reads of one monitor must answer the same body, got {} then {}",
        settled.body, again.body
    );
}

#[tokio::test]
async fn create_monitor_refuses_a_scope_arn_that_names_no_scope() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    // A real monitor, so its ARN is a live ARN of this service that is still not a
    // scope.
    let monitor = emulator
        .create_monitor(json!({
            "monitorName": "anchor-monitor",
            "scopeArn": scope_arn.clone(),
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        monitor.status, 200,
        "the anchor monitor must be created, got {} with body {}",
        monitor.status, monitor.body
    );

    let rows = [
        (
            "a well-formed scope ARN for an id the store never minted",
            format!("arn:aws:{SIGNING_NAME}:us-east-1:123456789012:scope/does-not-exist"),
        ),
        (
            "this service's own scope ARN form in another Region",
            format!("arn:aws:{SIGNING_NAME}:eu-west-1:{ACCOUNT_ID}:scope/00000000-0000-4000-8000-000000000001"),
        ),
        (
            "a monitor ARN: a live ARN of this service that is not a scope",
            monitor.text("monitorArn"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT_ID}:vpc/vpc-01234567"),
        ),
    ];

    for (index, (case, arn)) in rows.iter().enumerate() {
        let answer = emulator
            .create_monitor(json!({
                "monitorName": format!("rejected-{index}"),
                "scopeArn": arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException")
                || answer.text("__type").contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} / body {}",
            answer.error_type,
            answer.body
        );
        // The refusal left nothing behind.
        let read = emulator.get_monitor(&format!("rejected-{index}")).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must store nothing, got {} with body {}",
            read.status, read.body
        );
    }

    // The same request with the stored scope's ARN succeeds, so the refusals above
    // are about the ARN and nothing else.
    let accepted = emulator
        .create_monitor(json!({
            "monitorName": "accepted",
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "the stored scope's ARN must be accepted, got {} with body {}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn one_monitor_name_is_one_monitor() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;

    for (case, name, token) in [
        ("without a clientToken", "dup-plain", None),
        ("with a clientToken", "dup-token", Some(client_token(21))),
    ] {
        let mut request = HashMap::from([
            ("monitorName".to_string(), json!(name)),
            ("scopeArn".to_string(), json!(scope_arn)),
            (
                "localResources".to_string(),
                json!([{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]),
            ),
        ]);
        if let Some(token) = token.clone() {
            request.insert("clientToken".to_string(), json!(token));
        }
        let first = emulator.create_monitor(json!(request)).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create must succeed, got {} with body {}",
            first.status, first.body
        );

        let second = emulator.create_monitor(json!(request)).await;
        if token.is_some() {
            // The same token and the same parameters: a replay of the monitor that
            // token minted, not a second monitor and not a conflict.
            assert_eq!(
                second.status, 200,
                "case {case:?}: expected the idempotent replay to succeed, got {} with body {}",
                second.status, second.body
            );
            assert_eq!(
                second.text("monitorArn"),
                first.text("monitorArn"),
                "case {case:?}: expected the first monitor's ARN, got {:?}",
                second.text("monitorArn")
            );
            assert_eq!(
                second.number("createdAt"),
                first.number("createdAt"),
                "case {case:?}: a replay reports the stored creation time, got {} then {}",
                first.number("createdAt"),
                second.number("createdAt")
            );
        } else {
            assert_eq!(
                second.status, 409,
                "case {case:?}: expected 409 for a second create of one name, got {} with body {}",
                second.status, second.body
            );
            assert!(
                second.error_type.contains("ConflictException")
                    || second.text("__type").contains("ConflictException"),
                "case {case:?}: expected ConflictException, got {:?} / body {}",
                second.error_type,
                second.body
            );
        }

        // Either way the store still holds exactly the monitor the first call made.
        let read = emulator.get_monitor(name).await;
        assert_eq!(
            read.text("monitorArn"),
            first.text("monitorArn"),
            "case {case:?}: expected the stored monitor to be the first one, got {:?}",
            read.text("monitorArn")
        );
    }

    // A name differing only in case is a different name: `ResourceName` is
    // case-sensitive, so this is a second monitor rather than a conflict.
    let upper = emulator
        .create_monitor(json!({
            "monitorName": "DUP-PLAIN",
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        upper.status, 200,
        "a name differing in case is another monitor, got {} with body {}",
        upper.status, upper.body
    );
}

/// Every monitor `ListMonitors` reports, walked page by page with `max_results`:
/// the names in page order, and how many pages it took.
async fn page_through(emulator: &Emulator, filter: Option<&str>, max_results: i32) -> (Vec<String>, usize) {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut query = format!("?maxResults={max_results}");
        if let Some(status) = filter {
            query.push_str(&format!("&monitorStatus={status}"));
        }
        if let Some(cursor) = &token {
            query.push_str(&format!("&nextToken={cursor}"));
        }
        let answer = emulator.list_monitors(&query).await;
        assert_eq!(
            answer.status, 200,
            "maxResults {max_results}, filter {filter:?}, page {pages}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let items = answer
            .member("monitors")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            items.len() <= max_results as usize,
            "maxResults {max_results}, page {pages}: a page must hold at most {max_results} items, got {}",
            items.len()
        );
        for item in items {
            names.push(
                item.get("monitorName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        pages += 1;
        token = answer
            .member("nextToken")
            .as_str()
            .map(ToString::to_string);
        if token.is_none() {
            return (names, pages);
        }
        assert!(
            pages < 100,
            "maxResults {max_results}, filter {filter:?}: paging did not terminate after {pages} pages"
        );
    }
}

#[tokio::test]
async fn paging_through_list_monitors_yields_each_monitor_exactly_once() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    let expected: Vec<String> = (1..=7).map(|index| format!("mon-{index:02}")).collect();
    for name in &expected {
        let created = emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": format!("vpc-{name}") }]
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the monitor must be created, got {} with body {}",
            created.status, created.body
        );
    }

    // The page size dimension, including both published bounds of `maxResults`
    // (1..25) and a size that divides the store exactly.
    for max_results in [1, 2, 7, 25] {
        let (names, pages) = page_through(&emulator, None, max_results).await;
        assert_eq!(
            names, expected,
            "maxResults {max_results}: expected every monitor exactly once in name order, got {names:?}"
        );
        let want_pages = expected.len().div_ceil(max_results as usize);
        assert_eq!(
            pages, want_pages,
            "maxResults {max_results}: expected {want_pages} pages for {} monitors, got {pages}",
            expected.len()
        );
    }

    // An absent `maxResults` is a bounded page too, and still lists everything.
    let answer = emulator.list_monitors("").await;
    let listed: Vec<String> = answer
        .member("monitors")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item.get("monitorName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        listed, expected,
        "with no maxResults every monitor is listed once, got {listed:?}"
    );
    assert!(
        answer.member("nextToken").is_null(),
        "the last page carries no nextToken, got {:?}",
        answer.member("nextToken")
    );

    // Every summary is the record's own ARN, name and status. Read after the
    // provisioning span, so both the summary and the get report the monitor's
    // terminal status and the comparison cannot straddle the transition.
    tokio::time::sleep(PAST_PROVISIONING).await;
    let settled = emulator.list_monitors("").await;
    for item in settled
        .member("monitors")
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let name = item
            .get("monitorName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let read = emulator.get_monitor(&name).await;
        assert_eq!(
            item.get("monitorArn").and_then(Value::as_str),
            Some(read.text("monitorArn").as_str()),
            "case {name:?}: the summary's ARN is the stored one, got {item}"
        );
        assert_eq!(
            item.get("monitorStatus").and_then(Value::as_str),
            Some(read.text("monitorStatus").as_str()),
            "case {name:?}: the summary's status is the stored one, got {item}"
        );
    }
}

#[tokio::test]
async fn the_monitor_status_filter_lists_only_monitors_in_that_status() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    let expected: Vec<String> = (1..=3).map(|index| format!("filtered-{index}")).collect();
    for name in &expected {
        emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
    }

    // Fresh monitors are PENDING, so exactly the PENDING filter lists them and
    // every other modelled status lists none.
    for status in MONITOR_STATUS_VALUES {
        let (names, _) = page_through(&emulator, Some(status), 2).await;
        let want: Vec<String> = if *status == "PENDING" {
            expected.clone()
        } else {
            Vec::new()
        };
        assert_eq!(
            names, want,
            "case {status:?} before the provisioning span: expected {want:?}, got {names:?}"
        );
    }

    tokio::time::sleep(PAST_PROVISIONING).await;

    // Once the clock owes them ACTIVE, the same filters answer the other way.
    for status in MONITOR_STATUS_VALUES {
        let (names, _) = page_through(&emulator, Some(status), 2).await;
        let want: Vec<String> = if *status == "ACTIVE" {
            expected.clone()
        } else {
            Vec::new()
        };
        assert_eq!(
            names, want,
            "case {status:?} after the provisioning span: expected {want:?}, got {names:?}"
        );
    }
}

#[tokio::test]
async fn list_monitors_refuses_a_next_token_it_never_issued() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    for name in ["only-monitor", "second-monitor"] {
        emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
    }

    // A token this service issued resumes the walk; every other shape is refused —
    // including one that decodes cleanly but carries no cursor this listing minted,
    // which must not resume from an arbitrary lower bound.
    let issued = emulator.list_monitors("?maxResults=1").await;
    let good = issued.text("nextToken");
    assert!(
        !good.is_empty(),
        "the first page of two monitors must carry a nextToken, got body {}",
        issued.body
    );
    let resumed = emulator
        .list_monitors(&format!("?maxResults=1&nextToken={good}"))
        .await;
    assert_eq!(
        resumed.status, 200,
        "the issued token must be accepted, got {} with body {}",
        resumed.status, resumed.body
    );

    let rows = [
        ("not hex at all", "not-a-token".to_string()),
        ("two non-hex characters", "zz".to_string()),
        ("an odd-length hex string", "abc".to_string()),
        ("a single hex digit", "0".to_string()),
        ("percent-escaped braces", "%7B%7D".to_string()),
        // Hex that decodes to text, but to no cursor this listing minted.
        ("hex of a bare monitor name", hex::encode("second-monitor")),
        ("hex of arbitrary text", hex::encode("zzzz")),
        ("hex of the tag with no key", hex::encode("monitors/1:")),
        (
            "hex of another listing's tag",
            hex::encode("scopes/1:some-id"),
        ),
    ];

    for (case, token) in rows {
        let answer = emulator.list_monitors(&format!("?nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({token:?}): expected 400, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException")
                || answer.text("__type").contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} / body {}",
            answer.error_type,
            answer.body
        );
    }
}

#[tokio::test]
async fn a_deleted_monitor_is_gone_for_the_next_reader() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    let names: Vec<String> = (1..=4).map(|index| format!("doomed-{index}")).collect();
    for name in &names {
        emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
    }

    let mut remaining: Vec<String> = names.clone();
    for name in &names {
        let deleted = emulator.delete_monitor(name).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: expected 200 from the delete, got {} with body {}",
            deleted.status, deleted.body
        );

        let read = emulator.get_monitor(name).await;
        assert_eq!(
            read.status, 404,
            "case {name:?}: a deleted monitor is gone for the next reader, got {} with body {}",
            read.status, read.body
        );

        remaining.retain(|held| held != name);
        let (listed, _) = page_through(&emulator, None, 2).await;
        assert_eq!(
            listed, remaining,
            "case {name:?}: expected {remaining:?} still listed, got {listed:?}"
        );

        // Deleting it again names nothing: the modelled 404.
        let twice = emulator.delete_monitor(name).await;
        assert_eq!(
            twice.status, 404,
            "case {name:?}: a second delete names nothing, got {} with body {}",
            twice.status, twice.body
        );
    }

    // The name is free again after the delete, and the monitor that takes it is a
    // new record rather than the deleted one.
    let recreated = emulator
        .create_monitor(json!({
            "monitorName": names[0],
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::Region", "identifier": "us-west-2" }]
        }))
        .await;
    assert_eq!(
        recreated.status, 200,
        "a deleted name can be created again, got {} with body {}",
        recreated.status, recreated.body
    );
    assert_eq!(
        recreated.member("localResources"),
        &json!([{ "type": "AWS::Region", "identifier": "us-west-2" }]),
        "the recreated monitor holds the new request's resources, got {:?}",
        recreated.member("localResources")
    );
}

#[tokio::test]
async fn every_local_resource_type_the_model_declares_round_trips() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;

    // The catalog's source: `MonitorLocalResourceType` in the Smithy model, as
    // `MONITOR_LOCAL_RESOURCE_TYPE_VALUES` lists it.
    for (index, resource_type) in LOCAL_RESOURCE_TYPES.iter().enumerate() {
        let name = format!("typed-{index}");
        let local_resources = json!([{ "type": resource_type, "identifier": format!("resource-{index}") }]);
        let created = emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": local_resources
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {resource_type:?}: expected 200, got {} with body {}",
            created.status, created.body
        );
        let read = emulator.get_monitor(&name).await;
        assert_eq!(
            read.member("localResources"),
            &local_resources,
            "case {resource_type:?}: expected the stored localResources, got {:?}",
            read.member("localResources")
        );
    }
}

/// The store's own ceilings (`state::MAX_SCOPES` / `state::MAX_MONITORS`), so the
/// test asserts the bound the code states rather than a number of its own.
const MAX_SCOPES: usize = 100;
const MAX_MONITORS: usize = 100;

#[tokio::test]
async fn a_create_past_the_store_s_ceiling_is_refused() {
    let emulator = Emulator::new();

    // Fill the scope store exactly to its ceiling: every create up to it succeeds.
    let mut scope_arn = String::new();
    for index in 0..MAX_SCOPES {
        let created = emulator
            .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
            .await;
        assert_eq!(
            created.status, 200,
            "scope {index}: every create up to the ceiling succeeds, got {} with body {}",
            created.status, created.body
        );
        scope_arn = created.text("scopeArn");
    }

    // One more is the modelled quota refusal (HTTP 402), not a success and not a
    // 500: the store never grows past its bound.
    let refused = emulator
        .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
        .await;
    assert_eq!(
        refused.status, 402,
        "scope {MAX_SCOPES} is past the ceiling: expected 402, got {} with body {}",
        refused.status, refused.body
    );
    assert!(
        refused
            .error_type
            .contains("ServiceQuotaExceededException")
            || refused
                .text("__type")
                .contains("ServiceQuotaExceededException"),
        "expected ServiceQuotaExceededException, got {:?} / body {}",
        refused.error_type,
        refused.body
    );

    // The same rule holds for monitors, under a scope that does exist.
    for index in 0..MAX_MONITORS {
        let created = emulator
            .create_monitor(json!({
                "monitorName": format!("bounded-{index:03}"),
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "monitor {index}: every create up to the ceiling succeeds, got {} with body {}",
            created.status, created.body
        );
    }
    let refused = emulator
        .create_monitor(json!({
            "monitorName": "one-too-many",
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        refused.status, 402,
        "monitor {MAX_MONITORS} is past the ceiling: expected 402, got {} with body {}",
        refused.status, refused.body
    );
    // The refusal stored nothing, and deleting a monitor frees a slot again.
    let read = emulator.get_monitor("one-too-many").await;
    assert_eq!(
        read.status, 404,
        "a refused create must store nothing, got {} with body {}",
        read.status, read.body
    );
    let deleted = emulator.delete_monitor("bounded-000").await;
    assert_eq!(
        deleted.status, 200,
        "the delete that frees a slot must succeed, got {} with body {}",
        deleted.status, deleted.body
    );
    let accepted = emulator
        .create_monitor(json!({
            "monitorName": "one-too-many",
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "a freed slot takes a new monitor, got {} with body {}",
        accepted.status, accepted.body
    );
}

/// `DeleteScope` removes the scope the id names from the store, so every later
/// reader of that id is answered the modelled `ResourceNotFoundException` and no
/// other scope is touched.
///
/// The table is the create table (`scope_rows`) — scopes differing in target
/// count, tags and `clientToken` — plus the status dimension: the first row is
/// deleted while the clock still has it `IN_PROGRESS`, the rest once the clock owes
/// them `SUCCEEDED`.
#[tokio::test]
async fn a_deleted_scope_is_gone_for_every_later_reader() {
    let emulator = Emulator::new();
    let rows = scope_rows();
    let mut live: Vec<(String, String)> = Vec::new();
    for row in &rows {
        let created = emulator.create_scope(row.body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the scope must be created, got {} with body {}",
            row.case, created.status, created.body
        );
        live.push((created.text("scopeId"), row.case.to_string()));
    }

    for (index, (scope_id, case)) in live.clone().into_iter().enumerate() {
        // Row 0 is deleted inside the provisioning span, so a scope is deletable in
        // the transient status too; from row 1 on the clock has settled every
        // remaining scope into its terminal status.
        if index == 1 {
            tokio::time::sleep(PAST_PROVISIONING).await;
        }
        let before = emulator.get_scope(&scope_id).await;
        let expected_status = if index == 0 {
            "IN_PROGRESS"
        } else {
            "SUCCEEDED"
        };
        assert_eq!(
            before.text("status"),
            expected_status,
            "case {case:?}: expected the scope to be {expected_status} before the delete, got {:?}",
            before.text("status")
        );

        let deleted = emulator.delete_scope(&scope_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 from the delete, got {} with body {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted
                .body
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "case {case:?}: DeleteScopeOutput models no members, got body {}",
            deleted.body
        );

        let read = emulator.get_scope(&scope_id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a deleted scope is gone for the next reader, got {} with body {}",
            read.status, read.body
        );
        assert!(
            read.error_type.contains("ResourceNotFoundException")
                || read.text("__type").contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?} / body {}",
            read.error_type,
            read.body
        );

        // Deleting it again names nothing: the same modelled 404.
        let twice = emulator.delete_scope(&scope_id).await;
        assert_eq!(
            twice.status, 404,
            "case {case:?}: a second delete names nothing, got {} with body {}",
            twice.status, twice.body
        );

        // Only the named scope went: every scope still live reads back as itself.
        live.retain(|(held, _)| held != &scope_id);
        for (held, held_case) in &live {
            let still = emulator.get_scope(held).await;
            assert_eq!(
                still.status, 200,
                "case {case:?}: {held_case:?} must survive the delete, got {} with body {}",
                still.status, still.body
            );
            assert_eq!(
                still.text("scopeId"),
                *held,
                "case {case:?}: {held_case:?} must read back as itself, got {:?}",
                still.text("scopeId")
            );
        }
    }
}

/// Deleting an id the store does not hold is the modelled 404, whatever form the id
/// takes, and the delete leaves the store alone.
#[tokio::test]
async fn delete_scope_on_an_id_the_store_does_not_hold_is_404() {
    let emulator = Emulator::new();
    // One real scope exists, so a 404 is about the id and not about an empty store.
    let created = emulator
        .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
        .await;
    assert_eq!(
        created.status, 200,
        "the anchor scope must be created, got {} with body {}",
        created.status, created.body
    );
    let live_id = created.text("scopeId");

    // A scope this store did mint and that is already gone, so the table covers
    // both "never existed" and "deleted earlier".
    let deleted_once = emulator
        .create_scope(json!({ "targets": targets(&[("210987654321", "eu-west-1")]) }))
        .await;
    let deleted_id = deleted_once.text("scopeId");
    assert_eq!(
        emulator.delete_scope(&deleted_id).await.status,
        200,
        "the scope deleted up front must delete cleanly"
    );

    let rows = [
        (
            "a UUID-shaped id this store never minted",
            "11111111-2222-4333-8444-555555555555".to_string(),
        ),
        (
            "the id of a scope deleted earlier in this test",
            deleted_id.clone(),
        ),
        ("an id that is not UUID-shaped", "not-a-scope".to_string()),
        (
            "a monitor-style name in the scope id position",
            "scope_1.live-2".to_string(),
        ),
    ];

    for (case, scope_id) in &rows {
        let answer = emulator.delete_scope(scope_id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ResourceNotFoundException")
                || answer.text("__type").contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?} / body {}",
            answer.error_type,
            answer.body
        );
        let still = emulator.get_scope(&live_id).await;
        assert_eq!(
            still.status, 200,
            "case {case:?}: the live scope must be untouched, got {} with body {}",
            still.status, still.body
        );
    }
}

/// What a deleted scope's records are worth afterwards: its ARN resolves to no
/// scope (so `CreateMonitor` refuses it, exactly as it refuses an ARN that never
/// named one), and its `clientToken` mints a NEW scope rather than answering with
/// the deleted one.
#[tokio::test]
async fn deleting_a_scope_retires_its_arn_and_frees_its_client_token() {
    let emulator = Emulator::new();

    for (index, pairs) in [
        vec![("123456789012", "us-east-1")],
        vec![("111111111111", "eu-west-1"), ("222222222222", "us-east-2")],
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("{} target(s)", pairs.len());
        let token = client_token(900 + index as u32);
        let body = json!({ "targets": targets(&pairs), "clientToken": token });

        let created = emulator.create_scope(body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the scope must be created, got {} with body {}",
            created.status, created.body
        );
        let scope_id = created.text("scopeId");
        let scope_arn = created.text("scopeArn");

        // A monitor under the scope, so the ARN is live before the delete.
        let monitor_name = format!("orphan-{index}");
        let monitor = emulator
            .create_monitor(json!({
                "monitorName": monitor_name,
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
        assert_eq!(
            monitor.status, 200,
            "case {case:?}: the monitor under the live scope must be created, got {} with body {}",
            monitor.status, monitor.body
        );

        assert_eq!(
            emulator.delete_scope(&scope_id).await.status,
            200,
            "case {case:?}: the delete must succeed"
        );

        // The ARN now names no scope, so a create that needs a scope is refused
        // just as it is for an ARN this store never minted.
        let refused = emulator
            .create_monitor(json!({
                "monitorName": format!("after-{index}"),
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a retired scopeArn is refused, got {} with body {}",
            refused.status, refused.body
        );

        // The same `clientToken` is no longer a replay of the deleted scope: it
        // mints a new one, with a new id.
        let recreated = emulator.create_scope(body.clone()).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the token must create a new scope, got {} with body {}",
            recreated.status, recreated.body
        );
        assert_ne!(
            recreated.text("scopeId"),
            scope_id,
            "case {case:?}: the new scope must not be the deleted one, got {:?}",
            recreated.text("scopeId")
        );
        let read = emulator.get_scope(&recreated.text("scopeId")).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the new scope must be readable, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.member("targets"),
            &body["targets"],
            "case {case:?}: the new scope holds the request's targets, got {:?}",
            read.member("targets")
        );
    }
}
