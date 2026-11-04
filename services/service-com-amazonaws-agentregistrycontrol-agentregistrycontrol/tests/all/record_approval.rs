// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `SubmitRegistryRecordForApproval`, driven over HTTP through the
//! real router so every assertion goes through routing, deserialization, the handler and
//! serialization.
//!
//! The rule under test: submitting a `DRAFT` record moves it into the approval workflow
//! of the registry that holds it, and the status it lands in is computed from that
//! registry's approval configuration as it stands at submission time — a configuration
//! carrying an auto-approval rule that covers the record approves it on submission,
//! anything else (no configuration, an empty rule list) leaves it waiting for manual
//! review. The resulting status is persisted, so every later read reports it, and a
//! record in any other status cannot be submitted at all.
//!
//! Every test asserts over a TABLE of inputs, so it holds for a class of requests rather
//! than for one recorded shape.

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
/// owes (a registry becoming `READY`, a record becoming `DRAFT`) has certainly come due.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// One HTTP answer, reduced to what these tests assert on.
struct Reply {
    status: u16,
    body: Value,
    /// The wire error code (`x-amzn-errortype`), present only on a failure.
    error_code: Option<String>,
}

impl Reply {
    fn str_field(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    fn message(&self) -> String {
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_default()
            .to_string()
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

async fn list_records(router: &MultiServiceRouter, registry_id: &str, body: Value) -> Reply {
    call(
        router,
        "POST",
        &format!("/registries/{}/records-list", encode_label(registry_id)),
        Some(body),
    )
    .await
}

async fn submit_record(router: &MultiServiceRouter, registry_id: &str, record_id: &str) -> Reply {
    call(
        router,
        "POST",
        &format!(
            "/registries/{}/records/{}/submit-for-approval",
            encode_label(registry_id),
            encode_label(record_id)
        ),
        None,
    )
    .await
}

/// Create a registry with `body` and wait out the creation the clock owes, so it is
/// `READY`. Returns its registry id.
async fn ready_registry(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let created = create_registry(router, body).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateRegistry must answer 202, got {} {:?}",
        created.status, created.body
    );
    let registry_id = id_of(created.str_field("registryArn")).to_string();
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let got = get_registry(router, &registry_id).await;
    assert_eq!(
        got.str_field("status"),
        "READY",
        "case {case:?}: a created registry becomes READY, got {:?}",
        got.body
    );
    registry_id
}

/// A `descriptors` value per `RecordType`, so each row of a record table carries a
/// descriptor of its own shape. Member names are `Descriptors`' own
/// (`smithy_query(query="shape", shape="Descriptors")`).
fn descriptors_for(record_type: &str) -> Value {
    match record_type {
        "MCP" => {
            json!({ "mcpServer": { "data": "{\"tools\":[]}", "dataSchemaVersion": "2025-06-18" } })
        }
        "AGENT" => {
            json!({ "a2aAgentCard": { "data": "{\"name\":\"checkout\"}", "dataSchemaVersion": "0.2.1" } })
        }
        "SKILL" => json!({ "agentSkillsDefinition": { "data": "{\"skills\":[]}" } }),
        "GATEWAY" => {
            json!({ "http": { "source": { "fromUrl": { "url": "https://example.test/gateway" } } } })
        }
        _ => json!({ "custom": { "data": "{\"anything\":true}" } }),
    }
}

/// Create a record of `record_type` named `name` and wait out the creation the clock
/// owes, so it is `DRAFT`. Returns its record id.
async fn draft_record(
    router: &MultiServiceRouter,
    case: &str,
    registry_id: &str,
    name: &str,
    record_type: &str,
) -> String {
    let created = create_record(
        router,
        registry_id,
        json!({
            "name": name,
            "recordType": record_type,
            "descriptors": descriptors_for(record_type),
        }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateRegistryRecord must answer 202, got {} {:?}",
        created.status, created.body
    );
    let record_id = id_of(created.str_field("recordArn")).to_string();
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let got = get_record(router, registry_id, &record_id).await;
    assert_eq!(
        got.str_field("status"),
        "DRAFT",
        "case {case:?}: a created record becomes DRAFT, got {:?}",
        got.body
    );
    record_id
}

/// The approval configurations a registry can carry, each with the status its registry
/// gives a submitted record. `autoApprovalRules` entries come from
/// `AutoApprovalRule`'s own catalogue (`smithy_query(query="shape",
/// shape="AutoApprovalRule")`), whose only value is `APPROVE_ALL`; the shape documents
/// that an omitted or empty rule list means manual review.
fn approval_configuration_table() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "no approvalConfiguration at all",
            json!({}),
            "PENDING_APPROVAL",
        ),
        (
            "an approvalConfiguration with no rule list",
            json!({ "approvalConfiguration": {} }),
            "PENDING_APPROVAL",
        ),
        (
            "an empty autoApprovalRules list",
            json!({ "approvalConfiguration": { "autoApprovalRules": [] } }),
            "PENDING_APPROVAL",
        ),
        (
            "the APPROVE_ALL auto-approval rule",
            json!({ "approvalConfiguration": { "autoApprovalRules": ["APPROVE_ALL"] } }),
            "APPROVED",
        ),
    ]
}

#[tokio::test]
async fn submitting_a_draft_record_lands_in_the_status_the_registrys_approval_configuration_decides()
{
    // Rows differ along the registry's approval configuration (the member that decides
    // the outcome) and along the record type submitted, so the answer cannot be a fixed
    // status for a fixed request shape.
    let record_types = ["MCP", "AGENT", "CUSTOM"];
    let router = router();

    for (index, (case, configuration, expected_status)) in
        approval_configuration_table().into_iter().enumerate()
    {
        let mut body = json!({ "name": format!("approval-registry-{index}") });
        for (member, value) in configuration.as_object().into_iter().flatten() {
            body[member] = value.clone();
        }
        let registry_id = ready_registry(&router, case, body).await;

        for record_type in record_types {
            let case = format!("{case} / {record_type}");
            let record_id = draft_record(
                &router,
                &case,
                &registry_id,
                &format!("record-{record_type}"),
                record_type,
            )
            .await;
            let before = get_record(&router, &registry_id, &record_id).await;

            let submitted = submit_record(&router, &registry_id, &record_id).await;
            assert_eq!(
                submitted.status, 202,
                "case {case:?}: SubmitRegistryRecordForApproval answers 202, got {} {:?}",
                submitted.status, submitted.body
            );
            assert_eq!(
                submitted.str_field("status"),
                expected_status,
                "case {case:?}: expected status {expected_status}, got {:?}",
                submitted.body
            );
            assert_eq!(
                submitted.str_field("recordId"),
                record_id,
                "case {case:?}: expected recordId {record_id}, got {:?}",
                submitted.body
            );
            assert_eq!(
                submitted.str_field("recordArn"),
                before.str_field("recordArn"),
                "case {case:?}: expected recordArn {:?}, got {:?}",
                before.str_field("recordArn"),
                submitted.body
            );
            assert_eq!(
                submitted.str_field("registryArn"),
                before.str_field("registryArn"),
                "case {case:?}: expected registryArn {:?}, got {:?}",
                before.str_field("registryArn"),
                submitted.body
            );
            assert!(
                timestamp_key(&submitted.body["updatedAt"])
                    >= timestamp_key(&before.body["updatedAt"]),
                "case {case:?}: updatedAt must not go backwards from {:?}, got {:?}",
                before.body["updatedAt"],
                submitted.body["updatedAt"]
            );

            // The status is PERSISTED: a later read reports it, not the DRAFT the
            // record was in before.
            let after = get_record(&router, &registry_id, &record_id).await;
            assert_eq!(
                after.str_field("status"),
                expected_status,
                "case {case:?}: GetRegistryRecord reports the submitted status \
                 {expected_status}, got {:?}",
                after.body
            );
            assert_eq!(
                timestamp_key(&after.body["updatedAt"]),
                timestamp_key(&submitted.body["updatedAt"]),
                "case {case:?}: GetRegistryRecord must report the updatedAt the submit \
                 answered ({:?}), got {:?}",
                submitted.body["updatedAt"],
                after.body["updatedAt"]
            );
            assert_eq!(
                timestamp_key(&after.body["createdAt"]),
                timestamp_key(&before.body["createdAt"]),
                "case {case:?}: createdAt is not touched by a submission; expected {:?}, \
                 got {:?}",
                before.body["createdAt"],
                after.body["createdAt"]
            );
        }
    }
}

#[tokio::test]
async fn a_submitted_record_is_found_by_a_status_filter_and_hidden_from_the_others() {
    // Each row submits a record into a registry with its own approval configuration, so
    // the records in one registry end up spread across the statuses a submission can
    // produce plus the DRAFT of one never submitted.
    let router = router();
    let registry_id = ready_registry(
        &router,
        "status filter",
        json!({
            "name": "filterable-registry",
            "approvalConfiguration": { "autoApprovalRules": ["APPROVE_ALL"] }
        }),
    )
    .await;
    let approved = draft_record(&router, "approved", &registry_id, "auto", "MCP").await;
    let still_draft = draft_record(&router, "draft", &registry_id, "untouched", "AGENT").await;
    let submitted = submit_record(&router, &registry_id, &approved).await;
    assert_eq!(
        submitted.str_field("status"),
        "APPROVED",
        "case \"status filter\": expected APPROVED, got {:?}",
        submitted.body
    );

    let manual_registry = ready_registry(
        &router,
        "manual review",
        json!({ "name": "manual-registry" }),
    )
    .await;
    let pending = draft_record(&router, "pending", &manual_registry, "manual", "SKILL").await;
    let submitted = submit_record(&router, &manual_registry, &pending).await;
    assert_eq!(
        submitted.str_field("status"),
        "PENDING_APPROVAL",
        "case \"manual review\": expected PENDING_APPROVAL, got {:?}",
        submitted.body
    );

    // Rows span every status a record in these registries can be in, including two the
    // model declares that nothing here reached.
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("APPROVED", "APPROVED", vec![approved.as_str()]),
        ("DRAFT", "DRAFT", vec![still_draft.as_str()]),
        ("PENDING_APPROVAL", "PENDING_APPROVAL", vec![]),
        ("CREATING", "CREATING", vec![]),
        ("REJECTED", "REJECTED", vec![]),
    ];
    for (case, status, expected) in cases {
        let listed = list_records(
            &router,
            &registry_id,
            json!({ "filters": [{ "name": "status", "values": [status] }] }),
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListRegistryRecords answers 200, got {} {:?}",
            listed.status, listed.body
        );
        let ids: Vec<&str> = listed.body["registryRecords"]
            .as_array()
            .map(|records| {
                records
                    .iter()
                    .map(|record| record["recordId"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            ids, expected,
            "case {case:?}: a status filter returns exactly the records in that status; \
             expected {expected:?}, got {ids:?}"
        );
    }

    // The record submitted in the OTHER registry is not reachable through this one, so a
    // submission writes to the registry that holds the record and to no other.
    let listed = list_records(
        &router,
        &registry_id,
        json!({ "filters": [{ "name": "status", "values": ["PENDING_APPROVAL"] }] }),
    )
    .await;
    assert_eq!(
        listed.body["registryRecords"].as_array().map(Vec::len),
        Some(0),
        "case \"cross-registry\": the other registry's PENDING_APPROVAL record must not \
         be listed here, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn a_record_that_is_not_draft_cannot_be_submitted() {
    // Rows differ along WHY the record is not submittable: it is still being created,
    // or it has already been through the submission once.
    let router = router();

    for (case, auto_approve) in [("manual review", false), ("auto-approved", true)] {
        let mut body = json!({ "name": format!("conflicting-{}", case.replace(' ', "-")) });
        if auto_approve {
            body["approvalConfiguration"] = json!({ "autoApprovalRules": ["APPROVE_ALL"] });
        }
        let registry_id = ready_registry(&router, case, body).await;

        // Still CREATING: the creation the clock owes has not come due yet.
        let created = create_record(
            &router,
            &registry_id,
            json!({
                "name": "fresh",
                "recordType": "MCP",
                "descriptors": descriptors_for("MCP"),
            }),
        )
        .await;
        let fresh = id_of(created.str_field("recordArn")).to_string();
        let refused = submit_record(&router, &registry_id, &fresh).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?} / CREATING: a record that is not DRAFT is a 409, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ConflictException"),
            "case {case:?} / CREATING: expected ConflictException, got {:?}",
            refused.error_code
        );
        assert!(
            refused.message().contains("CREATING") && refused.message().contains(&fresh),
            "case {case:?} / CREATING: the message must name the record and its status, \
             got {:?}",
            refused.message()
        );

        // Already submitted: the second submission is refused and the status the first
        // one produced is left alone.
        let record_id = draft_record(&router, case, &registry_id, "submitted-once", "AGENT").await;
        let first = submit_record(&router, &registry_id, &record_id).await;
        let settled = first.str_field("status").to_string();
        let again = submit_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            again.status, 409,
            "case {case:?} / resubmitted: a record in {settled} cannot be submitted again, \
             got {} {:?}",
            again.status, again.body
        );
        let after = get_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            after.str_field("status"),
            settled,
            "case {case:?} / resubmitted: the refused resubmission must not change the \
             stored status {settled}, got {:?}",
            after.body
        );
    }
}

#[tokio::test]
async fn submitting_accepts_either_identifier_form_and_refuses_an_unknown_resource() {
    let router = router();
    let registry_id = ready_registry(
        &router,
        "identifier forms",
        json!({ "name": "identifier-registry" }),
    )
    .await;
    let registry = get_registry(&router, &registry_id).await;
    let registry_arn = registry.str_field("registryArn").to_string();

    // Both labels accept the resource's bare id or its full ARN, so the rows differ
    // along which form each label carries.
    let record_arn = {
        let record_id = draft_record(&router, "arn form", &registry_id, "arn-form", "MCP").await;
        get_record(&router, &registry_id, &record_id)
            .await
            .str_field("recordArn")
            .to_string()
    };
    let cases: Vec<(&str, String, String)> = vec![
        (
            "both as bare ids",
            registry_id.clone(),
            id_of(&record_arn).to_string(),
        ),
        ("registry as ARN", registry_arn.clone(), {
            draft_record(&router, "registry as ARN", &registry_id, "second", "AGENT").await
        }),
        ("record as ARN", registry_id.clone(), {
            let id = draft_record(&router, "record as ARN", &registry_id, "third", "SKILL").await;
            format!("{registry_arn}/record/{id}")
        }),
    ];
    for (case, registry_label, record_label) in cases {
        let submitted = submit_record(&router, &registry_label, &record_label).await;
        assert_eq!(
            submitted.status, 202,
            "case {case:?}: both identifier forms are accepted, got {} {:?}",
            submitted.status, submitted.body
        );
        assert_eq!(
            submitted.str_field("status"),
            "PENDING_APPROVAL",
            "case {case:?}: expected PENDING_APPROVAL, got {:?}",
            submitted.body
        );
        assert_eq!(
            submitted.str_field("registryArn"),
            registry_arn,
            "case {case:?}: expected registryArn {registry_arn}, got {:?}",
            submitted.body
        );
    }

    // A resource that does not exist is a 404, never a submission against nothing. The
    // ids below satisfy the labels' patterns (12 alphanumerics), so the handler, not the
    // boundary validator, is what refuses them.
    let known_record = id_of(&record_arn).to_string();
    let not_found: Vec<(&str, String, String)> = vec![
        (
            "unknown registry",
            "ffffffffffff".to_string(),
            known_record.clone(),
        ),
        (
            "unknown record",
            registry_id.clone(),
            "ffffffffffff".to_string(),
        ),
        (
            "a record of another registry",
            ready_registry(&router, "another registry", json!({ "name": "other-registry" })).await,
            known_record.clone(),
        ),
    ];
    for (case, registry_label, record_label) in not_found {
        let refused = submit_record(&router, &registry_label, &record_label).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_code
        );
    }
}

#[tokio::test]
async fn a_registry_that_is_not_ready_accepts_no_submission() {
    // The registry is still CREATING, so no record of it can enter the approval
    // workflow; the row below is the only state a registry can be in that holds a
    // record while not being READY, since a record cannot be created before READY.
    let router = router();
    let registry_id = ready_registry(
        &router,
        "deleting registry",
        json!({ "name": "doomed-registry" }),
    )
    .await;
    let record_id = draft_record(&router, "deleting registry", &registry_id, "doomed", "MCP").await;

    let deleted = call(
        &router,
        "DELETE",
        &format!("/registries/{}", encode_label(&registry_id)),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 202,
        "case \"deleting registry\": DeleteRegistry answers 202, got {} {:?}",
        deleted.status, deleted.body
    );

    let refused = submit_record(&router, &registry_id, &record_id).await;
    assert_eq!(
        refused.status, 409,
        "case \"deleting registry\": a registry that is not READY refuses a submission, \
         got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("ConflictException"),
        "case \"deleting registry\": expected ConflictException, got {:?}",
        refused.error_code
    );
    assert!(
        refused.message().contains("DELETING"),
        "case \"deleting registry\": the message must name the registry's status, got {:?}",
        refused.message()
    );
}
