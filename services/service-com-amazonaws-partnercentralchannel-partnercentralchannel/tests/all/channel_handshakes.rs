// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the channel-handshake family: `CreateChannelHandshake` and
//! `ListChannelHandshakes`.
//!
//! The rule under test: a valid `ListChannelHandshakes` call is a SUCCESS (HTTP 200,
//! `application/x-amz-json-1.0`) whose `items` is always serialized — the empty list
//! when the scope and filters match nothing — and which reports exactly the stored
//! handshakes that match, paged by `maxResults`. A list over a catalog names no single
//! resource, so it never answers `ResourceNotFoundException`.
//!
//! Every case drives the service the way a client does: an awsJson1_0 `POST /` with
//! `X-Amz-Target: PartnerCentralChannel.<Operation>`, dispatched through the same
//! [`MultiServiceRouter`] the emulator binary mounts (the dispatch harness mirrors the
//! sibling `relationship_lifecycle` suite's). Nothing reaches into the store directly,
//! so a response that is not computed from persisted state fails here.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralchannel_partnercentralchannel::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A router with one freshly-engined copy of this service mounted.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Deterministic regardless of the process-wide default: these cases are about
    // the service's behaviour, not about SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The region every case signs for.
const DEFAULT_REGION: &str = "us-east-1";

/// The single account every emulated request is made by, and therefore the sender
/// of every handshake this emulator creates. It is the canonical emulator account
/// (`engine::emulator_account_id`), not a literal.
fn caller_account_id() -> String {
    engine::emulator_account_id().to_string()
}

/// Call one operation and return `(http status, content-type, parsed body)`.
async fn call_full(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://partnercentral-channel.{DEFAULT_REGION}.amazonaws.com/"
        ))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("PartnerCentralChannel.{operation}"))
        // The router keys on the SigV4 credential scope's service field
        // (`partnercentral-channel`, this service's signing name) and the service
        // reads the region segment out of the same scope. The signature itself is
        // not checked, because authentication is off above.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLEEXAMPLE/20260101/{DEFAULT_REGION}/\
                 partnercentral-channel/aws4_request, SignedHeaders=host;x-amz-target, \
                 Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(bytes).unwrap_or(Value::Null)
    };
    (status, content_type, parsed)
}

async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let (status, _content_type, parsed) = call_full(router, operation, body).await;
    (status, parsed)
}

/// How a request addresses this service: the routing signals it carries.
/// `resolve_route_key` reads them in a fixed order — a SigV4 credential scope
/// first, then `X-Amz-Target`, then a host-style endpoint — so each variant
/// exercises a different tier of that rule.
#[derive(Debug, Clone, Copy)]
enum Routing {
    /// Signed for the SigV4 signing name (`partnercentral-channel`).
    SignedForSigningName,
    /// Signed with SigV4A (`AWS4-ECDSA-P256-SHA256`), the scheme this service's
    /// model lists FIRST. Its `Authorization` header is not the
    /// `AWS4-HMAC-SHA256` form `service_from_sigv4` reads, so the route key comes
    /// from `X-Amz-Target` — the shape the recorded fixture has.
    SigV4ASigned,
    /// Unsigned, addressed to the emulator's own host: `X-Amz-Target` is the only
    /// signal that names a service.
    TargetOnly,
    /// Unsigned, with a version-suffixed target prefix
    /// (`PartnerCentralChannel_20240318.<Op>`), which the shared reader strips.
    VersionedTargetOnly,
    /// Unsigned, addressed by a host-style endpoint alongside the target.
    HostStyleEndpoint,
}

/// Call `operation` with the routing signals `routing` describes, returning
/// `(http status, content-type, parsed body)`.
async fn call_routed(
    router: &MultiServiceRouter,
    routing: Routing,
    operation: &str,
    body: &Value,
) -> (u16, String, Value) {
    let (uri, target) = match routing {
        Routing::SignedForSigningName | Routing::HostStyleEndpoint => (
            format!("https://partnercentral-channel.{DEFAULT_REGION}.amazonaws.com/"),
            format!("PartnerCentralChannel.{operation}"),
        ),
        Routing::SigV4ASigned | Routing::TargetOnly => (
            "https://127.0.0.1:9297/".to_string(),
            format!("PartnerCentralChannel.{operation}"),
        ),
        Routing::VersionedTargetOnly => (
            "https://127.0.0.1:9297/".to_string(),
            format!("PartnerCentralChannel_20240318.{operation}"),
        ),
    };
    let mut request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", target);
    if matches!(routing, Routing::SignedForSigningName) {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLEEXAMPLE/20260101/{DEFAULT_REGION}/\
                 partnercentral-channel/aws4_request, SignedHeaders=host;x-amz-target, \
                 Signature=0"
            ),
        );
    }
    if matches!(routing, Routing::SigV4ASigned) {
        // A SigV4A credential scope carries `*` as its region set, so the region
        // travels in `X-Amz-Region-Set` instead. The signature itself is not
        // checked, because authentication is off on this router.
        request = request
            .header(
                "authorization",
                "AWS4-ECDSA-P256-SHA256 Credential=AKIAEXAMPLEEXAMPLE/20260101/\
                 partnercentral-channel/aws4_request, SignedHeaders=host;x-amz-target, \
                 Signature=0",
            )
            .header("x-amz-region-set", DEFAULT_REGION);
    }
    if matches!(routing, Routing::HostStyleEndpoint) {
        request = request.header(
            "host",
            format!("partnercentral-channel.{DEFAULT_REGION}.amazonaws.com"),
        );
    }
    let request = request
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(bytes).unwrap_or(Value::Null)
    };
    (status, content_type, parsed)
}

/// Call one operation, requiring success.
async fn call_ok(router: &MultiServiceRouter, operation: &str, body: &Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to succeed with HTTP 200, got {status} and {parsed}"
    );
    parsed
}

/// The awsJson error code an error body carries in `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

fn assert_refused(
    (status, body): (u16, Value),
    expected_status: u16,
    expected_code: &str,
    case: &str,
) {
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} and {body}"
    );
    assert_eq!(
        error_code(&body),
        expected_code,
        "case {case:?}: expected error code {expected_code}, got {body}"
    );
}

fn member<'a>(body: &'a Value, name: &str) -> &'a Value {
    body.get(name).unwrap_or(&Value::Null)
}

fn string_member(body: &Value, name: &str) -> String {
    member(body, name).as_str().unwrap_or_default().to_string()
}

/// The `items` of a list response, as a vector.
fn items(body: &Value) -> Vec<Value> {
    member(body, "items")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `id` of every item of a list response, in the order they came back.
fn listed_ids(body: &Value) -> Vec<String> {
    items(body)
        .iter()
        .map(|item| string_member(item, "id"))
        .collect()
}

/// Is `value` a `ChannelHandshakeId`? The model declares
/// `^ch-[a-z0-9]{13}$` with length exactly 16.
fn is_handshake_id(value: &str) -> bool {
    value.len() == 16
        && value.strip_prefix("ch-").is_some_and(|suffix| {
            suffix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

// ---------------------------------------------------------------------------
// Seeding
// ---------------------------------------------------------------------------

/// Create a program management account and return its `(id, arn)`.
async fn create_account(
    router: &MultiServiceRouter,
    catalog: &str,
    program: &str,
    account_id: &str,
) -> (String, String) {
    let created = call_ok(
        router,
        "CreateProgramManagementAccount",
        &json!({
            "catalog": catalog,
            "program": program,
            "displayName": "Acme PMA",
            "accountId": account_id,
        }),
        account_id,
    )
    .await;
    let detail = member(&created, "programManagementAccountDetail").clone();
    (
        string_member(&detail, "id"),
        string_member(&detail, "arn"),
    )
}

/// Create a relationship under `parent_id` and return its `(id, arn)`.
async fn create_relationship(
    router: &MultiServiceRouter,
    catalog: &str,
    parent_id: &str,
    associated_account_id: &str,
    display_name: &str,
) -> (String, String) {
    let created = call_ok(
        router,
        "CreateRelationship",
        &json!({
            "catalog": catalog,
            "associationType": "END_CUSTOMER",
            "programManagementAccountIdentifier": parent_id,
            "associatedAccountId": associated_account_id,
            "displayName": display_name,
            "sector": "COMMERCIAL",
        }),
        display_name,
    )
    .await;
    let detail = member(&created, "relationshipDetail").clone();
    (
        string_member(&detail, "id"),
        string_member(&detail, "arn"),
    )
}

/// Create a channel handshake and return its `(id, arn)`.
async fn create_handshake(
    router: &MultiServiceRouter,
    catalog: &str,
    handshake_type: &str,
    associated_resource_identifier: &str,
    payload: Option<Value>,
    case: &str,
) -> (String, String) {
    let mut request = json!({
        "catalog": catalog,
        "handshakeType": handshake_type,
        "associatedResourceIdentifier": associated_resource_identifier,
    });
    if let Some(payload) = payload {
        request["payload"] = payload;
    }
    let created = call_ok(router, "CreateChannelHandshake", &request, case).await;
    let detail = member(&created, "channelHandshakeDetail").clone();
    let id = string_member(&detail, "id");
    let arn = string_member(&detail, "arn");
    assert!(
        is_handshake_id(&id),
        "case {case:?}: expected a ^ch-[a-z0-9]{{13}}$ id, got {id:?}"
    );
    assert_eq!(
        arn,
        format!(
            "arn:aws:partnercentral:{DEFAULT_REGION}:{}:catalog/{catalog}/channel-handshake/{id}",
            caller_account_id()
        ),
        "case {case:?}: expected the ARN to be built from the stored id and catalog, got {arn:?}"
    );
    (id, arn)
}

/// The payload a `START_SERVICE_PERIOD` handshake carries. `servicePeriodType` is
/// from the model's `ServicePeriodType` value set
/// (`MINIMUM_NOTICE_PERIOD | FIXED_COMMITMENT_PERIOD`).
fn start_payload(parent_id: &str, service_period_type: &str) -> Value {
    json!({"startServicePeriodPayload": {
        "programManagementAccountIdentifier": parent_id,
        "servicePeriodType": service_period_type,
        "note": "please start",
        "minimumNoticeDays": "30",
    }})
}

/// The payload a `REVOKE_SERVICE_PERIOD` handshake carries.
fn revoke_payload(parent_id: &str) -> Value {
    json!({"revokeServicePeriodPayload": {
        "programManagementAccountIdentifier": parent_id,
        "note": "please revoke",
    }})
}

/// One seeded world: six handshakes spanning both catalogs, all three handshake
/// types, both participant sides and both associated-resource kinds.
struct Seeded {
    router: MultiServiceRouter,
    /// `(label, id)` for each seeded handshake, in creation order.
    handshakes: Vec<(&'static str, String)>,
    /// The program management account whose `accountId` is another account.
    other_account: (String, String),
    /// The relationship whose associated account is another account.
    other_relationship: (String, String),
}

impl Seeded {
    fn id_of(&self, label: &str) -> String {
        self.handshakes
            .iter()
            .find(|(seeded, _)| *seeded == label)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("seeded handshake {label}"))
    }

    fn ids_of(&self, labels: &[&str]) -> Vec<String> {
        labels.iter().map(|label| self.id_of(label)).collect()
    }
}

async fn seeded() -> Seeded {
    let router = emulator();
    // Two accounts in `AWS`: one enrolled for ANOTHER account (so the caller is
    // only the SENDER of its handshake) and one enrolled for the calling account
    // (so the caller is also the RECEIVER).
    let other_account = create_account(&router, "AWS", "SOLUTION_PROVIDER", "222222222222").await;
    let own_account =
        create_account(&router, "AWS", "DISTRIBUTION", &caller_account_id()).await;
    let sandbox_account =
        create_account(&router, "Sandbox", "DISTRIBUTION_SELLER", "444444444444").await;
    // Two relationships: one associated with the calling account, one with another.
    let own_relationship = create_relationship(
        &router,
        "AWS",
        &other_account.0,
        &caller_account_id(),
        "own",
    )
    .await;
    let other_relationship =
        create_relationship(&router, "AWS", &other_account.0, "333333333333", "other").await;

    let mut handshakes = Vec::new();
    for (label, catalog, handshake_type, resource, payload) in [
        (
            "pma-to-other",
            "AWS",
            "PROGRAM_MANAGEMENT_ACCOUNT",
            other_account.0.clone(),
            None,
        ),
        (
            "pma-to-self",
            "AWS",
            "PROGRAM_MANAGEMENT_ACCOUNT",
            own_account.1.clone(),
            None,
        ),
        (
            "start-own",
            "AWS",
            "START_SERVICE_PERIOD",
            own_relationship.0.clone(),
            Some(start_payload(&other_account.0, "MINIMUM_NOTICE_PERIOD")),
        ),
        (
            "start-other",
            "AWS",
            "START_SERVICE_PERIOD",
            other_relationship.1.clone(),
            Some(start_payload(&other_account.0, "FIXED_COMMITMENT_PERIOD")),
        ),
        (
            "revoke-other",
            "AWS",
            "REVOKE_SERVICE_PERIOD",
            other_relationship.0.clone(),
            Some(revoke_payload(&other_account.0)),
        ),
        (
            "pma-sandbox",
            "Sandbox",
            "PROGRAM_MANAGEMENT_ACCOUNT",
            sandbox_account.0.clone(),
            None,
        ),
    ] {
        let (id, _arn) =
            create_handshake(&router, catalog, handshake_type, &resource, payload, label).await;
        handshakes.push((label, id));
    }

    Seeded {
        router,
        handshakes,
        other_account,
        other_relationship,
    }
}

// ---------------------------------------------------------------------------
// The empty page: the class the recorded fixture covers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_routing_signal_that_names_this_service_reaches_it() {
    // A request is claimed by the service its resolved route key names, whichever
    // tier of the routing rule supplied that key. An unclaimed request is answered
    // by the ROUTER — `404 UnknownService` in an awsJson1_1 envelope — so a signal
    // that does not reach the service costs both the status and the protocol.
    let router = emulator();
    let account = create_account(&router, "AWS", "SOLUTION_PROVIDER", "222222222222").await;
    let created = create_handshake(
        &router,
        "AWS",
        "PROGRAM_MANAGEMENT_ACCOUNT",
        &account.0,
        None,
        "routing-seed",
    )
    .await;

    for routing in [
        Routing::SignedForSigningName,
        Routing::SigV4ASigned,
        Routing::TargetOnly,
        Routing::VersionedTargetOnly,
        Routing::HostStyleEndpoint,
    ] {
        let case = format!("{routing:?}");
        let (status, content_type, body) = call_routed(
            &router,
            routing,
            "ListChannelHandshakes",
            &json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "SENDER",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the service to claim and serve the request, got {status} \
             and {body}"
        );
        // awsJson1_0 is the protocol the model binds this service to; the router's
        // own UnknownService envelope is awsJson1_1, so this pins both.
        assert_eq!(
            content_type, "application/x-amz-json-1.0",
            "case {case:?}: expected the awsJson1_0 content-type, got {content_type:?}"
        );
        assert_eq!(
            listed_ids(&body),
            vec![created.0.clone()],
            "case {case:?}: expected the one stored handshake, got {body}"
        );
    }
}

#[tokio::test]
async fn a_list_that_matches_nothing_is_a_200_with_an_empty_items_list() {
    // Every (handshakeType, participantType) pair the model declares, over a store
    // that holds no handshake at all, plus a catalog that holds none. None of these
    // names a single resource, so none of them is a 404.
    let handshake_types = ["PROGRAM_MANAGEMENT_ACCOUNT", "REVOKE_SERVICE_PERIOD",
        "START_SERVICE_PERIOD"];
    let participant_types = ["SENDER", "RECEIVER"];

    let empty = emulator();
    let populated = seeded().await;

    for handshake_type in handshake_types {
        for participant_type in participant_types {
            for (case, router, catalog) in [
                ("empty-store", &empty, "AWS"),
                ("empty-catalog", &populated.router, "Empty"),
            ] {
                let case = format!("{case}/{handshake_type}/{participant_type}/{catalog}");
                let (status, content_type, body) = call_full(
                    router,
                    "ListChannelHandshakes",
                    &json!({
                        "catalog": catalog,
                        "handshakeType": handshake_type,
                        "participantType": participant_type,
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: a list whose filters match nothing is a success, got \
                     {status} and {body}"
                );
                // The model binds this service to awsJson1_0, which fixes the
                // response content-type.
                assert_eq!(
                    content_type, "application/x-amz-json-1.0",
                    "case {case:?}: expected the awsJson1_0 content-type, got {content_type:?}"
                );
                assert!(
                    member(&body, "items").is_array(),
                    "case {case:?}: items must always be serialized, got {body}"
                );
                assert!(
                    items(&body).is_empty(),
                    "case {case:?}: expected no items, got {body}"
                );
                assert!(
                    member(&body, "nextToken").is_null(),
                    "case {case:?}: the last page carries no nextToken, got {body}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The populated page: scope, filters and the summary's own members
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_reports_the_stored_handshakes_its_scope_and_filters_select() {
    let world = seeded().await;

    // Each row names the request under test and which seeded handshakes it must
    // select. An omitted filter constrains nothing; several filters are ANDed.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "sender-side-program-management-account",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "SENDER",
            }),
            vec!["pma-to-other", "pma-to-self"],
        ),
        (
            "receiver-side-selects-only-handshakes-addressed-to-the-caller",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "RECEIVER",
            }),
            vec!["pma-to-self"],
        ),
        (
            "handshake-type-scopes-the-list",
            json!({
                "catalog": "AWS",
                "handshakeType": "START_SERVICE_PERIOD",
                "participantType": "SENDER",
            }),
            vec!["start-own", "start-other"],
        ),
        (
            "revoke-type",
            json!({
                "catalog": "AWS",
                "handshakeType": "REVOKE_SERVICE_PERIOD",
                "participantType": "SENDER",
            }),
            vec!["revoke-other"],
        ),
        (
            "receiver-side-service-period",
            json!({
                "catalog": "AWS",
                "handshakeType": "START_SERVICE_PERIOD",
                "participantType": "RECEIVER",
            }),
            vec!["start-own"],
        ),
        (
            "catalog-scopes-the-list",
            json!({
                "catalog": "Sandbox",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "SENDER",
            }),
            vec!["pma-sandbox"],
        ),
        (
            "statuses-pending-selects-a-fresh-handshake",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "SENDER",
                "statuses": ["PENDING"],
            }),
            vec!["pma-to-other", "pma-to-self"],
        ),
        (
            "statuses-that-select-nothing",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "SENDER",
                "statuses": ["ACCEPTED", "REJECTED", "CANCELED", "EXPIRED"],
            }),
            vec![],
        ),
        (
            "associated-resource-as-a-bare-id",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "SENDER",
                "associatedResourceIdentifiers": [world.other_account.0.clone()],
            }),
            vec!["pma-to-other"],
        ),
        (
            "associated-resource-as-an-arn",
            json!({
                "catalog": "AWS",
                "handshakeType": "START_SERVICE_PERIOD",
                "participantType": "SENDER",
                "associatedResourceIdentifiers": [world.other_relationship.1.clone()],
            }),
            vec!["start-other"],
        ),
        (
            "service-period-type-filter",
            json!({
                "catalog": "AWS",
                "handshakeType": "START_SERVICE_PERIOD",
                "participantType": "SENDER",
                "handshakeTypeFilters": {"startServicePeriodTypeFilters": {
                    "servicePeriodTypes": ["FIXED_COMMITMENT_PERIOD"]
                }},
            }),
            vec!["start-other"],
        ),
        (
            "program-filter",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "SENDER",
                "handshakeTypeFilters": {"programManagementAccountTypeFilters": {
                    "programs": ["DISTRIBUTION"]
                }},
            }),
            vec!["pma-to-self"],
        ),
        (
            "program-filter-that-selects-nothing",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "SENDER",
                "handshakeTypeFilters": {"programManagementAccountTypeFilters": {
                    "programs": ["DISTRIBUTION_SELLER"]
                }},
            }),
            vec![],
        ),
        (
            "several-filters-are-anded",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "SENDER",
                "statuses": ["PENDING"],
                "associatedResourceIdentifiers": [world.other_account.1.clone()],
            }),
            vec!["pma-to-other"],
        ),
    ];

    for (case, request, expected_labels) in cases {
        let listed = call_ok(&world.router, "ListChannelHandshakes", &request, case).await;
        let actual = listed_ids(&listed);
        let expected = world.ids_of(&expected_labels);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected handshakes {expected_labels:?} ({expected:?}), got \
             {actual:?} from {listed}"
        );
        assert!(
            member(&listed, "items").is_array(),
            "case {case:?}: items must always be serialized, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_summary_is_built_from_the_stored_handshake() {
    let world = seeded().await;
    let listed = call_ok(
        &world.router,
        "ListChannelHandshakes",
        &json!({
            "catalog": "AWS",
            "handshakeType": "START_SERVICE_PERIOD",
            "participantType": "SENDER",
            "associatedResourceIdentifiers": [world.other_relationship.0.clone()],
        }),
        "summary",
    )
    .await;
    let listed_items = items(&listed);
    assert_eq!(
        listed_items.len(),
        1,
        "case \"summary\": expected exactly the one matching handshake, got {listed}"
    );
    let summary = &listed_items[0];

    // Every member the summary reports comes from the stored record: the id and
    // ARN the create handed back, the catalog and type the create was made with,
    // the accounts derived from the associated resource, and the detail built from
    // the payload.
    for (field, expected) in [
        ("id", world.id_of("start-other")),
        ("catalog", "AWS".to_string()),
        ("handshakeType", "START_SERVICE_PERIOD".to_string()),
        ("status", "PENDING".to_string()),
        ("ownerAccountId", caller_account_id()),
        ("senderAccountId", caller_account_id()),
        // The receiver is the counterparty the associated relationship names.
        ("receiverAccountId", "333333333333".to_string()),
        ("associatedResourceId", world.other_relationship.0.clone()),
    ] {
        assert_eq!(
            string_member(summary, field),
            expected,
            "case \"summary\": expected {field} to be {expected:?}, got {summary}"
        );
    }
    assert_eq!(
        string_member(summary, "arn"),
        format!(
            "arn:aws:partnercentral:{DEFAULT_REGION}:{}:catalog/AWS/channel-handshake/{}",
            caller_account_id(),
            world.id_of("start-other")
        ),
        "case \"summary\": expected the stored ARN, got {summary}"
    );
    let detail = member(
        member(summary, "detail"),
        "startServicePeriodHandshakeDetail",
    )
    .clone();
    assert_eq!(
        string_member(&detail, "servicePeriodType"),
        "FIXED_COMMITMENT_PERIOD",
        "case \"summary\": the detail reports what the payload proposed, got {detail}"
    );
    assert_eq!(
        string_member(&detail, "note"),
        "please start",
        "case \"summary\": the detail reports the payload's note, got {detail}"
    );
    assert!(
        !string_member(summary, "createdAt").is_empty()
            || member(summary, "createdAt").is_number(),
        "case \"summary\": expected a createdAt timestamp, got {summary}"
    );
    assert_eq!(
        member(summary, "createdAt"),
        member(summary, "updatedAt"),
        "case \"summary\": an untouched handshake's updatedAt equals its createdAt, got {summary}"
    );
    assert!(
        member(summary, "senderDisplayName").is_null(),
        "case \"summary\": this service stores no partner profile, so senderDisplayName is \
         absent rather than invented, got {summary}"
    );

    // A read must not change what a later caller sees.
    let again = call_ok(
        &world.router,
        "ListChannelHandshakes",
        &json!({
            "catalog": "AWS",
            "handshakeType": "START_SERVICE_PERIOD",
            "participantType": "SENDER",
            "associatedResourceIdentifiers": [world.other_relationship.0.clone()],
        }),
        "summary-again",
    )
    .await;
    assert_eq!(
        items(&again),
        listed_items,
        "case \"summary-again\": two reads of the same handshake must report the same record"
    );
}

// ---------------------------------------------------------------------------
// Paging and sorting
// ---------------------------------------------------------------------------

/// Walk every page with `max_results` and return the ids in the order they came
/// back.
async fn page_through(
    router: &MultiServiceRouter,
    max_results: i64,
    sort: Option<Value>,
    case: &str,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut next_token: Option<String> = None;
    // Bounded so a token that never advances fails the assertion below instead of
    // looping forever.
    for _ in 0..64 {
        let mut request = json!({
            "catalog": "AWS",
            "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
            "participantType": "SENDER",
            "maxResults": max_results,
        });
        if let Some(sort) = sort.clone() {
            request["handshakeTypeSort"] = sort;
        }
        if let Some(token) = next_token.clone() {
            request["nextToken"] = json!(token);
        }
        let page = call_ok(router, "ListChannelHandshakes", &request, case).await;
        let page_items = items(&page);
        assert!(
            page_items.len() as i64 <= max_results,
            "case {case:?}: a page must not exceed maxResults {max_results}, got {}",
            page_items.len()
        );
        ids.extend(listed_ids(&page));
        match member(&page, "nextToken").as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => return ids,
        }
    }
    panic!("case {case:?}: paging did not terminate within 64 pages");
}

#[tokio::test]
async fn paging_yields_every_matching_handshake_once() {
    let router = emulator();
    let account = create_account(&router, "AWS", "SOLUTION_PROVIDER", "222222222222").await;
    // Five handshakes on the same resource, so only the page size varies.
    let mut created = Vec::new();
    for index in 0..5 {
        let (id, _arn) = create_handshake(
            &router,
            "AWS",
            "PROGRAM_MANAGEMENT_ACCOUNT",
            &account.0,
            None,
            &format!("seed-{index}"),
        )
        .await;
        created.push(id);
    }

    // Each row is a page size: the published `maxResults` bounds (1 and 100), one
    // in between, and one that exactly divides the record count.
    for max_results in [1, 2, 5, 100] {
        let paged = page_through(&router, max_results, None, &format!("maxResults={max_results}"))
            .await;
        assert_eq!(
            paged, created,
            "case maxResults={max_results}: expected every record once in creation order, got \
             {paged:?} against {created:?}"
        );
    }

    // The default page size the model declares (20) holds all five when the
    // request sends none.
    let defaulted = call_ok(
        &router,
        "ListChannelHandshakes",
        &json!({
            "catalog": "AWS",
            "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
            "participantType": "SENDER",
        }),
        "no-maxResults",
    )
    .await;
    assert_eq!(
        listed_ids(&defaulted),
        created,
        "case \"no-maxResults\": expected the modelled default page size to hold every record, \
         got {defaulted}"
    );

    // Sorting on `updatedAt` — the only `sortBy` the model declares — reverses the
    // creation order when it is `Descending`, and pages the same way.
    for (case, sort_order, expected) in [
        ("Ascending", "Ascending", created.clone()),
        (
            "Descending",
            "Descending",
            created.iter().rev().cloned().collect::<Vec<_>>(),
        ),
    ] {
        let sort = json!({"programManagementAccountTypeSort": {
            "sortOrder": sort_order,
            "sortBy": "UpdatedAt",
        }});
        for max_results in [1, 100] {
            let paged = page_through(&router, max_results, Some(sort.clone()), case).await;
            assert_eq!(
                paged, expected,
                "case {case:?} (maxResults={max_results}): expected {expected:?}, got {paged:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_bad_list_request_is_refused_with_validation_rather_than_served() {
    let world = seeded().await;

    let first = call_ok(
        &world.router,
        "ListChannelHandshakes",
        &json!({
            "catalog": "AWS",
            "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
            "participantType": "SENDER",
            "maxResults": 1,
        }),
        "token-reuse",
    )
    .await;
    let token = string_member(&first, "nextToken");
    assert!(
        !token.is_empty(),
        "case \"token-reuse\": expected a nextToken while a page remains, got {first}"
    );

    for (case, request) in [
        (
            "token-with-different-filters",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "RECEIVER",
                "maxResults": 1,
                "nextToken": token,
            }),
        ),
        (
            "token-we-never-issued",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "SENDER",
                "nextToken": "notatokenweissued",
            }),
        ),
        (
            "associated-resource-of-no-known-kind",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "SENDER",
                "associatedResourceIdentifiers": ["xx-0123456789abc"],
            }),
        ),
        (
            "type-filter-for-another-handshake-type",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "participantType": "SENDER",
                "handshakeTypeFilters": {"startServicePeriodTypeFilters": {
                    "servicePeriodTypes": ["MINIMUM_NOTICE_PERIOD"]
                }},
            }),
        ),
        (
            "type-sort-for-another-handshake-type",
            json!({
                "catalog": "AWS",
                "handshakeType": "START_SERVICE_PERIOD",
                "participantType": "SENDER",
                "handshakeTypeSort": {"programManagementAccountTypeSort": {
                    "sortOrder": "Ascending",
                    "sortBy": "UpdatedAt",
                }},
            }),
        ),
    ] {
        assert_refused(
            call(&world.router, "ListChannelHandshakes", &request).await,
            400,
            "ValidationException",
            case,
        );
    }
}

#[tokio::test]
async fn a_handshake_can_only_be_created_against_a_resource_that_exists() {
    let router = emulator();
    let account = create_account(&router, "AWS", "SOLUTION_PROVIDER", "222222222222").await;
    let other_account = create_account(&router, "AWS", "DISTRIBUTION", "333333333333").await;
    let relationship =
        create_relationship(&router, "AWS", &account.0, "444444444444", "child").await;

    // Each row is a create that must NOT store a handshake: a resource no catalog
    // holds, a resource held in another catalog, a payload that is not the one the
    // handshake type names, a missing payload for a type that requires one, and a
    // payload naming an unknown program management account or one that is not the
    // parent of the named resource.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "unknown-resource",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "associatedResourceIdentifier": "pma-0123456789abc",
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "resource-in-another-catalog",
            json!({
                "catalog": "Sandbox",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "associatedResourceIdentifier": account.0.clone(),
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "payload-for-another-handshake-type",
            json!({
                "catalog": "AWS",
                "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
                "associatedResourceIdentifier": account.0.clone(),
                "payload": revoke_payload(&account.0),
            }),
            400,
            "ValidationException",
        ),
        (
            "missing-payload",
            json!({
                "catalog": "AWS",
                "handshakeType": "START_SERVICE_PERIOD",
                "associatedResourceIdentifier": account.0.clone(),
            }),
            400,
            "ValidationException",
        ),
        (
            "payload-names-an-unknown-account",
            json!({
                "catalog": "AWS",
                "handshakeType": "START_SERVICE_PERIOD",
                "associatedResourceIdentifier": relationship.0.clone(),
                "payload": start_payload("pma-0123456789abc", "MINIMUM_NOTICE_PERIOD"),
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "payload-names-an-account-that-is-not-the-parent",
            json!({
                "catalog": "AWS",
                "handshakeType": "REVOKE_SERVICE_PERIOD",
                "associatedResourceIdentifier": relationship.0.clone(),
                "payload": revoke_payload(&other_account.0),
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, status, code) in cases {
        assert_refused(
            call(&router, "CreateChannelHandshake", &request).await,
            status,
            code,
            case,
        );
    }

    // Nothing was stored by any refused create.
    for handshake_type in [
        "PROGRAM_MANAGEMENT_ACCOUNT",
        "START_SERVICE_PERIOD",
        "REVOKE_SERVICE_PERIOD",
    ] {
        for catalog in ["AWS", "Sandbox"] {
            let listed = call_ok(
                &router,
                "ListChannelHandshakes",
                &json!({
                    "catalog": catalog,
                    "handshakeType": handshake_type,
                    "participantType": "SENDER",
                }),
                handshake_type,
            )
            .await;
            assert!(
                items(&listed).is_empty(),
                "case {handshake_type:?}/{catalog:?}: a refused create must store nothing, got \
                 {listed}"
            );
        }
    }
}

#[tokio::test]
async fn a_client_token_replays_instead_of_creating_a_second_handshake() {
    let router = emulator();
    let account = create_account(&router, "AWS", "SOLUTION_PROVIDER", "222222222222").await;
    let request = json!({
        "catalog": "AWS",
        "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
        "associatedResourceIdentifier": account.0.clone(),
        "clientToken": "token-one",
    });

    let first = call_ok(&router, "CreateChannelHandshake", &request, "first").await;
    let replayed = call_ok(&router, "CreateChannelHandshake", &request, "replay").await;
    assert_eq!(
        member(&first, "channelHandshakeDetail"),
        member(&replayed, "channelHandshakeDetail"),
        "case \"replay\": the same clientToken must replay the first result, got {replayed}"
    );

    let listed = call_ok(
        &router,
        "ListChannelHandshakes",
        &json!({
            "catalog": "AWS",
            "handshakeType": "PROGRAM_MANAGEMENT_ACCOUNT",
            "participantType": "SENDER",
        }),
        "after-replay",
    )
    .await;
    assert_eq!(
        listed_ids(&listed),
        vec![string_member(
            member(&first, "channelHandshakeDetail"),
            "id"
        )],
        "case \"after-replay\": the replay must not have stored a second handshake, got {listed}"
    );

    // The same token with different parameters is a conflict, not a second create.
    let mut different = request.clone();
    different["handshakeType"] = json!("START_SERVICE_PERIOD");
    different["payload"] = start_payload(&account.0, "MINIMUM_NOTICE_PERIOD");
    assert_refused(
        call(&router, "CreateChannelHandshake", &different).await,
        409,
        "ConflictException",
        "token-with-different-parameters",
    );
}
