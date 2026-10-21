// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Voice Connector lifecycle.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, deserialization, the modelled constraints, the
//! handler and the response envelope are all exercised, not just the handler body.
//!
//! The assertions pin RULES over a set of inputs rather than one recorded request:
//! the create/read-back table spans the whole `VoiceConnectorAwsRegion` catalog and both
//! enum catalogs, the pagination test is an invariant check over a grid of store sizes
//! and page sizes, and the refusal table walks each published bound and one value past
//! it.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`chime`, per `OPERATIONS.md`'s `## Wire protocol`), which is what the router's
/// `can_handle` routes on. Request authentication is switched off below, so only the
/// scope's shape matters.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20240101/us-east-1/chime/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The outbound host suffix a Voice Connector's `OutboundHostName` is built from.
const OUTBOUND_SUFFIX: &str = "voiceconnector.chime.aws";

/// Every Region `VoiceConnectorAwsRegion` admits.
/// Source: `smithy_query(query="shape", shape="VoiceConnectorAwsRegion")`.
const AWS_REGIONS: [&str; 10] = [
    "us-east-1",
    "us-west-2",
    "ca-central-1",
    "eu-central-1",
    "eu-west-1",
    "eu-west-2",
    "ap-northeast-2",
    "ap-northeast-1",
    "ap-southeast-1",
    "ap-southeast-2",
];

/// Every value `NetworkType` admits.
/// Source: `smithy_query(query="shape", shape="NetworkType")`.
const NETWORK_TYPES: [&str; 2] = ["IPV4_ONLY", "DUAL_STACK"];

/// Every value `VoiceConnectorIntegrationType` admits.
/// Source: `smithy_query(query="shape", shape="VoiceConnectorIntegrationType")`.
const INTEGRATION_TYPES: [&str; 2] = [
    "CONNECT_CALL_TRANSFER_CONNECTOR",
    "CONNECT_ANALYTICS_CONNECTOR",
];

/// One emulator: a shared engine, the Chime SDK Voice service, and the router that
/// dispatches to it. Authentication is off so a test needs no signing key.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_chimesdkvoice_chimesdktelephonyservice::EmulatedService::new(engine),
    );
    router
}

/// One HTTP answer, kept as raw bytes so a test can assert on an EMPTY body (which a
/// 204 must have) as well as on JSON.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn json(&self, case: &str) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "case {case:?}: expected a JSON body, got {:?} ({e})",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code: restJson1 carries it as the body's `__type`.
    fn error_code(&self, case: &str) -> String {
        self.json(case)["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: error body has no __type: {:?}", self.body))
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let body_bytes = match body {
        Some(value) => Bytes::from(serde_json::to_vec(&value).expect("serializable request body")),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://voice-chime.us-east-1.amazonaws.com{path_and_query}"
        ))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(body_bytes)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    Reply {
        status,
        body: response.into_body(),
    }
}

/// `VoiceConnectorId`'s modelled pattern, `^([a-z0-9]{21,22}|<uuid>)$`.
/// Source: `smithy_query(query="shape", shape="VoiceConnector")`, member `VoiceConnectorId`.
fn is_modelled_connector_id(id: &str) -> bool {
    let short = matches!(id.len(), 21 | 22)
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let uuid = id.len() == 36
        && id.split('-').map(str::len).eq([8, 4, 4, 4, 12])
        && id
            .chars()
            .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c));
    short || uuid
}

/// Seconds since the epoch of an ISO-8601 (`date-time`) timestamp member, or a panic
/// naming the case: `CreatedTimestamp`/`UpdatedTimestamp` target `Iso8601Timestamp`.
fn iso8601_secs(case: &str, member: &str, value: &Value) -> f64 {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {member} is not an ISO-8601 string: {value}"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {member} {text:?} is not ISO-8601: {e}"))
        .as_secs_f64()
}

/// One row of the create/read-back table.
#[derive(Debug, Clone)]
struct CreateCase {
    label: String,
    name: String,
    aws_region: Option<&'static str>,
    require_encryption: bool,
    integration_type: Option<&'static str>,
    network_type: Option<&'static str>,
}

impl CreateCase {
    fn new(label: &str, name: &str) -> Self {
        Self {
            label: label.to_string(),
            name: name.to_string(),
            aws_region: None,
            require_encryption: true,
            integration_type: None,
            network_type: None,
        }
    }

    /// The Region the stored record must carry: the one asked for, else the default the
    /// model documents for `AwsRegion` ("Default: us-east-1").
    fn expected_region(&self) -> &str {
        self.aws_region.unwrap_or("us-east-1")
    }

    fn request_body(&self) -> Value {
        let mut body = json!({
            "Name": self.name,
            "RequireEncryption": self.require_encryption,
        });
        if let Some(region) = self.aws_region {
            body["AwsRegion"] = json!(region);
        }
        if let Some(integration) = self.integration_type {
            body["IntegrationType"] = json!(integration);
        }
        if let Some(network) = self.network_type {
            body["NetworkType"] = json!(network);
        }
        body
    }
}

/// The table: rows differ along name form and length (both published `@length` bounds),
/// Region (the whole catalog, plus the absent-so-defaulted row), the encryption flag,
/// and each enum catalog — the dimensions the create rule generalises over.
fn create_cases() -> Vec<CreateCase> {
    let mut cases = vec![
        CreateCase::new("name at the minimum length 1", "a"),
        CreateCase::new("name at the maximum length 256", &"n".repeat(256)),
        CreateCase::new("name using every allowed punctuation", "vc 1_2.3-4"),
        CreateCase::new("name of digits only", "0123456789"),
    ];
    // The encryption flag is written, not assumed: one row for each value.
    let mut unencrypted = CreateCase::new("encryption disabled", "plain-connector");
    unencrypted.require_encryption = false;
    cases.push(unencrypted);

    // The absent-Region row is the one that exercises the documented default; the rest
    // of the catalog is named explicitly.
    for region in AWS_REGIONS {
        let mut case = CreateCase::new(&format!("region {region}"), "regional-connector");
        case.aws_region = Some(region);
        cases.push(case);
    }
    for network in NETWORK_TYPES {
        let mut case = CreateCase::new(&format!("network type {network}"), "networked-connector");
        case.network_type = Some(network);
        cases.push(case);
    }
    for integration in INTEGRATION_TYPES {
        let mut case =
            CreateCase::new(&format!("integration type {integration}"), "integrated-vc");
        case.integration_type = Some(integration);
        cases.push(case);
    }
    cases
}

/// RULE: a create persists the resource and mints its identity; every operation that
/// reports it afterwards reports exactly what was stored, with the id-derived members
/// derived from the minted id and the Region defaulted when the request named none.
#[tokio::test]
async fn create_persists_every_member_and_get_reports_it_back() {
    let router = emulator();
    let mut minted: HashSet<String> = HashSet::new();

    for case in create_cases() {
        let label = case.label.clone();
        let created = call(
            &router,
            "POST",
            "/voice-connectors",
            Some(case.request_body()),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected HTTP 201, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let body = created.json(&label);
        let connector = body["VoiceConnector"].clone();
        assert!(
            connector.is_object(),
            "case {label:?}: expected a VoiceConnector object, got {body}"
        );

        let id = connector["VoiceConnectorId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no VoiceConnectorId in {connector}"))
            .to_string();
        assert!(
            is_modelled_connector_id(&id),
            "case {label:?}: expected an id matching ^([a-z0-9]{{21,22}}|<uuid>)$, got {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {label:?}: expected a fresh id, got the already-minted {id:?}"
        );

        assert_eq!(
            connector["Name"],
            json!(case.name),
            "case {label:?}: expected the submitted Name, got {}",
            connector["Name"]
        );
        assert_eq!(
            connector["AwsRegion"],
            json!(case.expected_region()),
            "case {label:?}: expected AwsRegion {:?}, got {}",
            case.expected_region(),
            connector["AwsRegion"]
        );
        assert_eq!(
            connector["RequireEncryption"],
            json!(case.require_encryption),
            "case {label:?}: expected RequireEncryption {}, got {}",
            case.require_encryption,
            connector["RequireEncryption"]
        );
        assert_eq!(
            connector["OutboundHostName"],
            json!(format!("{id}.{OUTBOUND_SUFFIX}")),
            "case {label:?}: expected the outbound host name derived from the id, got {}",
            connector["OutboundHostName"]
        );
        assert_eq!(
            connector["VoiceConnectorArn"],
            json!(format!(
                "arn:aws:chime:{}:{}:vc/{id}",
                case.expected_region(),
                engine::emulator_account_id()
            )),
            "case {label:?}: expected the canonical Voice Connector ARN, got {}",
            connector["VoiceConnectorArn"]
        );
        assert_eq!(
            connector["IntegrationType"],
            case.integration_type.map_or(Value::Null, |v| json!(v)),
            "case {label:?}: expected IntegrationType {:?}, got {}",
            case.integration_type,
            connector["IntegrationType"]
        );
        assert_eq!(
            connector["NetworkType"],
            case.network_type.map_or(Value::Null, |v| json!(v)),
            "case {label:?}: expected NetworkType {:?}, got {}",
            case.network_type,
            connector["NetworkType"]
        );

        let created_at = iso8601_secs(&label, "CreatedTimestamp", &connector["CreatedTimestamp"]);
        let updated_at = iso8601_secs(&label, "UpdatedTimestamp", &connector["UpdatedTimestamp"]);
        assert!(
            updated_at >= created_at,
            "case {label:?}: expected UpdatedTimestamp >= CreatedTimestamp, got {updated_at} < {created_at}"
        );

        // ROUND TRIP: reading the resource back reports the identical record, and does
        // so repeatedly — a read must not change what a later caller sees.
        for attempt in 1..=2 {
            let fetched = call(&router, "GET", &format!("/voice-connectors/{id}"), None).await;
            assert_eq!(
                fetched.status, 200,
                "case {label:?}: read {attempt} expected HTTP 200, got {} with body {:?}",
                fetched.status,
                String::from_utf8_lossy(&fetched.body)
            );
            assert_eq!(
                fetched.json(&label)["VoiceConnector"],
                connector,
                "case {label:?}: read {attempt} reported a different record than was created"
            );
        }
    }
}

/// Create `count` connectors and answer their ids.
async fn seed(router: &MultiServiceRouter, count: usize, label: &str) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for n in 0..count {
        let reply = call(
            router,
            "POST",
            "/voice-connectors",
            Some(json!({ "Name": format!("seed-{n}"), "RequireEncryption": n % 2 == 0 })),
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "case {label:?}: seeding connector {n} expected HTTP 201, got {}",
            reply.status
        );
        let id = reply.json(label)["VoiceConnector"]["VoiceConnectorId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: seeded connector {n} has no id"))
            .to_string();
        ids.insert(id);
    }
    ids
}

/// RULE: a paged listing yields every stored resource exactly once and terminates — each
/// page is no longer than the requested page size, and the last page is the one that
/// returns no token.
#[tokio::test]
async fn a_paged_listing_yields_every_connector_exactly_once() {
    // The grid: store sizes around and above the page sizes, and page sizes at the
    // published `MaxResults` bounds (1 and 100) plus values in between.
    for store_size in [0_usize, 1, 2, 5, 7] {
        for page_size in [1_usize, 2, 3, 100] {
            let label = format!("{store_size} connectors paged by {page_size}");
            let router = emulator();
            let expected = seed(&router, store_size, &label).await;

            let mut seen: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0;
            loop {
                pages += 1;
                assert!(
                    pages <= store_size + 2,
                    "case {label:?}: the paged walk did not terminate after {pages} pages"
                );
                let mut query = format!("?max-results={page_size}");
                if let Some(token) = next_token.as_deref() {
                    query.push_str(&format!("&next-token={token}"));
                }
                let reply = call(&router, "GET", &format!("/voice-connectors{query}"), None).await;
                assert_eq!(
                    reply.status, 200,
                    "case {label:?}: expected HTTP 200, got {} with body {:?}",
                    reply.status,
                    String::from_utf8_lossy(&reply.body)
                );
                let body = reply.json(&label);
                let page = body["VoiceConnectors"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {label:?}: VoiceConnectors is not a list: {body}"))
                    .clone();
                assert!(
                    page.len() <= page_size,
                    "case {label:?}: expected at most {page_size} per page, got {}",
                    page.len()
                );
                for connector in &page {
                    let id = connector["VoiceConnectorId"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {label:?}: a listed connector has no id: {connector}")
                        })
                        .to_string();
                    // A listing reports the stored record, not a stub of it.
                    assert_eq!(
                        connector["OutboundHostName"],
                        json!(format!("{id}.{OUTBOUND_SUFFIX}")),
                        "case {label:?}: listed {id} carries the wrong OutboundHostName"
                    );
                    seen.push(id);
                }
                next_token = body["NextToken"].as_str().map(str::to_string);
                if next_token.is_none() {
                    break;
                }
            }

            let unique: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                seen.len(),
                "case {label:?}: the walk reported {} connectors but only {} distinct ones",
                seen.len(),
                unique.len()
            );
            assert_eq!(
                unique, expected,
                "case {label:?}: the walk reported {unique:?}, expected {expected:?}"
            );
        }
    }
}

/// RULE: an update writes the submitted members into the stored record, refreshes its
/// `UpdatedTimestamp`, leaves its identity and creation facts alone, and every later read
/// reports the new values.
#[tokio::test]
async fn an_update_is_written_through_and_read_back() {
    let router = emulator();

    // Rows differ along the new name's form and length and along the encryption flag,
    // including the no-op flip — the dimensions the update rule generalises over.
    let updates: [(&str, String, bool); 5] = [
        ("rename and disable encryption", "renamed-1".to_string(), false),
        ("rename to the minimum length", "z".to_string(), true),
        ("rename to the maximum length", "u".repeat(256), false),
        ("rename with punctuation", "re named_1.2-3".to_string(), true),
        ("same name, flag unchanged", "stable-name".to_string(), true),
    ];

    for (label, new_name, require_encryption) in updates {
        let created = call(
            &router,
            "POST",
            "/voice-connectors",
            Some(json!({ "Name": "stable-name", "RequireEncryption": true, "AwsRegion": "eu-west-1" })),
        )
        .await;
        assert_eq!(created.status, 201, "case {label:?}: seeding failed");
        let before = created.json(label)["VoiceConnector"].clone();
        let id = before["VoiceConnectorId"].as_str().expect("an id").to_string();

        let updated = call(
            &router,
            "PUT",
            &format!("/voice-connectors/{id}"),
            Some(json!({ "Name": new_name, "RequireEncryption": require_encryption })),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200, got {} with body {:?}",
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );
        let after = updated.json(label)["VoiceConnector"].clone();

        assert_eq!(
            after["Name"],
            json!(new_name),
            "case {label:?}: expected Name {new_name:?}, got {}",
            after["Name"]
        );
        assert_eq!(
            after["RequireEncryption"],
            json!(require_encryption),
            "case {label:?}: expected RequireEncryption {require_encryption}, got {}",
            after["RequireEncryption"]
        );
        for untouched in [
            "VoiceConnectorId",
            "AwsRegion",
            "OutboundHostName",
            "VoiceConnectorArn",
            "CreatedTimestamp",
        ] {
            assert_eq!(
                after[untouched], before[untouched],
                "case {label:?}: expected {untouched} to survive the update as {}, got {}",
                before[untouched], after[untouched]
            );
        }
        let created_at = iso8601_secs(label, "CreatedTimestamp", &after["CreatedTimestamp"]);
        let updated_at = iso8601_secs(label, "UpdatedTimestamp", &after["UpdatedTimestamp"]);
        let previous = iso8601_secs(label, "UpdatedTimestamp", &before["UpdatedTimestamp"]);
        assert!(
            updated_at >= created_at && updated_at >= previous,
            "case {label:?}: expected a refreshed UpdatedTimestamp >= {previous} and >= {created_at}, got {updated_at}"
        );

        // The write is in state, not just in the answer.
        let fetched = call(&router, "GET", &format!("/voice-connectors/{id}"), None).await;
        assert_eq!(fetched.status, 200, "case {label:?}: the read-back failed");
        assert_eq!(
            fetched.json(label)["VoiceConnector"], after,
            "case {label:?}: the read-back does not match the update's answer"
        );
    }
}

/// RULE: a delete removes the resource — it answers 204 with no body, and every later
/// read of that id reports it as absent.
#[tokio::test]
async fn a_delete_removes_the_connector_from_every_later_read() {
    // Rows differ along how many connectors share the store with the deleted one, and
    // along which of them is deleted.
    for (store_size, victim) in [(1_usize, 0_usize), (3, 0), (3, 1), (3, 2)] {
        let label = format!("delete connector {victim} of {store_size}");
        let router = emulator();
        let ids: Vec<String> = seed(&router, store_size, &label).await.into_iter().collect();
        let doomed = ids[victim].clone();

        let deleted = call(&router, "DELETE", &format!("/voice-connectors/{doomed}"), None).await;
        assert_eq!(
            deleted.status, 204,
            "case {label:?}: expected HTTP 204, got {} with body {:?}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
        assert!(
            deleted.body.is_empty(),
            "case {label:?}: expected an empty 204 body, got {:?}",
            String::from_utf8_lossy(&deleted.body)
        );

        let fetched = call(&router, "GET", &format!("/voice-connectors/{doomed}"), None).await;
        assert_eq!(
            fetched.status, 404,
            "case {label:?}: expected HTTP 404 after the delete, got {}",
            fetched.status
        );
        assert_eq!(
            fetched.error_code(&label),
            "NotFoundException",
            "case {label:?}: expected NotFoundException, got {}",
            fetched.error_code(&label)
        );

        // Deleting it again is a 404 too: the record is gone, not emptied.
        let again = call(&router, "DELETE", &format!("/voice-connectors/{doomed}"), None).await;
        assert_eq!(
            again.status, 404,
            "case {label:?}: expected HTTP 404 on the second delete, got {}",
            again.status
        );

        let listed = call(&router, "GET", "/voice-connectors", None).await;
        assert_eq!(listed.status, 200, "case {label:?}: the listing failed");
        let remaining: BTreeSet<String> = listed.json(&label)["VoiceConnectors"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|c| c["VoiceConnectorId"].as_str().expect("an id").to_string())
            .collect();
        let expected: BTreeSet<String> = ids
            .iter()
            .filter(|id| **id != doomed)
            .cloned()
            .collect();
        assert_eq!(
            remaining, expected,
            "case {label:?}: expected the listing to hold {expected:?}, got {remaining:?}"
        );
    }
}

/// RULE: a well-formed id that names no stored connector is reported as absent (404
/// `NotFoundException`) by every operation that resolves one.
#[tokio::test]
async fn a_well_formed_but_unknown_id_is_not_found() {
    let router = emulator();
    // Both alternatives of the id pattern, so the refusal is not keyed to one spelling.
    let unknown_ids = [
        "abcdef0123456789abcde",
        "abcdef0123456789abcdef",
        "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d",
    ];

    for id in unknown_ids {
        for (method, path, body) in [
            ("GET", format!("/voice-connectors/{id}"), None),
            (
                "PUT",
                format!("/voice-connectors/{id}"),
                Some(json!({ "Name": "nobody", "RequireEncryption": true })),
            ),
            ("DELETE", format!("/voice-connectors/{id}"), None),
        ] {
            let label = format!("{method} {path}");
            let reply = call(&router, method, &path, body).await;
            assert_eq!(
                reply.status, 404,
                "case {label:?}: expected HTTP 404, got {} with body {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(&label),
                "NotFoundException",
                "case {label:?}: expected NotFoundException, got {}",
                reply.error_code(&label)
            );
        }
    }
}

/// RULE: the store is bounded — once it holds the maximum number of Voice Connectors, a
/// further create is refused with the error the model declares for a quota, and the store
/// is left exactly as it was.
#[tokio::test]
async fn the_store_is_bounded_and_the_ceiling_is_a_resource_limit() {
    let router = emulator();
    let limit = service_com_amazonaws_chimesdkvoice_chimesdktelephonyservice::state::MAX_VOICE_CONNECTORS;

    for n in 0..limit {
        let reply = call(
            &router,
            "POST",
            "/voice-connectors",
            Some(json!({ "Name": format!("bounded-{n}"), "RequireEncryption": true })),
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "case \"connector {n} of {limit}\": expected HTTP 201 below the ceiling, got {}",
            reply.status
        );
    }

    // One past the ceiling.
    let refused = call(
        &router,
        "POST",
        "/voice-connectors",
        Some(json!({ "Name": "one-too-many", "RequireEncryption": true })),
    )
    .await;
    let label = format!("create number {} of {limit}", limit + 1);
    assert_eq!(
        refused.status, 400,
        "case {label:?}: expected HTTP 400 at the ceiling, got {} with body {:?}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    assert_eq!(
        refused.error_code(&label),
        "ResourceLimitExceededException",
        "case {label:?}: expected ResourceLimitExceededException, got {}",
        refused.error_code(&label)
    );

    // The refusal changed nothing: the store still holds exactly the ceiling.
    let mut seen = 0_usize;
    let mut next_token: Option<String> = None;
    loop {
        let mut query = "?max-results=100".to_string();
        if let Some(token) = next_token.as_deref() {
            query.push_str(&format!("&next-token={token}"));
        }
        let page = call(&router, "GET", &format!("/voice-connectors{query}"), None).await;
        assert_eq!(page.status, 200, "case {label:?}: the listing failed");
        let body = page.json(&label);
        seen += body["VoiceConnectors"].as_array().expect("a list").len();
        next_token = body["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, limit,
        "case {label:?}: expected the store to still hold {limit} connectors, got {seen}"
    );
}

/// RULE: a request that breaks a declared input rule is refused as
/// `BadRequestException` (HTTP 400) — never a 501, never a silent success — while a value
/// ON a published bound is accepted.
#[tokio::test]
async fn input_outside_a_declared_rule_is_a_bad_request() {
    let router = emulator();
    let known = call(
        &router,
        "POST",
        "/voice-connectors",
        Some(json!({ "Name": "bounds-probe", "RequireEncryption": true })),
    )
    .await;
    assert_eq!(known.status, 201, "seeding the bounds probe failed");
    let known_id = known.json("seed")["VoiceConnector"]["VoiceConnectorId"]
        .as_str()
        .expect("an id")
        .to_string();

    // Each row breaks exactly one declared rule: `Name`'s @length (both bounds) and
    // @pattern, `RequireEncryption`'s @required, `AwsRegion`'s enum value set,
    // `MaxResults`'s @range (both bounds), and `VoiceConnectorId`'s @pattern.
    let refused: Vec<(String, &str, String, Option<Value>)> = vec![
        (
            "Name one below the minimum length".to_string(),
            "POST",
            "/voice-connectors".to_string(),
            Some(json!({ "Name": "", "RequireEncryption": true })),
        ),
        (
            "Name one above the maximum length".to_string(),
            "POST",
            "/voice-connectors".to_string(),
            Some(json!({ "Name": "n".repeat(257), "RequireEncryption": true })),
        ),
        (
            "Name outside the pattern".to_string(),
            "POST",
            "/voice-connectors".to_string(),
            Some(json!({ "Name": "bad name!!", "RequireEncryption": true })),
        ),
        (
            "Name absent".to_string(),
            "POST",
            "/voice-connectors".to_string(),
            Some(json!({ "RequireEncryption": true })),
        ),
        (
            "RequireEncryption absent".to_string(),
            "POST",
            "/voice-connectors".to_string(),
            Some(json!({ "Name": "no-flag" })),
        ),
        (
            "AwsRegion outside the enum".to_string(),
            "POST",
            "/voice-connectors".to_string(),
            Some(json!({ "Name": "nowhere", "RequireEncryption": true, "AwsRegion": "mars-east-1" })),
        ),
        (
            "MaxResults one below the minimum".to_string(),
            "GET",
            "/voice-connectors?max-results=0".to_string(),
            None,
        ),
        (
            "MaxResults one above the maximum".to_string(),
            "GET",
            "/voice-connectors?max-results=101".to_string(),
            None,
        ),
        (
            "VoiceConnectorId outside the pattern on a read".to_string(),
            "GET",
            "/voice-connectors/NOT-AN-ID".to_string(),
            None,
        ),
        (
            "VoiceConnectorId outside the pattern on an update".to_string(),
            "PUT",
            "/voice-connectors/NOT-AN-ID".to_string(),
            Some(json!({ "Name": "whoever", "RequireEncryption": true })),
        ),
        (
            "VoiceConnectorId outside the pattern on a delete".to_string(),
            "DELETE",
            "/voice-connectors/NOT-AN-ID".to_string(),
            None,
        ),
        (
            "Name outside the pattern on an update".to_string(),
            "PUT",
            format!("/voice-connectors/{known_id}"),
            Some(json!({ "Name": "bad name!!", "RequireEncryption": true })),
        ),
        (
            "NextToken this service never issued".to_string(),
            "GET",
            "/voice-connectors?next-token=not-a-token".to_string(),
            None,
        ),
    ];

    for (label, method, path, body) in refused {
        let reply = call(&router, method, &path, body).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(&label),
            "BadRequestException",
            "case {label:?}: expected BadRequestException, got {}",
            reply.error_code(&label)
        );
    }

    // ... and the published bounds themselves are ACCEPTED, so the refusals above are
    // the rule's edge and not a blanket rejection.
    let accepted: Vec<(String, &str, String, Option<Value>)> = vec![
        (
            "MaxResults at the minimum".to_string(),
            "GET",
            "/voice-connectors?max-results=1".to_string(),
            None,
        ),
        (
            "MaxResults at the maximum".to_string(),
            "GET",
            "/voice-connectors?max-results=100".to_string(),
            None,
        ),
        (
            "no MaxResults at all".to_string(),
            "GET",
            "/voice-connectors".to_string(),
            None,
        ),
    ];
    for (label, method, path, body) in accepted {
        let reply = call(&router, method, &path, body).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected HTTP 200, got {} with body {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
}
