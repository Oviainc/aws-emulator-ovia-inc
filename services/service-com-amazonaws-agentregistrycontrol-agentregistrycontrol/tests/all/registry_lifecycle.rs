// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the registry + registry-record slice of
//! `agent-registry-control`, driven over HTTP through the real router so every
//! assertion goes through routing, deserialization, the handler and serialization.
//!
//! The rule under test: each of these operations computes its answer from the state the
//! create operations persisted — a create stores the whole request and mints the
//! identifiers the model's patterns describe, a read reports exactly what is stored, a
//! delete removes it, and the asynchronous statuses the model documents (`CREATING` ->
//! `READY`, `CREATING` -> `DRAFT`, `DELETING` -> gone) are owed by the clock rather than
//! by the caller's next call. Every test therefore asserts over a TABLE of inputs, so it
//! holds for a class of requests and not for one recorded shape.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region in the test's SigV4 credential scope; every minted ARN must carry it.
const REGION: &str = "us-west-2";

/// A signed-looking request, so the router resolves this service's signing name
/// (`agent-registry`) and the handlers read the region from the credential scope.
/// The signature itself is never checked: the test router runs with authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTAGENTREGISTRY/20260101/us-west-2/agent-registry/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// Longer than the service's own asynchronous-transition span, so a transition the clock
/// owes has certainly come due by the time the next call is made.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// Wait out a transition the clock owes, without blocking the test runtime.
async fn wait_past_the_transition() {
    tokio::time::sleep(PAST_THE_TRANSITION).await;
}

/// One HTTP answer, reduced to what these tests assert on.
struct Reply {
    status: u16,
    body: Value,
    /// The wire error code (`x-amzn-errortype`), present only on a failure.
    error_code: Option<String>,
}

impl Reply {
    fn field(&self, name: &str) -> &Value {
        &self.body[name]
    }

    fn str_field(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }
}

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(
            engine,
        ),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Reply {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://agent-registry-control.{REGION}.amazonaws.com{path}"
        ))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.rsplit('#').next().unwrap_or(value).to_string());
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        body,
        error_code,
    }
}

/// Percent-encode a path segment the way an SDK encodes a non-greedy `@httpLabel`, so an
/// ARN (which carries `:` and `/`) can be given where the model allows the ARN form.
/// Every byte that is not ASCII-alphanumeric becomes `%XX`, which is what an SDK's
/// strictest label encoding does and is always a valid encoding of the byte.
fn encode_label(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn id_of(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

fn is_alnum_within(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len()) && value.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A sortable key for a modelled timestamp, whichever wire form the protocol renders it
/// in: epoch seconds (zero-padded so the keys sort numerically) or an RFC-3339 string
/// (which sorts lexicographically). Empty for anything that is not a timestamp at all.
fn timestamp_key(value: &Value) -> String {
    match value {
        Value::Number(number) => format!("{:020.3}", number.as_f64().unwrap_or_default()),
        Value::String(text) => text.clone(),
        _ => String::new(),
    }
}

/// `^arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/[a-zA-Z0-9]{12,16}$`
fn registry_arn_is_well_formed(arn: &str) -> bool {
    let prefix = format!(
        "arn:aws:agent-registry:{REGION}:{}:registry/",
        engine::emulator_account_id()
    );
    arn.strip_prefix(&prefix)
        .is_some_and(|id| is_alnum_within(id, 12, 16))
}

/// `^…:registry/[a-zA-Z0-9]{12,16}/record/[a-zA-Z0-9]{12}$`
fn record_arn_is_well_formed(arn: &str, registry_arn: &str) -> bool {
    arn.strip_prefix(&format!("{registry_arn}/record/"))
        .is_some_and(|id| is_alnum_within(id, 12, 12))
}

async fn create_registry(router: &MultiServiceRouter, body: Value) -> Reply {
    call(router, "POST", "/registries", Some(body)).await
}

async fn get_registry(router: &MultiServiceRouter, identifier: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/registries/{}", encode_label(identifier)),
        None,
    )
    .await
}

async fn create_record(router: &MultiServiceRouter, registry_id: &str, body: Value) -> Reply {
    call(
        router,
        "POST",
        &format!("/registries/{}/records", encode_label(registry_id)),
        Some(body),
    )
    .await
}

async fn get_record(router: &MultiServiceRouter, registry_id: &str, record_id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!(
            "/registries/{}/records/{}",
            encode_label(registry_id),
            encode_label(record_id)
        ),
        None,
    )
    .await
}

async fn delete_record(router: &MultiServiceRouter, registry_id: &str, record_id: &str) -> Reply {
    call(
        router,
        "DELETE",
        &format!(
            "/registries/{}/records/{}",
            encode_label(registry_id),
            encode_label(record_id)
        ),
        None,
    )
    .await
}

async fn list_records(router: &MultiServiceRouter, registry_id: &str, body: Value) -> Reply {
    call(
        router,
        "POST",
        &format!("/registries/{}/records-list", encode_label(registry_id)),
        Some(body),
    )
    .await
}

/// Create a registry and wait out the creation the clock owes, so it is `READY`.
/// Returns its registry id.
async fn ready_registry(router: &MultiServiceRouter, name: &str) -> String {
    let created = create_registry(router, json!({ "name": name })).await;
    assert_eq!(
        created.status, 202,
        "case {name}: CreateRegistry must answer 202, got {} {:?}",
        created.status, created.body
    );
    let registry_id = id_of(created.str_field("registryArn")).to_string();
    wait_past_the_transition().await;
    let got = get_registry(router, &registry_id).await;
    assert_eq!(
        got.str_field("status"),
        "READY",
        "case {name}: a created registry becomes READY, got {:?}",
        got.body
    );
    registry_id
}

/// A `descriptors` value per `RecordType`, so each row of the record table carries a
/// descriptor of its own shape. Member names are `Descriptors`' own
/// (`smithy_query(query="shape", shape="Descriptors")`).
fn descriptors_for(record_type: &str) -> Value {
    match record_type {
        "MCP" => json!({ "mcpServer": { "data": "{\"tools\":[]}", "dataSchemaVersion": "2025-06-18" } }),
        "AGENT" => json!({ "a2aAgentCard": { "data": "{\"name\":\"checkout\"}", "dataSchemaVersion": "0.2.1" } }),
        "SKILL" => json!({ "agentSkillsDefinition": { "data": "{\"skills\":[]}" } }),
        "GATEWAY" => json!({ "http": { "source": { "fromUrl": { "url": "https://example.test/gateway" } } } }),
        _ => json!({ "custom": { "data": "{\"anything\":true}" } }),
    }
}

#[tokio::test]
async fn create_registry_persists_every_member_and_get_registry_reports_it() {
    // Rows differ along WHICH optional members the request carries and along the size of
    // `name` (the model's 1..64 bound), so the echo cannot be a fixed shape.
    let cases: Vec<(&str, Value)> = vec![
        ("only the required name", json!({ "name": "a" })),
        (
            "description and tags",
            json!({
                "name": "demo-registry",
                "description": "demo",
                "tags": { "team": "agents", "env": "test" }
            }),
        ),
        (
            "encryption and discovery configuration",
            json!({
                "name": "encrypted.registry/one",
                "encryptionConfiguration": { "kmsKeyArn": "arn:aws:kms:us-west-2:000000000000:key/12345678-1234-1234-1234-123456789012" },
                "discoveryConfiguration": { "authorizerType": "AWS_IAM" }
            }),
        ),
        (
            "approval, metadata schema and auto-detection configuration",
            json!({
                "name": "curated_registry-2",
                "approvalConfiguration": { "autoApprovalRules": ["APPROVE_ALL"] },
                "customMetadataSchemaConfiguration": { "defaultSchema": "{\"type\":\"object\"}" },
                "autoDetectionConfiguration": { "scope": "ORGANIZATION", "enabled": true }
            }),
        ),
        (
            "a name at the model's 64-character bound",
            json!({ "name": "r".repeat(64) }),
        ),
    ];

    let router = router();
    for (case, body) in cases {
        let created = create_registry(&router, body.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateRegistry answers 202, got {} {:?}",
            created.status, created.body
        );
        let registry_arn = created.str_field("registryArn").to_string();
        assert!(
            registry_arn_is_well_formed(&registry_arn),
            "case {case:?}: registryArn must be arn:aws:agent-registry:<region>:<account>:registry/<12-16 alnum>, got {registry_arn:?}"
        );
        let registry_id = id_of(&registry_arn).to_string();

        // Creation is asynchronous, so a caller reading immediately still sees CREATING.
        let creating = get_registry(&router, &registry_id).await;
        assert_eq!(
            creating.status, 200,
            "case {case:?}: GetRegistry answers 200, got {} {:?}",
            creating.status, creating.body
        );
        assert_eq!(
            creating.str_field("status"),
            "CREATING",
            "case {case:?}: a registry begins CREATING, got {:?}",
            creating.body
        );

        wait_past_the_transition().await;
        let got = get_registry(&router, &registry_id).await;
        assert_eq!(
            got.str_field("status"),
            "READY",
            "case {case:?}: the clock owes CREATING -> READY, got {:?}",
            got.body
        );
        assert_eq!(
            got.str_field("registryId"),
            registry_id,
            "case {case:?}: GetRegistry reports the minted id, got {:?}",
            got.body
        );
        assert_eq!(
            got.str_field("registryArn"),
            registry_arn,
            "case {case:?}: GetRegistry reports the minted ARN, got {:?}",
            got.body
        );

        // Every member the request carried is reported back from state, and one it did
        // not carry is absent rather than defaulted.
        for member in [
            "name",
            "description",
            "encryptionConfiguration",
            "discoveryConfiguration",
            "approvalConfiguration",
            "customMetadataSchemaConfiguration",
        ] {
            match body.get(member) {
                Some(sent) => assert_eq!(
                    got.field(member),
                    sent,
                    "case {case:?}: {member} must be what CreateRegistry stored, got {:?}",
                    got.field(member)
                ),
                None => assert!(
                    got.field(member).is_null(),
                    "case {case:?}: {member} was never sent, so nothing may be reported, got {:?}",
                    got.field(member)
                ),
            }
        }

        match body.get("autoDetectionConfiguration") {
            Some(sent) => {
                assert_eq!(
                    &got.field("autoDetection")["configuration"],
                    sent,
                    "case {case:?}: autoDetection reports the configuration asked for, got {:?}",
                    got.field("autoDetection")
                );
                assert_eq!(
                    got.field("autoDetection")["status"],
                    json!("ACTIVE"),
                    "case {case:?}: an enabled auto-detection is ACTIVE, got {:?}",
                    got.field("autoDetection")
                );
            }
            None => assert!(
                got.field("autoDetection").is_null(),
                "case {case:?}: no auto-detection was asked for, got {:?}",
                got.field("autoDetection")
            ),
        }

        let created_at = timestamp_key(got.field("createdAt"));
        let updated_at = timestamp_key(got.field("updatedAt"));
        assert!(
            !created_at.is_empty() && !updated_at.is_empty(),
            "case {case:?}: both timestamps are reported, got createdAt={:?} updatedAt={:?}",
            got.field("createdAt"),
            got.field("updatedAt")
        );
        assert!(
            updated_at >= created_at,
            "case {case:?}: the READY transition is not before creation, got createdAt={created_at:?} updatedAt={updated_at:?}"
        );

        // The model lets the label be the registry's full ARN as well as its bare id.
        let by_arn = get_registry(&router, &registry_arn).await;
        assert_eq!(
            by_arn.status, 200,
            "case {case:?}: GetRegistry accepts the ARN form, got {} {:?}",
            by_arn.status, by_arn.body
        );
        assert_eq!(
            by_arn.str_field("registryId"),
            registry_id,
            "case {case:?}: the ARN form names the same registry, got {:?}",
            by_arn.body
        );
    }
}

#[tokio::test]
async fn an_unknown_registry_or_record_is_a_resource_not_found() {
    let router = router();
    let registry_id = ready_registry(&router, "lookup-registry").await;
    let registry_arn = get_registry(&router, &registry_id)
        .await
        .str_field("registryArn")
        .to_string();

    // Rows: a bare id, the ARN form, and a record under a registry that does exist.
    let absent_registry = "zzzz999999zz";
    let cases: Vec<(&str, Reply)> = vec![
        (
            "bare registry id",
            get_registry(&router, absent_registry).await,
        ),
        (
            "registry ARN form",
            get_registry(
                &router,
                &format!(
                    "arn:aws:agent-registry:{REGION}:{}:registry/{absent_registry}",
                    engine::emulator_account_id()
                ),
            )
            .await,
        ),
        (
            "record of an existing registry",
            get_record(&router, &registry_id, "aaaa11112222").await,
        ),
        (
            "record of an absent registry",
            get_record(&router, absent_registry, "aaaa11112222").await,
        ),
        (
            "record named by its ARN form",
            get_record(
                &router,
                &registry_id,
                &format!("{registry_arn}/record/aaaa11112222"),
            )
            .await,
        ),
        (
            "deleting a record that is not there",
            delete_record(&router, &registry_id, "aaaa11112222").await,
        ),
        (
            "listing records of an absent registry",
            list_records(&router, absent_registry, json!({})).await,
        ),
    ];

    for (case, reply) in cases {
        assert_eq!(
            reply.status, 404,
            "case {case:?}: an unknown resource is 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: the modelled error code is ResourceNotFoundException, got {:?}",
            reply.error_code
        );
    }
}

#[tokio::test]
async fn a_duplicate_registry_name_conflicts_and_a_replayed_client_token_creates_nothing() {
    // Rows differ along the name's shape and the token's length (the model's 33..256).
    let cases = [
        ("short name", "x", "aaaaaaaabbbbccccddddeeeeffff11112222"),
        (
            "dotted name",
            "orders.registry",
            "tokenfortheordersregistrycreate00001",
        ),
        (
            "underscored name",
            "fleet-registry_01",
            "tokenforthefleetregistrycreate000002",
        ),
    ];

    let router = router();
    for (case, name, token) in cases {
        let first = create_registry(&router, json!({ "name": name, "clientToken": token })).await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: the first create answers 202, got {} {:?}",
            first.status, first.body
        );
        let arn = first.str_field("registryArn").to_string();

        // Same token, same request: the service ignores the request and answers the ARN
        // the first call minted — no second registry.
        let replay = create_registry(&router, json!({ "name": name, "clientToken": token })).await;
        assert_eq!(
            replay.status, 202,
            "case {case:?}: a replayed token is not an error, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.str_field("registryArn"),
            arn,
            "case {case:?}: a replayed token answers the same ARN, got {:?}",
            replay.body
        );

        // A different token with a name a live registry holds is a conflict.
        let duplicate = create_registry(
            &router,
            json!({ "name": name, "clientToken": format!("other{token}") }),
        )
        .await;
        assert_eq!(
            duplicate.status, 409,
            "case {case:?}: a duplicate name is 409, got {} {:?}",
            duplicate.status, duplicate.body
        );
        assert_eq!(
            duplicate.error_code.as_deref(),
            Some("ConflictException"),
            "case {case:?}: the modelled error code is ConflictException, got {:?}",
            duplicate.error_code
        );

        // And the registry the first call created is still the one in state.
        let got = get_registry(&router, id_of(&arn)).await;
        assert_eq!(
            got.str_field("name"),
            name,
            "case {case:?}: the stored registry is unchanged, got {:?}",
            got.body
        );
    }
}

#[tokio::test]
async fn create_registry_record_persists_every_member_and_get_registry_record_reports_it() {
    let router = router();
    let registry_id = ready_registry(&router, "record-registry").await;
    let registry_arn = get_registry(&router, &registry_id)
        .await
        .str_field("registryArn")
        .to_string();

    // One row per `RecordType` the model declares (source: the RecordType enum,
    // `smithy_query(query="shape", shape="RecordType")`: AGENT | CUSTOM | GATEWAY | MCP |
    // SKILL), each with a descriptor of its own shape and a different set of optional
    // members — including the row that omits `recordVersion`.
    let cases: Vec<(&str, Value)> = vec![
        (
            "MCP with a version and metadata",
            json!({
                "name": "search-mcp",
                "recordType": "MCP",
                "descriptors": descriptors_for("MCP"),
                "displayName": "Search MCP",
                "description": "an MCP server",
                "recordVersion": "1.2.3",
                "customMetadata": { "owner": "search", "critical": true },
                "tags": { "env": "test" }
            }),
        ),
        (
            "AGENT without a version",
            json!({
                "name": "checkout-agent",
                "recordType": "AGENT",
                "descriptors": descriptors_for("AGENT")
            }),
        ),
        (
            "SKILL with provenance",
            json!({
                "name": "summarise.skill",
                "recordType": "SKILL",
                "descriptors": descriptors_for("SKILL"),
                "recordVersion": "2026-01-01",
                "provenance": [{
                    "relation": "DETECTED_FROM",
                    "sourceId": "arn:aws:bedrock-agentcore:us-west-2:000000000000:runtime/abc",
                    "sourceType": "AWS::BedrockAgentCore::Runtime"
                }]
            }),
        ),
        (
            "GATEWAY from a URL descriptor",
            json!({
                "name": "edge_gateway",
                "recordType": "GATEWAY",
                "descriptors": descriptors_for("GATEWAY"),
                "displayName": "Edge gateway"
            }),
        ),
        (
            "CUSTOM with the longest allowed display name",
            json!({
                "name": "custom-thing",
                "recordType": "CUSTOM",
                "descriptors": descriptors_for("CUSTOM"),
                "displayName": "d".repeat(255)
            }),
        ),
    ];

    for (case, body) in cases {
        let created = create_record(&router, &registry_id, body.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateRegistryRecord answers 202, got {} {:?}",
            created.status, created.body
        );
        let record_arn = created.str_field("recordArn").to_string();
        assert!(
            record_arn_is_well_formed(&record_arn, &registry_arn),
            "case {case:?}: recordArn must be <registryArn>/record/<12 alnum>, got {record_arn:?}"
        );
        assert_eq!(
            created.str_field("status"),
            "CREATING",
            "case {case:?}: a record is returned CREATING while it is processed, got {:?}",
            created.body
        );
        let record_id = id_of(&record_arn).to_string();

        let creating = get_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            creating.str_field("status"),
            "CREATING",
            "case {case:?}: the transient status is visible to the next caller, got {:?}",
            creating.body
        );

        wait_past_the_transition().await;
        let got = get_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: GetRegistryRecord answers 200, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.str_field("status"),
            "DRAFT",
            "case {case:?}: the clock owes CREATING -> DRAFT, got {:?}",
            got.body
        );
        assert_eq!(
            got.str_field("recordId"),
            record_id,
            "case {case:?}: the reported id is the minted one, got {:?}",
            got.body
        );
        assert_eq!(
            got.str_field("recordArn"),
            record_arn,
            "case {case:?}: the reported ARN is the minted one, got {:?}",
            got.body
        );
        assert_eq!(
            got.str_field("registryArn"),
            registry_arn,
            "case {case:?}: the record reports its parent registry, got {:?}",
            got.body
        );

        for member in [
            "name",
            "recordType",
            "descriptors",
            "displayName",
            "description",
            "customMetadata",
            "provenance",
        ] {
            match body.get(member) {
                Some(sent) => assert_eq!(
                    got.field(member),
                    sent,
                    "case {case:?}: {member} must be what CreateRegistryRecord stored, got {:?}",
                    got.field(member)
                ),
                None => assert!(
                    got.field(member).is_null(),
                    "case {case:?}: {member} was never sent, so nothing may be reported, got {:?}",
                    got.field(member)
                ),
            }
        }

        let expected_version = body
            .get("recordVersion")
            .and_then(Value::as_str)
            .unwrap_or("1");
        assert_eq!(
            got.str_field("recordVersion"),
            expected_version,
            "case {case:?}: recordVersion is the one stored, or the first version when none was given, got {:?}",
            got.body
        );
        assert_eq!(
            got.str_field("createdBy"),
            engine::emulator_account_id(),
            "case {case:?}: createdBy is the calling account, got {:?}",
            got.body
        );
        assert_eq!(
            got.field("createdByAutoDetection"),
            &json!(false),
            "case {case:?}: a record created through the API is not auto-detected, got {:?}",
            got.body
        );
    }
}

#[tokio::test]
async fn creating_a_record_needs_a_ready_registry() {
    let router = router();

    // A registry that is still being created cannot take records yet: it "becomes usable
    // once it reaches READY".
    let created = create_registry(&router, json!({ "name": "not-ready-yet" })).await;
    let registry_id = id_of(created.str_field("registryArn")).to_string();
    let too_early = create_record(
        &router,
        &registry_id,
        json!({
            "name": "too-early",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM")
        }),
    )
    .await;
    assert_eq!(
        too_early.status, 409,
        "case \"registry still CREATING\": a record cannot be created yet, got {} {:?}",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_code.as_deref(),
        Some("ConflictException"),
        "case \"registry still CREATING\": the modelled error code is ConflictException, got {:?}",
        too_early.error_code
    );

    // Once the clock has made it READY the same request succeeds.
    wait_past_the_transition().await;
    let accepted = create_record(
        &router,
        &registry_id,
        json!({
            "name": "too-early",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM")
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 202,
        "case \"registry READY\": the same request now succeeds, got {} {:?}",
        accepted.status, accepted.body
    );

    // A second record may not take a name a record of that registry already holds.
    let duplicate = create_record(
        &router,
        &registry_id,
        json!({
            "name": "too-early",
            "recordType": "MCP",
            "descriptors": descriptors_for("MCP")
        }),
    )
    .await;
    assert_eq!(
        duplicate.status, 409,
        "case \"duplicate record name\": record names are unique within a registry, got {} {:?}",
        duplicate.status, duplicate.body
    );
}

#[tokio::test]
async fn list_registry_records_pages_in_id_order_and_resumes_where_the_page_stopped() {
    let router = router();

    // Rows differ along how many records exist and how large a page is asked for,
    // including a page the same size as the set and one larger than it.
    let counts_and_pages = [(1usize, 1i64), (3, 1), (5, 2), (4, 4), (7, 3), (2, 100)];

    for (count, page_size) in counts_and_pages {
        let case = format!("{count} records, maxResults {page_size}");
        // A fresh registry per row, so each row's set is exactly what it created.
        let registry_id = ready_registry(&router, &format!("paging-{count}-{page_size}")).await;
        let registry_arn = get_registry(&router, &registry_id)
            .await
            .str_field("registryArn")
            .to_string();

        let mut created: Vec<String> = Vec::new();
        for index in 0..count {
            let reply = create_record(
                &router,
                &registry_id,
                json!({
                    "name": format!("record-{index}"),
                    "recordType": "CUSTOM",
                    "descriptors": descriptors_for("CUSTOM")
                }),
            )
            .await;
            assert_eq!(
                reply.status, 202,
                "case {case:?}: creating record {index} answers 202, got {} {:?}",
                reply.status, reply.body
            );
            created.push(id_of(reply.str_field("recordArn")).to_string());
        }
        created.sort();

        // Walk every page and collect what the pages reported.
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let body = match &token {
                Some(token) => json!({ "maxResults": page_size, "nextToken": token }),
                None => json!({ "maxResults": page_size }),
            };
            let reply = list_records(&router, &registry_id, body).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: ListRegistryRecords answers 200, got {} {:?}",
                reply.status, reply.body
            );
            let items = reply
                .field("registryRecords")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                items.len() <= page_size as usize,
                "case {case:?}: a page never exceeds maxResults, got {} items",
                items.len()
            );
            for item in &items {
                assert_eq!(
                    item["registryArn"].as_str().unwrap_or_default(),
                    registry_arn,
                    "case {case:?}: every summary reports its parent registry, got {item:?}"
                );
                seen.push(item["recordId"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= count + 1,
                "case {case:?}: pagination must terminate, got {pages} pages"
            );
            match reply.field("nextToken").as_str() {
                Some(next) => {
                    assert!(
                        !items.is_empty(),
                        "case {case:?}: a token is only handed out with a non-empty page"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }

        let mut sorted_seen = seen.clone();
        sorted_seen.sort();
        assert_eq!(
            sorted_seen, created,
            "case {case:?}: paging reports every stored record exactly once, got {seen:?}"
        );
        let in_id_order = seen.windows(2).all(|pair| pair[0] < pair[1]);
        assert!(
            in_id_order,
            "case {case:?}: pages run in record-id order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn list_registry_records_honours_the_filters_the_model_declares() {
    let router = router();
    let registry_id = ready_registry(&router, "filter-registry").await;

    // Three records that differ in name and recordType, so each filter attribute
    // (source: the RegistryRecordFilterName enum — name | status | recordType) selects a
    // different subset.
    for (name, record_type) in [
        ("alpha", "MCP"),
        ("beta", "AGENT"),
        ("gamma", "MCP"),
    ] {
        let reply = create_record(
            &router,
            &registry_id,
            json!({
                "name": name,
                "recordType": record_type,
                "descriptors": descriptors_for(record_type)
            }),
        )
        .await;
        assert_eq!(
            reply.status, 202,
            "case {name:?}: creating the record answers 202, got {} {:?}",
            reply.status, reply.body
        );
    }
    wait_past_the_transition().await;

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "by name",
            json!({ "filters": [{ "name": "name", "values": ["beta"] }] }),
            vec!["beta"],
        ),
        (
            "by recordType",
            json!({ "filters": [{ "name": "recordType", "values": ["MCP"] }] }),
            vec!["alpha", "gamma"],
        ),
        (
            "by status",
            json!({ "filters": [{ "name": "status", "values": ["DRAFT"] }] }),
            vec!["alpha", "beta", "gamma"],
        ),
        (
            "by a status nothing holds",
            json!({ "filters": [{ "name": "status", "values": ["APPROVED"] }] }),
            vec![],
        ),
        (
            "two filters together",
            json!({ "filters": [
                { "name": "recordType", "values": ["MCP"] },
                { "name": "name", "values": ["gamma"] }
            ] }),
            vec!["gamma"],
        ),
        ("no filter", json!({}), vec!["alpha", "beta", "gamma"]),
    ];

    for (case, body, expected) in cases {
        let reply = list_records(&router, &registry_id, body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ListRegistryRecords answers 200, got {} {:?}",
            reply.status, reply.body
        );
        let mut names: Vec<String> = reply
            .field("registryRecords")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| item["name"].as_str().unwrap_or_default().to_string())
            .collect();
        names.sort();
        let selected: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: the filter selects exactly the matching records, got {selected:?}"
        );
    }

    // A filter attribute outside `RegistryRecordFilterName`'s catalogue (source:
    // `REGISTRY_RECORD_FILTER_NAME_VALUES` = name | recordType | status) is refused, not
    // silently ignored: rows for an attribute the record has but the model does not make
    // filterable, for one no shape declares, and for the right name in the wrong case.
    for case in ["displayName", "not-an-attribute", "RecordType"] {
        let refused = list_records(
            &router,
            &registry_id,
            json!({ "filters": [{ "name": case, "values": ["alpha"] }] }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a filter name outside the model's catalogue is a 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the modelled error code is ValidationException, got {:?}",
            refused.error_code
        );
    }

    // A token this service never issued is refused, not treated as the first page.
    // Rows: one that is not even decodable, and one a different registry issued.
    let other_registry = ready_registry(&router, "other-filter-registry").await;
    for index in 0..2 {
        let reply = create_record(
            &router,
            &other_registry,
            json!({
                "name": format!("elsewhere-{index}"),
                "recordType": "CUSTOM",
                "descriptors": descriptors_for("CUSTOM")
            }),
        )
        .await;
        assert_eq!(
            reply.status, 202,
            "case \"seed the other registry\": got {} {:?}",
            reply.status, reply.body
        );
    }
    let issued_elsewhere = list_records(&router, &other_registry, json!({ "maxResults": 1 }))
        .await
        .str_field("nextToken")
        .to_string();
    assert!(
        !issued_elsewhere.is_empty(),
        "case \"token of another registry\": the other registry must hand out a token"
    );

    for (case, token) in [
        ("undecodable token", "not-a-token".to_string()),
        ("token of another registry", issued_elsewhere),
    ] {
        let refused = list_records(&router, &registry_id, json!({ "nextToken": token })).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a token this registry did not issue is a 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the modelled error code is ValidationException, got {:?}",
            refused.error_code
        );
    }
}

#[tokio::test]
async fn list_registry_records_pages_at_the_models_maximum_when_no_max_results_is_given() {
    let router = router();
    let registry_id = ready_registry(&router, "default-page-registry").await;

    // One more record than the model's `MaxResults` maximum (1..100), so a request that
    // names no `maxResults` cannot answer with the whole registry.
    let total = 101;
    for index in 0..total {
        let reply = create_record(
            &router,
            &registry_id,
            json!({
                "name": format!("bulk-{index:03}"),
                "recordType": "CUSTOM",
                "descriptors": descriptors_for("CUSTOM")
            }),
        )
        .await;
        assert_eq!(
            reply.status, 202,
            "case \"bulk record {index}\": got {} {:?}",
            reply.status, reply.body
        );
    }

    let first = list_records(&router, &registry_id, json!({})).await;
    let first_page = first
        .field("registryRecords")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        first_page.len(),
        100,
        "case \"no maxResults\": the page is capped at the model's maximum, got {} items",
        first_page.len()
    );
    let token = first.str_field("nextToken").to_string();
    assert!(
        !token.is_empty(),
        "case \"no maxResults\": a capped page hands out a nextToken, got {:?}",
        first.body
    );

    let second = list_records(&router, &registry_id, json!({ "nextToken": token })).await;
    let second_page = second
        .field("registryRecords")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        second_page.len(),
        total - 100,
        "case \"resume the default page\": the rest of the registry follows, got {} items",
        second_page.len()
    );
    assert!(
        second.field("nextToken").is_null(),
        "case \"resume the default page\": the last page carries no token, got {:?}",
        second.body
    );
}

#[tokio::test]
async fn an_idempotency_token_is_scoped_to_its_operation_and_dropped_with_its_resource() {
    let router = router();
    // One token, used by two different creates: the record create must create a record
    // rather than replay the registry's answer.
    let token = "onetokenusedbytwodifferentcreates001";

    let registry = create_registry(&router, json!({ "name": "scoped-token", "clientToken": token }))
        .await;
    assert_eq!(
        registry.status, 202,
        "case \"create the registry\": got {} {:?}",
        registry.status, registry.body
    );
    let registry_arn = registry.str_field("registryArn").to_string();
    let registry_id = id_of(&registry_arn).to_string();
    wait_past_the_transition().await;

    let record = create_record(
        &router,
        &registry_id,
        json!({
            "name": "scoped-token-record",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM"),
            "clientToken": token
        }),
    )
    .await;
    assert_eq!(
        record.status, 202,
        "case \"same token, other operation\": got {} {:?}",
        record.status, record.body
    );
    let record_arn = record.str_field("recordArn").to_string();
    assert!(
        record_arn.starts_with(&format!("{registry_arn}/record/")),
        "case \"same token, other operation\": the record create must mint a record ARN, got {record_arn:?}"
    );

    // Deleting the resource drops what the token remembered, so the same token creates
    // afresh instead of answering with something that no longer exists.
    let deleted = delete_record(&router, &registry_id, id_of(&record_arn)).await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete the record\": got {} {:?}",
        deleted.status, deleted.body
    );
    let recreated = create_record(
        &router,
        &registry_id,
        json!({
            "name": "scoped-token-record",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM"),
            "clientToken": token
        }),
    )
    .await;
    assert_eq!(
        recreated.status, 202,
        "case \"replay after deletion\": got {} {:?}",
        recreated.status, recreated.body
    );
    assert_ne!(
        recreated.str_field("recordArn"),
        record_arn,
        "case \"replay after deletion\": a token naming a deleted record must create a new one, got {:?}",
        recreated.body
    );
    let readable = get_record(&router, &registry_id, id_of(recreated.str_field("recordArn"))).await;
    assert_eq!(
        readable.status, 200,
        "case \"replay after deletion\": the new record is readable, got {} {:?}",
        readable.status, readable.body
    );

    // A replay stays idempotent even once the parent registry has left READY: the
    // remembered answer is consulted before any state refusal, so deleting the registry
    // (which moves it to DELETING) does not turn a replay into a conflict.
    let registry_deleted = call(
        &router,
        "DELETE",
        &format!("/registries/{registry_id}"),
        None,
    )
    .await;
    assert_eq!(
        registry_deleted.str_field("status"),
        "DELETING",
        "case \"delete the parent registry\": got {} {:?}",
        registry_deleted.status,
        registry_deleted.body
    );
    let replay_while_deleting = create_record(
        &router,
        &registry_id,
        json!({
            "name": "scoped-token-record",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM"),
            "clientToken": token
        }),
    )
    .await;
    assert_eq!(
        replay_while_deleting.status, 202,
        "case \"replay while the registry is DELETING\": a replay is not refused, got {} {:?}",
        replay_while_deleting.status, replay_while_deleting.body
    );
    assert_eq!(
        replay_while_deleting.str_field("recordArn"),
        recreated.str_field("recordArn"),
        "case \"replay while the registry is DELETING\": the replay answers the stored record, got {:?}",
        replay_while_deleting.body
    );
}

#[tokio::test]
async fn delete_registry_record_removes_it_from_state() {
    let router = router();
    let registry_id = ready_registry(&router, "delete-record-registry").await;

    let mut ids = Vec::new();
    for index in 0..3 {
        let reply = create_record(
            &router,
            &registry_id,
            json!({
                "name": format!("doomed-{index}"),
                "recordType": "CUSTOM",
                "descriptors": descriptors_for("CUSTOM")
            }),
        )
        .await;
        ids.push(id_of(reply.str_field("recordArn")).to_string());
    }

    // Rows: delete each of the three in turn, so the removal is not tied to one position.
    for (index, record_id) in ids.iter().enumerate() {
        let case = format!("record {index}");
        let deleted = delete_record(&router, &registry_id, record_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteRegistryRecord answers 200, got {} {:?}",
            deleted.status, deleted.body
        );

        let got = get_record(&router, &registry_id, record_id).await;
        assert_eq!(
            got.status, 404,
            "case {case:?}: a deleted record is gone, got {} {:?}",
            got.status, got.body
        );

        let listed = list_records(&router, &registry_id, json!({})).await;
        let remaining: Vec<String> = listed
            .field("registryRecords")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| item["recordId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            !remaining.contains(record_id),
            "case {case:?}: a deleted record is no longer listed, got {remaining:?}"
        );
        assert_eq!(
            remaining.len(),
            ids.len() - index - 1,
            "case {case:?}: only the deleted record left the registry, got {remaining:?}"
        );

        // Deleting it again is the modelled 404 for an unknown record, not a panic.
        let again = delete_record(&router, &registry_id, record_id).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: deleting an absent record is 404, got {} {:?}",
            again.status, again.body
        );
    }
}

#[tokio::test]
async fn delete_registry_moves_it_to_deleting_and_then_removes_it_with_its_records() {
    let router = router();
    // Created with a token, so the test can also show that removing the registry drops
    // what that token remembered.
    let token = "tokenforthedoomedregistrycreate00001";
    let created = create_registry(
        &router,
        json!({ "name": "doomed-registry", "clientToken": token }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"create the registry\": got {} {:?}",
        created.status, created.body
    );
    let first_arn = created.str_field("registryArn").to_string();
    let registry_id = id_of(&first_arn).to_string();
    wait_past_the_transition().await;
    let record = create_record(
        &router,
        &registry_id,
        json!({
            "name": "doomed-record",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM")
        }),
    )
    .await;
    let record_id = id_of(record.str_field("recordArn")).to_string();

    let deleted = call(
        &router,
        "DELETE",
        &format!("/registries/{registry_id}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 202,
        "case \"delete a READY registry\": DeleteRegistry answers 202, got {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.str_field("status"),
        "DELETING",
        "case \"delete a READY registry\": deletion is asynchronous, so the status is DELETING, got {:?}",
        deleted.body
    );

    // While deletion is in flight the registry still reports itself, as DELETING.
    let during = get_registry(&router, &registry_id).await;
    assert_eq!(
        during.str_field("status"),
        "DELETING",
        "case \"read during deletion\": the transient status is visible, got {:?}",
        during.body
    );

    // @idempotent: a second delete reports the same status and does not fail.
    let again = call(
        &router,
        "DELETE",
        &format!("/registries/{registry_id}"),
        None,
    )
    .await;
    assert_eq!(
        again.status, 202,
        "case \"delete again\": a repeated delete is accepted, got {} {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.str_field("status"),
        "DELETING",
        "case \"delete again\": the status is unchanged, got {:?}",
        again.body
    );

    wait_past_the_transition().await;
    let gone = get_registry(&router, &registry_id).await;
    assert_eq!(
        gone.status, 404,
        "case \"read after deletion\": the registry is removed, got {} {:?}",
        gone.status, gone.body
    );
    let record_gone = get_record(&router, &registry_id, &record_id).await;
    assert_eq!(
        record_gone.status, 404,
        "case \"read a record after its registry was deleted\": records go with the registry, got {} {:?}",
        record_gone.status, record_gone.body
    );

    // The name the deleted registry held is free again, and the token that created it
    // no longer names anything, so the same token creates a NEW registry.
    let reused = create_registry(
        &router,
        json!({ "name": "doomed-registry", "clientToken": token }),
    )
    .await;
    assert_eq!(
        reused.status, 202,
        "case \"reuse the name of a removed registry\": the name is free again, got {} {:?}",
        reused.status, reused.body
    );
    assert_ne!(
        reused.str_field("registryArn"),
        first_arn,
        "case \"replay a token naming a removed registry\": it must create a new registry, got {:?}",
        reused.body
    );
}
