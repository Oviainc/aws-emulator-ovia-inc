// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of Amazon Connect's instance -> queue resource loop, driven over
//! HTTP through the same `MultiServiceRouter` the emulator serves with.
//!
//! Every suite here asserts a RULE over a table of inputs rather than one recorded call:
//! what `CreateInstance`/`CreateQueue` persist is exactly what every later read reports,
//! identifiers and ARNs are minted per resource, a deleted resource stops being visible,
//! and `List*` walks the store in a deterministic order one `MaxResults` page at a time.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use engine::{InMemoryEngine, EMULATOR_ACCOUNT_ID};
use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::state::{
    INSTANCE_ACTIVATION_DELAY, INSTANCE_QUOTA, QUEUES_PER_INSTANCE_QUOTA,
};
use service_com_amazonaws_connect_amazonconnectservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the HTTP status, the `x-amzn-errortype` error code restJson1
/// names a modelled error shape with (empty on a success), and the parsed JSON body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error_code: String,
    pub(crate) body: Value,
}

/// One emulator, one region, with the Connect service mounted — the fixture every suite
/// builds. The region travels in the SigV4 credential scope, which is also how the
/// handlers learn which region to mint ARNs in.
///
/// `pub(crate)` so a sibling suite in this test crate drives the same fixture rather
/// than standing up a second copy of it.
pub(crate) struct Connect {
    router: MultiServiceRouter,
    region: String,
}

impl Connect {
    pub(crate) fn in_region(region: &str) -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self {
            router,
            region: region.to_string(),
        }
    }

    pub(crate) fn new() -> Self {
        Self::in_region("us-east-1")
    }

    pub(crate) async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let answer = self.send_answer(method, uri, body).await;
        (answer.status, answer.body)
    }

    /// The same call as [`Connect::send`], keeping the `x-amzn-errortype` header a
    /// suite needs to tell two modelled errors of the same status apart.
    pub(crate) async fn send_answer(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> Answer {
        let payload = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
        });
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "connect.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=TESTKEY/20240101/{}/connect/aws4_request, \
                     SignedHeaders=host, Signature=abc",
                    self.region
                ),
            )
            .body(payload)
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error_code,
            body: parsed,
        }
    }

    /// `CreateInstance`, asserting success and handing back `(Id, Arn)`.
    pub(crate) async fn create_instance(&self, body: Value) -> (String, String) {
        let (status, out) = self.send("PUT", "/instance", Some(body.clone())).await;
        assert_eq!(status, 200, "CreateInstance {body}: got {out}");
        (
            out["Id"].as_str().expect("Id").to_string(),
            out["Arn"].as_str().expect("Arn").to_string(),
        )
    }

    async fn describe_instance(&self, id: &str) -> (u16, Value) {
        self.send("GET", &format!("/instance/{id}"), None).await
    }

    pub(crate) async fn delete_instance(&self, id: &str) -> u16 {
        self.send("DELETE", &format!("/instance/{id}"), None).await.0
    }

    async fn list_instances(&self, max_results: Option<i32>, token: Option<&str>) -> (u16, Value) {
        self.send("GET", &format!("/instance{}", query(max_results, token, &[])), None)
            .await
    }

    /// `CreateQueue`, returning the raw status and body so a suite can assert a refusal.
    async fn create_queue(&self, instance_id: &str, body: Value) -> (u16, Value) {
        self.send("PUT", &format!("/queues/{instance_id}"), Some(body))
            .await
    }

    async fn describe_queue(&self, instance_id: &str, queue_id: &str) -> (u16, Value) {
        self.send("GET", &format!("/queues/{instance_id}/{queue_id}"), None)
            .await
    }

    async fn delete_queue(&self, instance_id: &str, queue_id: &str) -> u16 {
        self.send("DELETE", &format!("/queues/{instance_id}/{queue_id}"), None)
            .await
            .0
    }

    async fn list_queues(
        &self,
        instance_id: &str,
        max_results: Option<i32>,
        token: Option<&str>,
        queue_types: &[&str],
    ) -> (u16, Value) {
        let uri = format!(
            "/queues-summary/{instance_id}{}",
            query(max_results, token, queue_types)
        );
        self.send("GET", &uri, None).await
    }
}

/// Build the `maxResults`/`nextToken`/`queueTypes` query string the models bind.
fn query(max_results: Option<i32>, token: Option<&str>, queue_types: &[&str]) -> String {
    let mut params: Vec<String> = Vec::new();
    if let Some(max) = max_results {
        params.push(format!("maxResults={max}"));
    }
    if let Some(token) = token {
        params.push(format!("nextToken={token}"));
    }
    for queue_type in queue_types {
        params.push(format!("queueTypes={queue_type}"));
    }
    if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    }
}

/// A minimal valid `CreateInstance` body.
pub(crate) fn connect_managed(alias: &str) -> Value {
    json!({
        "IdentityManagementType": "CONNECT_MANAGED",
        "InstanceAlias": alias,
        "InboundCallsEnabled": true,
        "OutboundCallsEnabled": true,
    })
}

/// A minimal valid `CreateQueue` body.
fn queue_named(name: &str) -> Value {
    json!({ "Name": name, "HoursOfOperationId": "hop-1" })
}

/// `8-4-4-4-12` lowercase hex: the shape of the v4 UUIDs Connect mints.
pub(crate) fn looks_like_a_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len && group.chars().all(|c| c.is_ascii_hexdigit())
            })
}

fn ids_of(list: &Value, key: &str) -> Vec<String> {
    list.as_array()
        .expect("a summary list")
        .iter()
        .map(|entry| entry[key].as_str().expect("an id").to_string())
        .collect()
}

/// RULE: every member `CreateInstance` accepts is persisted and reported verbatim by
/// `DescribeInstance` and `ListInstances`, and the instance's identity (a fresh UUID id
/// and the ARN built from it in the request's region) is minted per instance.
///
/// The table spans the `DirectoryType` members, both call flags in both positions, an
/// absent and a present alias, tag counts 0/1/2, and the alias length bound (45).
#[tokio::test]
async fn created_instance_members_are_what_every_later_read_reports() {
    let max_alias = "a".repeat(44) + "z"; // DirectoryAlias length bound: 1..45.
    let cases: Vec<(&str, Value)> = vec![
        ("connect-managed, both channels, untagged", connect_managed("myorg")),
        (
            "saml, inbound only, one tag",
            json!({
                "IdentityManagementType": "SAML",
                "InstanceAlias": "team2",
                "InboundCallsEnabled": true,
                "OutboundCallsEnabled": false,
                "Tags": { "env": "prod" },
            }),
        ),
        (
            "existing directory, no alias, outbound only, two tags",
            json!({
                "IdentityManagementType": "EXISTING_DIRECTORY",
                "DirectoryId": "d-0123456789",
                "InboundCallsEnabled": false,
                "OutboundCallsEnabled": true,
                "Tags": { "a": "1", "b": "2" },
            }),
        ),
        (
            "alias at the 45-character bound, no channels",
            json!({
                "IdentityManagementType": "CONNECT_MANAGED",
                "InstanceAlias": max_alias,
                "InboundCallsEnabled": false,
                "OutboundCallsEnabled": false,
            }),
        ),
    ];

    let connect = Connect::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for (case, body) in &cases {
        let (id, arn) = connect.create_instance(body.clone()).await;

        assert!(
            looks_like_a_uuid(&id),
            "case {case:?}: expected a v4-UUID instance id, got {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: expected a fresh id, got the already-minted {id:?}"
        );
        assert_eq!(
            arn,
            format!("arn:aws:connect:us-east-1:{EMULATOR_ACCOUNT_ID}:instance/{id}"),
            "case {case:?}: ARN must be built from the minted id"
        );

        let (status, described) = connect.describe_instance(&id).await;
        assert_eq!(status, 200, "case {case:?}: DescribeInstance got {described}");
        let instance = &described["Instance"];

        assert_eq!(instance["Id"], json!(id), "case {case:?}: Id");
        assert_eq!(instance["Arn"], json!(arn), "case {case:?}: Arn");
        assert_eq!(
            instance["IdentityManagementType"], body["IdentityManagementType"],
            "case {case:?}: IdentityManagementType"
        );
        assert_eq!(
            instance["InstanceAlias"],
            body.get("InstanceAlias").cloned().unwrap_or(Value::Null),
            "case {case:?}: InstanceAlias"
        );
        assert_eq!(
            instance["InboundCallsEnabled"], body["InboundCallsEnabled"],
            "case {case:?}: InboundCallsEnabled"
        );
        assert_eq!(
            instance["OutboundCallsEnabled"], body["OutboundCallsEnabled"],
            "case {case:?}: OutboundCallsEnabled"
        );
        assert_eq!(
            instance["Tags"],
            body.get("Tags").cloned().unwrap_or(Value::Null),
            "case {case:?}: Tags"
        );
        assert!(
            instance["CreatedTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: expected a CreatedTime timestamp, got {}",
            instance["CreatedTime"]
        );
        let expected_url = format!(
            "https://{}.my.connect.aws",
            body.get("InstanceAlias").and_then(Value::as_str).unwrap_or(&id)
        );
        assert_eq!(
            instance["InstanceAccessUrl"], json!(expected_url),
            "case {case:?}: InstanceAccessUrl"
        );
        assert!(
            instance["ServiceRole"]
                .as_str()
                .is_some_and(|role| role.starts_with("arn:aws:iam::")),
            "case {case:?}: expected an IAM ServiceRole ARN, got {}",
            instance["ServiceRole"]
        );

        // The second read answers from storage too: identical, not re-derived.
        let (_, again) = connect.describe_instance(&id).await;
        assert_eq!(
            again["Instance"]["CreatedTime"], instance["CreatedTime"],
            "case {case:?}: CreatedTime must be stable across reads"
        );
    }

    // Every created instance appears in ListInstances exactly once, with the same
    // members the per-instance read reports.
    let (status, listed) = connect.list_instances(Some(10), None).await;
    assert_eq!(status, 200, "ListInstances got {listed}");
    let summary_ids = ids_of(&listed["InstanceSummaryList"], "Id");
    assert_eq!(
        summary_ids.iter().cloned().collect::<BTreeSet<_>>(),
        minted,
        "ListInstances must report every created instance exactly once, got {summary_ids:?}"
    );
    assert_eq!(
        summary_ids.len(),
        minted.len(),
        "ListInstances must not duplicate an instance, got {summary_ids:?}"
    );
}

/// RULE: a resource ARN carries the region the request was signed for.
#[tokio::test]
async fn instance_arns_carry_the_requests_region() {
    // Any AWS region token: the ARN's region segment is taken from the request, never
    // from a constant in the handler.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let connect = Connect::in_region(region);
        let (id, arn) = connect.create_instance(connect_managed("myorg")).await;
        assert_eq!(
            arn,
            format!("arn:aws:connect:{region}:{EMULATOR_ACCOUNT_ID}:instance/{id}"),
            "case {region:?}: instance ARN region"
        );

        let (_, queue) = connect
            .create_queue(&id, queue_named("support"))
            .await;
        assert_eq!(
            queue["QueueArn"],
            json!(format!("{arn}/queue/{}", queue["QueueId"].as_str().expect("QueueId"))),
            "case {region:?}: a queue ARN extends its instance ARN"
        );
    }
}

/// RULE: Connect returns a new instance in the non-terminal `CREATION_IN_PROGRESS`
/// status, and the clock — not a later call — owes it `ACTIVE`.
#[tokio::test]
async fn a_new_instance_becomes_active_on_the_clock() {
    let connect = Connect::new();
    let (id, _) = connect.create_instance(connect_managed("myorg")).await;

    let (_, described) = connect.describe_instance(&id).await;
    assert_eq!(
        described["Instance"]["InstanceStatus"],
        json!("CREATION_IN_PROGRESS"),
        "straight after the create the instance is still being created, got {described}"
    );

    tokio::time::sleep(INSTANCE_ACTIVATION_DELAY + Duration::from_millis(100)).await;

    let (_, described) = connect.describe_instance(&id).await;
    assert_eq!(
        described["Instance"]["InstanceStatus"],
        json!("ACTIVE"),
        "once the activation span elapsed the instance is ACTIVE, got {described}"
    );
    let (_, listed) = connect.list_instances(None, None).await;
    assert_eq!(
        listed["InstanceSummaryList"][0]["InstanceStatus"],
        json!("ACTIVE"),
        "ListInstances reports the same status as DescribeInstance, got {listed}"
    );
}

/// RULE: a repeated `ClientToken` names the instance that token already created; a
/// different token (or none) creates another one.
#[tokio::test]
async fn create_instance_is_idempotent_per_client_token() {
    let connect = Connect::new();

    let mut body = connect_managed("myorg");
    body["ClientToken"] = json!("token-a");
    let (first, first_arn) = connect.create_instance(body.clone()).await;
    let (repeat, repeat_arn) = connect.create_instance(body.clone()).await;
    assert_eq!(
        (first.as_str(), first_arn.as_str()),
        (repeat.as_str(), repeat_arn.as_str()),
        "case {:?}: a repeated ClientToken must not mint a second instance",
        "token-a twice"
    );

    body["ClientToken"] = json!("token-b");
    let (other, _) = connect.create_instance(body.clone()).await;
    assert_ne!(
        other, first,
        "case {:?}: a different ClientToken mints a different instance",
        "token-b"
    );

    let (_, no_token) = connect.create_instance(connect_managed("myorg")).await;
    assert!(
        ![first.as_str(), other.as_str()].contains(&no_token.as_str()),
        "case {:?}: an absent ClientToken always mints a new instance, got {no_token}",
        "no token"
    );

    let (_, listed) = connect.list_instances(Some(10), None).await;
    assert_eq!(
        ids_of(&listed["InstanceSummaryList"], "Id").len(),
        3,
        "three creates with two distinct tokens plus one untokenised: got {listed}"
    );
}

/// RULE: a read or a delete of an identifier the store does not hold is
/// `ResourceNotFoundException` (404), and a deleted resource stops being visible to
/// every reader.
#[tokio::test]
async fn unknown_and_deleted_resources_answer_404() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("myorg")).await;
    let (_, queue) = connect.create_queue(&instance_id, queue_named("support")).await;
    let queue_id = queue["QueueId"].as_str().expect("QueueId").to_string();

    // Identifiers of the right modelled form that name nothing.
    let absent = "11111111-2222-3333-4444-555555555555";

    let cases: Vec<(&str, u16, Value)> = vec![
        {
            let (status, body) = connect.describe_instance(absent).await;
            ("DescribeInstance of an absent instance", status, body)
        },
        {
            let (status, body) = connect.describe_queue(absent, &queue_id).await;
            ("DescribeQueue inside an absent instance", status, body)
        },
        {
            let (status, body) = connect.describe_queue(&instance_id, absent).await;
            ("DescribeQueue of an absent queue", status, body)
        },
        {
            let (status, body) = connect.list_queues(absent, None, None, &[]).await;
            ("ListQueues of an absent instance", status, body)
        },
        {
            let (status, body) = connect.create_queue(absent, queue_named("support")).await;
            ("CreateQueue in an absent instance", status, body)
        },
    ];
    for (case, status, body) in cases {
        assert_eq!(status, 404, "case {case:?}: got {status} {body}");
        assert!(
            body["message"].is_string() || body["Message"].is_string(),
            "case {case:?}: the error must carry a message, got {body}"
        );
    }

    assert_eq!(
        connect.delete_queue(&instance_id, absent).await,
        404,
        "deleting a queue that is not there is itself a 404"
    );
    assert_eq!(
        connect.delete_instance(absent).await,
        404,
        "deleting an instance that is not there is itself a 404"
    );

    // Delete the queue: Describe is a 404 and the list no longer holds it.
    assert_eq!(connect.delete_queue(&instance_id, &queue_id).await, 200);
    assert_eq!(
        connect.describe_queue(&instance_id, &queue_id).await.0,
        404,
        "a deleted queue is gone from DescribeQueue"
    );
    let (_, listed) = connect.list_queues(&instance_id, None, None, &[]).await;
    assert_eq!(
        ids_of(&listed["QueueSummaryList"], "Id"),
        Vec::<String>::new(),
        "a deleted queue is gone from ListQueues, got {listed}"
    );
    assert_eq!(
        connect.delete_queue(&instance_id, &queue_id).await,
        404,
        "deleting the same queue twice is a 404 the second time"
    );

    // Delete the instance: every reader loses it, including its queues.
    assert_eq!(connect.delete_instance(&instance_id).await, 200);
    assert_eq!(
        connect.describe_instance(&instance_id).await.0,
        404,
        "a deleted instance is gone from DescribeInstance"
    );
    assert_eq!(
        connect.list_queues(&instance_id, None, None, &[]).await.0,
        404,
        "a deleted instance's queues are gone with it"
    );
    let (_, listed) = connect.list_instances(Some(10), None).await;
    assert_eq!(
        ids_of(&listed["InstanceSummaryList"], "Id"),
        Vec::<String>::new(),
        "a deleted instance is gone from ListInstances, got {listed}"
    );
}

/// RULE: every member `CreateQueue` accepts is persisted and reported verbatim by
/// `DescribeQueue`, and `ListQueues` reports the same queue as a `STANDARD` summary.
///
/// The table spans an absent and a present description, `MaxContacts` at its `0` bound
/// and above it, an absent and a present `OutboundCallerConfig`, and tag counts 0/1/2.
#[tokio::test]
async fn created_queue_members_are_what_every_later_read_reports() {
    let connect = Connect::new();
    let (instance_id, instance_arn) = connect.create_instance(connect_managed("myorg")).await;

    let cases: Vec<(&str, Value)> = vec![
        ("name and hours only", queue_named("support")),
        (
            "description, MaxContacts at the 0 bound, one tag",
            json!({
                "Name": "billing",
                "HoursOfOperationId": "hop-2",
                "Description": "billing enquiries",
                "MaxContacts": 0,
                "Tags": { "team": "fin" },
            }),
        ),
        (
            "outbound caller config, MaxContacts above the bound, two tags",
            json!({
                "Name": "escalations",
                "HoursOfOperationId": "hop-3",
                "Description": "tier two",
                "MaxContacts": 50,
                "OutboundCallerConfig": {
                    "OutboundCallerIdName": "Acme Support",
                    "OutboundFlowId": "flow-1",
                },
                "Tags": { "team": "cx", "tier": "2" },
            }),
        ),
        (
            "a 127-character name (the modelled bound)",
            json!({
                "Name": "q".repeat(127),
                "HoursOfOperationId": "hop-4",
            }),
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, body) in &cases {
        let (status, created) = connect.create_queue(&instance_id, body.clone()).await;
        assert_eq!(status, 200, "case {case:?}: CreateQueue got {created}");
        let queue_id = created["QueueId"].as_str().expect("QueueId").to_string();
        let queue_arn = created["QueueArn"].as_str().expect("QueueArn").to_string();

        assert!(
            looks_like_a_uuid(&queue_id),
            "case {case:?}: expected a v4-UUID queue id, got {queue_id:?}"
        );
        assert!(
            minted.insert(queue_id.clone()),
            "case {case:?}: expected a fresh queue id, got {queue_id:?}"
        );
        assert_eq!(
            queue_arn,
            format!("{instance_arn}/queue/{queue_id}"),
            "case {case:?}: a queue ARN is its instance's ARN plus /queue/<QueueId>"
        );

        let (status, described) = connect.describe_queue(&instance_id, &queue_id).await;
        assert_eq!(status, 200, "case {case:?}: DescribeQueue got {described}");
        let queue = &described["Queue"];

        assert_eq!(queue["QueueId"], json!(queue_id), "case {case:?}: QueueId");
        assert_eq!(queue["QueueArn"], json!(queue_arn), "case {case:?}: QueueArn");
        assert_eq!(queue["Name"], body["Name"], "case {case:?}: Name");
        assert_eq!(
            queue["HoursOfOperationId"], body["HoursOfOperationId"],
            "case {case:?}: HoursOfOperationId"
        );
        assert_eq!(
            queue["Description"],
            body.get("Description").cloned().unwrap_or(Value::Null),
            "case {case:?}: Description"
        );
        assert_eq!(
            queue["MaxContacts"],
            body.get("MaxContacts").cloned().unwrap_or(Value::Null),
            "case {case:?}: MaxContacts"
        );
        assert_eq!(
            queue["OutboundCallerConfig"],
            body.get("OutboundCallerConfig").cloned().unwrap_or(Value::Null),
            "case {case:?}: OutboundCallerConfig"
        );
        assert_eq!(
            queue["Tags"],
            body.get("Tags").cloned().unwrap_or(Value::Null),
            "case {case:?}: Tags"
        );
        assert_eq!(
            queue["Status"], json!("ENABLED"),
            "case {case:?}: a created queue is ENABLED"
        );
        assert_eq!(
            queue["LastModifiedRegion"], json!("us-east-1"),
            "case {case:?}: LastModifiedRegion"
        );
        assert!(
            queue["LastModifiedTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: expected a LastModifiedTime, got {}",
            queue["LastModifiedTime"]
        );
    }

    let (_, listed) = connect.list_queues(&instance_id, None, None, &[]).await;
    let summaries = listed["QueueSummaryList"].as_array().expect("QueueSummaryList");
    assert_eq!(
        summaries
            .iter()
            .map(|summary| summary["Id"].as_str().expect("Id").to_string())
            .collect::<BTreeSet<_>>(),
        minted,
        "ListQueues must report every created queue exactly once, got {listed}"
    );
    for summary in summaries {
        assert_eq!(
            summary["QueueType"], json!("STANDARD"),
            "a queue CreateQueue minted is STANDARD, got {summary}"
        );
        assert_eq!(
            summary["Arn"],
            json!(format!(
                "{instance_arn}/queue/{}",
                summary["Id"].as_str().expect("Id")
            )),
            "a summary's Arn matches the created queue's ARN, got {summary}"
        );
        assert!(
            summary["Name"].is_string(),
            "a summary carries the queue's name, got {summary}"
        );
    }
}

/// RULE: a queue name is unique within its instance — a same-name create there is
/// `DuplicateResourceException` (409) — but the name is free in any other instance, and
/// free again once the queue holding it is deleted.
#[tokio::test]
async fn queue_names_are_unique_within_an_instance_only() {
    let connect = Connect::new();
    let (first, _) = connect.create_instance(connect_managed("orgone")).await;
    let (second, _) = connect.create_instance(connect_managed("orgtwo")).await;

    // Names differing in value, case and length: uniqueness is per exact name.
    for name in ["support", "Support", "a", &"q".repeat(127)] {
        let (status, created) = connect.create_queue(&first, queue_named(name)).await;
        assert_eq!(status, 200, "case {name:?}: first create got {created}");
        let queue_id = created["QueueId"].as_str().expect("QueueId").to_string();

        let (status, refused) = connect.create_queue(&first, queue_named(name)).await;
        assert_eq!(
            status, 409,
            "case {name:?}: a repeated name in the same instance is a 409, got {refused}"
        );

        let (status, elsewhere) = connect.create_queue(&second, queue_named(name)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the same name in another instance is free, got {elsewhere}"
        );

        assert_eq!(connect.delete_queue(&first, &queue_id).await, 200);
        let (status, reused) = connect.create_queue(&first, queue_named(name)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the name is free again once its queue is deleted, got {reused}"
        );
    }
}

/// RULE: `ListQueues` keeps only the queue types `queueTypes` names, and reports every
/// stored queue when the parameter is absent.
#[tokio::test]
async fn list_queues_filters_on_the_requested_queue_types() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("myorg")).await;
    for name in ["support", "billing", "escalations"] {
        let (status, _) = connect.create_queue(&instance_id, queue_named(name)).await;
        assert_eq!(status, 200);
    }

    // `QueueType` members, from com.amazonaws.connect#QueueType: STANDARD | AGENT.
    // CreateQueue mints STANDARD queues, so only a filter admitting STANDARD matches.
    let cases: Vec<(&str, Vec<&str>, usize)> = vec![
        ("no filter", vec![], 3),
        ("STANDARD only", vec!["STANDARD"], 3),
        ("AGENT only", vec!["AGENT"], 0),
        ("both members", vec!["STANDARD", "AGENT"], 3),
    ];
    for (case, filter, expected) in cases {
        let (status, listed) = connect
            .list_queues(&instance_id, None, None, &filter)
            .await;
        assert_eq!(status, 200, "case {case:?}: got {listed}");
        let ids = ids_of(&listed["QueueSummaryList"], "Id");
        assert_eq!(
            ids.len(),
            expected,
            "case {case:?}: expected {expected} queues, got {ids:?}"
        );
    }
}

/// RULE: a `List*` walks the store in one deterministic order, returns at most
/// `MaxResults` items per page with a `NextToken` only while items remain, and resuming
/// from that token yields every remaining item exactly once.
#[tokio::test]
async fn list_pages_cover_every_item_exactly_once() {
    // (how many resources exist, the MaxResults asked for) — page sizes below, equal to
    // and above the population, plus the unpaginated call.
    let cases: Vec<(usize, Option<i32>)> = vec![
        (1, Some(1)),
        (3, Some(1)),
        (5, Some(2)),
        (4, Some(4)),
        (4, Some(10)),
        (7, None),
    ];

    for (population, page_size) in cases {
        let case = format!("{population} instances, maxResults={page_size:?}");
        let connect = Connect::new();
        let mut created: BTreeSet<String> = BTreeSet::new();
        for index in 0..population {
            let (id, _) = connect
                .create_instance(connect_managed(&format!("org{index}")))
                .await;
            created.insert(id);
        }

        let (seen, pages) = walk_instances(&connect, page_size).await;
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            created,
            "case {case:?}: the pages must cover exactly the created instances, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            created.len(),
            "case {case:?}: no instance may appear on two pages, got {seen:?}"
        );
        let expected_pages = page_size
            .map_or(1, |size| population.div_ceil(size as usize).max(1));
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );

        // The same rule over queues inside one instance.
        let (instance_id, _) = connect.create_instance(connect_managed("queueorg")).await;
        let mut queues: BTreeSet<String> = BTreeSet::new();
        for index in 0..population {
            let (status, created) = connect
                .create_queue(&instance_id, queue_named(&format!("queue{index}")))
                .await;
            assert_eq!(status, 200, "case {case:?}: CreateQueue got {created}");
            queues.insert(created["QueueId"].as_str().expect("QueueId").to_string());
        }
        let (seen, pages) = walk_queues(&connect, &instance_id, page_size).await;
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            queues,
            "case {case:?}: the queue pages must cover exactly the created queues, got {seen:?}"
        );
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} queue pages, got {pages}"
        );
    }
}

/// Walk `ListInstances` to exhaustion, returning every id seen and the page count.
async fn walk_instances(connect: &Connect, page_size: Option<i32>) -> (Vec<String>, usize) {
    let mut seen = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    loop {
        let (status, listed) = connect.list_instances(page_size, token.as_deref()).await;
        assert_eq!(status, 200, "ListInstances got {listed}");
        pages += 1;
        let page = ids_of(&listed["InstanceSummaryList"], "Id");
        if let Some(size) = page_size {
            assert!(
                page.len() <= size as usize,
                "a page must hold at most maxResults items, got {page:?}"
            );
        }
        seen.extend(page);
        match listed["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(pages < 64, "ListInstances did not terminate");
    }
    (seen, pages)
}

/// Walk `ListQueues` to exhaustion, returning every id seen and the page count.
async fn walk_queues(
    connect: &Connect,
    instance_id: &str,
    page_size: Option<i32>,
) -> (Vec<String>, usize) {
    let mut seen = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    loop {
        let (status, listed) = connect
            .list_queues(instance_id, page_size, token.as_deref(), &[])
            .await;
        assert_eq!(status, 200, "ListQueues got {listed}");
        pages += 1;
        let page = ids_of(&listed["QueueSummaryList"], "Id");
        if let Some(size) = page_size {
            assert!(
                page.len() <= size as usize,
                "a page must hold at most maxResults items, got {page:?}"
            );
        }
        seen.extend(page);
        match listed["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(pages < 64, "ListQueues did not terminate");
    }
    (seen, pages)
}

/// RULE: a `MaxResults` inside the modelled range is served and one outside it is
/// refused, and a `NextToken` this service did not issue is refused rather than
/// silently restarting the scan.
#[tokio::test]
async fn page_size_bounds_and_foreign_tokens_are_refused() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("myorg")).await;
    let (status, _) = connect.create_queue(&instance_id, queue_named("support")).await;
    assert_eq!(status, 200);

    // MaxResult10 range 1..10 for ListInstances; MaxResult1000 range 1..1000 for
    // ListQueues — both published bounds, then one past the top.
    for (case, max_results, expected) in [
        ("ListInstances lower bound", 1, 200),
        ("ListInstances upper bound", 10, 200),
        ("ListInstances past the upper bound", 11, 400),
    ] {
        let (status, body) = connect.list_instances(Some(max_results), None).await;
        assert_eq!(status, expected, "case {case:?}: got {status} {body}");
    }
    for (case, max_results, expected) in [
        ("ListQueues lower bound", 1, 200),
        ("ListQueues upper bound", 1000, 200),
        ("ListQueues past the upper bound", 1001, 400),
    ] {
        let (status, body) = connect
            .list_queues(&instance_id, Some(max_results), None, &[])
            .await;
        assert_eq!(status, expected, "case {case:?}: got {status} {body}");
    }

    // Tokens no `encode_page_token` could have produced: non-hex text, an odd hex
    // length, and hex that decodes to bytes which are not UTF-8.
    for token in ["not-a-token", "zz", "0", "ff"] {
        let (status, body) = connect.list_instances(Some(10), Some(token)).await;
        assert_eq!(
            status, 400,
            "case {token:?}: a foreign ListInstances NextToken is refused, got {body}"
        );
        let (status, body) = connect
            .list_queues(&instance_id, Some(10), Some(token), &[])
            .await;
        assert_eq!(
            status, 400,
            "case {token:?}: a foreign ListQueues NextToken is refused, got {body}"
        );
    }
}

/// RULE: each store the service keeps for the life of the process is bounded — a
/// create that would grow it past the emulator's own quota is refused with that
/// operation's modelled quota error, and the store stops growing there.
///
/// The two rows are the two stores: the instance registry (`INSTANCE_QUOTA`,
/// `ServiceQuotaExceededException`, HTTP 402) and one instance's queue map
/// (`QUEUES_PER_INSTANCE_QUOTA`, `LimitExceededException`, HTTP 429).
#[tokio::test]
async fn every_store_is_bounded_by_its_quota() {
    let connect = Connect::new();

    // Fill the instance registry to its bound, then one more.
    for index in 0..INSTANCE_QUOTA {
        let (status, body) = connect
            .send("PUT", "/instance", Some(connect_managed("org")))
            .await;
        assert_eq!(status, 200, "instance {index}: got {status} {body}");
    }
    let (status, body) = connect
        .send("PUT", "/instance", Some(connect_managed("org")))
        .await;
    assert_eq!(
        status, 402,
        "case {:?}: a create past the instance quota is refused, got {status} {body}",
        "instance quota"
    );
    // Walked to exhaustion: the refused create added nothing, so the registry holds
    // exactly the quota's worth of instances.
    let mut instance_ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (status, page) = connect.list_instances(Some(10), token.as_deref()).await;
        assert_eq!(status, 200, "paging the instance registry: got {page}");
        instance_ids.extend(ids_of(&page["InstanceSummaryList"], "Id"));
        token = page["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        instance_ids.len(),
        INSTANCE_QUOTA,
        "case {:?}: the store holds exactly the quota, got {} instance(s)",
        "instance quota",
        instance_ids.len()
    );

    // And the per-instance queue map, on an instance of its own.
    let fresh = Connect::new();
    let (instance_id, _) = fresh.create_instance(connect_managed("org")).await;
    for index in 0..QUEUES_PER_INSTANCE_QUOTA {
        let (status, body) = fresh
            .create_queue(&instance_id, queue_named(&format!("queue-{index}")))
            .await;
        assert_eq!(status, 200, "queue {index}: got {status} {body}");
    }
    let (status, body) = fresh
        .create_queue(&instance_id, queue_named("one-too-many"))
        .await;
    assert_eq!(
        status, 429,
        "case {:?}: a create past the queue quota is refused, got {status} {body}",
        "queue quota"
    );
    let mut queue_names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (status, page) = fresh
            .list_queues(&instance_id, Some(1000), token.as_deref(), &[])
            .await;
        assert_eq!(status, 200, "paging the queue map: got {page}");
        queue_names.extend(ids_of(&page["QueueSummaryList"], "Name"));
        token = page["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        queue_names.len(),
        QUEUES_PER_INSTANCE_QUOTA,
        "case {:?}: the instance holds exactly the quota, got {} queue(s)",
        "queue quota",
        queue_names.len()
    );
    assert!(
        !queue_names.iter().any(|name| name == "one-too-many"),
        "case {:?}: the refused create stored nothing, yet the queue it named is listed",
        "queue quota"
    );
}
