// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the Interconnect Environment catalog and the Connection
//! lifecycle, driven over the wire the way a client drives it: `POST /` with
//! `X-Amz-Target: Interconnect.<Operation>` on `awsJson1_0`, through the same
//! `MultiServiceRouter::dispatch` the socket front door uses.
//!
//! Every assertion here is about a RULE, checked over a table of inputs built from
//! the live seeded catalog rather than from a copy of it: a row differs from its
//! neighbours in the environment, the provider kind, the bandwidth, the attach-point
//! union member, whether the optional members are present, and the region the request
//! is signed for.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The signing name the model gives this service; also its ARN namespace.
const SIGNING_NAME: &str = "interconnect";
/// The account id every emulator ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// Long enough to cover the handler's own provisioning span (400 ms) with margin;
/// the transition is due at a stored instant, so any wait past the span settles it.
const SETTLE_WAIT: Duration = Duration::from_millis(700);

/// One emulator: a router with only this service mounted, over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pinned off so the suite does not depend on the process-wide default; the
        // Authorization header below is still what carries the credential scope.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_interconnect_interconnect::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, signed for `region`. Returns the status and the
    /// decoded JSON body.
    async fn call_in(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{SIGNING_NAME}/aws4_request, \
             SignedHeaders=host;x-amz-date;x-amz-target, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "interconnect.localhost")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("Interconnect.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let decoded = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, decoded)
    }

    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call_in("us-east-1", operation, body).await
    }

    /// Call `operation` and require a 200, reporting the body when it is not.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, decoded) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with body {decoded:?}"
        );
        decoded
    }

    /// Every seeded Environment, as `ListEnvironments` reports it.
    async fn environments(&self) -> Vec<Value> {
        let listed = self.ok("ListEnvironments", json!({})).await;
        listed["environments"]
            .as_array()
            .expect("environments is a list")
            .clone()
    }
}

// ---------------------------------------------------------------------------
// Shape checks, written from the model's own constraints
// ---------------------------------------------------------------------------

/// `ConnectionId`: `^(mcc|lmcc)-[a-z0-9]{8}$`.
fn is_connection_id(value: &str) -> bool {
    let Some(suffix) = value
        .strip_prefix("mcc-")
        .or_else(|| value.strip_prefix("lmcc-"))
    else {
        return false;
    };
    suffix.len() == 8
        && suffix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// `ConnectionSharedId`: `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| group.len() == *len)
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// `ConnectionBandwidth`: `^\d+[MG]bps$`, length 1..8.
fn is_bandwidth(value: &str) -> bool {
    let Some(digits) = value
        .strip_suffix("Mbps")
        .or_else(|| value.strip_suffix("Gbps"))
    else {
        return false;
    };
    !value.is_empty()
        && value.len() <= 8
        && !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
}

fn text(value: &Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("member '{member}' is a string in {value:?}"))
        .to_string()
}

fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The bandwidths an Environment currently offers.
fn available_bandwidths(environment: &Value) -> Vec<String> {
    string_list(&environment["bandwidths"]["available"])
}

/// The bandwidths an Environment plans to support, which is a superset of the above.
fn supported_bandwidths(environment: &Value) -> Vec<String> {
    string_list(&environment["bandwidths"]["supported"])
}

/// The `ConnectionId` prefix an Environment's provider kind dictates: a
/// `cloudServiceProvider` environment yields Multicloud connections (`mcc`), a
/// `lastMileProvider` environment LastMile ones (`lmcc`).
fn expected_id_prefix(environment: &Value) -> &'static str {
    if environment["provider"]["cloudServiceProvider"].is_string() {
        "mcc"
    } else {
        "lmcc"
    }
}

/// An attach point naming a Direct Connect gateway by UUID.
fn dxgw_attach_point(seed: u8) -> Value {
    json!({ "directConnectGateway": format!("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee{seed:02}") })
}

/// An attach point naming another connection by full ARN (length 59..150).
fn arn_attach_point(seed: u8) -> Value {
    json!({
        "arn": format!(
            "arn:aws:interconnect:us-east-1:{ACCOUNT_ID}:connection/mcc-aaaaaa{seed:02}"
        )
    })
}

// ---------------------------------------------------------------------------
// Environments
// ---------------------------------------------------------------------------

/// The catalog is seeded, every record satisfies the model, and both list filters
/// select exactly the subset of the full listing that matches them.
#[tokio::test]
async fn environment_catalog_is_seeded_and_every_filter_selects_its_subset() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;

    assert!(
        !catalog.is_empty(),
        "ListEnvironments with no filters: expected a non-empty seeded catalog, got {catalog:?}"
    );

    // Every member `Environment` marks @required is present and within its
    // modelled constraints; `available` is a subset of `supported`, which is what
    // the `limited` state means.
    for environment in &catalog {
        let id = text(environment, "environmentId");
        let state = text(environment, "state");
        let product_type = text(environment, "type");
        assert!(
            (1..=64).contains(&id.len()),
            "environment {id}: expected environmentId length 1..64, got {}",
            id.len()
        );
        assert!(
            ["available", "limited", "unavailable"].contains(&state.as_str()),
            "environment {id}: expected an EnvironmentState value, got {state:?}"
        );
        assert!(
            (5..=32).contains(&product_type.len()),
            "environment {id}: expected ProductType length 5..32, got {product_type:?}"
        );
        assert!(
            environment["location"].is_string(),
            "environment {id}: expected a location, got {environment:?}"
        );
        assert!(
            environment["provider"]["cloudServiceProvider"].is_string()
                ^ environment["provider"]["lastMileProvider"].is_string(),
            "environment {id}: expected exactly one Provider member, got {:?}",
            environment["provider"]
        );
        let supported = supported_bandwidths(environment);
        for bandwidth in available_bandwidths(environment) {
            assert!(
                is_bandwidth(&bandwidth),
                "environment {id}: expected a ConnectionBandwidth, got {bandwidth:?}"
            );
            assert!(
                supported.contains(&bandwidth),
                "environment {id}: expected available bandwidth {bandwidth} to also be supported, got supported {supported:?}"
            );
        }
    }

    // The filter table: every provider in the catalog, every location in the
    // catalog, one of each that matches nothing, and both together.
    let mut cases: Vec<(String, Value)> = Vec::new();
    for environment in &catalog {
        cases.push((
            format!("provider {:?}", environment["provider"]),
            json!({ "provider": environment["provider"] }),
        ));
        cases.push((
            format!("location {:?}", environment["location"]),
            json!({ "location": environment["location"] }),
        ));
    }
    cases.push((
        "provider and location together".to_string(),
        json!({ "provider": catalog[0]["provider"], "location": catalog[0]["location"] }),
    ));
    cases.push((
        "a provider no environment has".to_string(),
        json!({ "provider": { "cloudServiceProvider": "NoSuchPartner" } }),
    ));
    cases.push((
        "a location no environment has".to_string(),
        json!({ "location": "nowhere-0" }),
    ));
    cases.push((
        "a provider of the wrong union member".to_string(),
        json!({ "provider": { "lastMileProvider": catalog[0]["provider"]["cloudServiceProvider"] } }),
    ));

    for (case, filter) in cases {
        let expected: Vec<Value> = catalog
            .iter()
            .filter(|environment| {
                filter
                    .get("provider")
                    .is_none_or(|wanted| environment["provider"] == *wanted)
                    && filter
                        .get("location")
                        .is_none_or(|wanted| environment["location"] == *wanted)
            })
            .cloned()
            .collect();
        let listed = emulator.ok("ListEnvironments", filter.clone()).await;
        let actual = listed["environments"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the {} matching environments, got {actual:?}",
            expected.len()
        );
    }
}

/// A paged walk visits the catalog exactly once, in the unpaged order, for every
/// page size — including a page size of one and one larger than the catalog.
#[tokio::test]
async fn environment_pages_walk_the_catalog_without_repeats_or_gaps() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;

    for page_size in 1..=(catalog.len() + 1) {
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = Map::new();
            request.insert("maxResults".to_string(), json!(page_size));
            if let Some(token) = &token {
                request.insert("nextToken".to_string(), json!(token));
            }
            let listed = emulator
                .ok("ListEnvironments", Value::Object(request))
                .await;
            let page = listed["environments"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "page size {page_size}: expected at most {page_size} environments per page, got {}",
                page.len()
            );
            walked.extend(page);
            token = listed["nextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= catalog.len() + 2,
                "page size {page_size}: expected the walk to terminate, got {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, catalog,
            "page size {page_size}: expected the paged walk to equal the unpaged catalog, got {walked:?}"
        );
    }

    // A cursor this operation did not issue is refused, not answered with an
    // empty page: a hand-made token, a foreign one, and one of the other list
    // operation's.
    for token in [
        "not-a-cursor",
        "env-csp-azure-us-east-1",
        "conn:mcc-00000000",
        "env:",
    ] {
        let (status, body) = emulator
            .call("ListEnvironments", json!({ "nextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400 for a cursor ListEnvironments never issued, got {status} with {body:?}"
        );
        assert!(
            text(&body, "__type").contains("InterconnectValidationException"),
            "case {token:?}: expected InterconnectValidationException, got {body:?}"
        );
    }
}

/// `GetEnvironment` answers from the same store, so it returns the very record
/// `ListEnvironments` returned for that id — and nothing for an id it never listed.
#[tokio::test]
async fn get_environment_returns_the_record_the_list_returned() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;

    for expected in &catalog {
        let id = text(expected, "environmentId");
        let fetched = emulator.ok("GetEnvironment", json!({ "id": id })).await;
        assert_eq!(
            &fetched["environment"], expected,
            "case {id:?}: expected GetEnvironment to return the listed record, got {:?}",
            fetched["environment"]
        );
    }

    for case in ["env-does-not-exist", "0", &"e".repeat(64)] {
        let (status, body) = emulator.call("GetEnvironment", json!({ "id": case })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an unknown environment, got {status} with {body:?}"
        );
        assert!(
            text(&body, "__type").contains("InterconnectClientException"),
            "case {case:?}: expected InterconnectClientException, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// CreateConnection / GetConnection
// ---------------------------------------------------------------------------

/// One row of the create table.
#[derive(Debug, Clone)]
struct CreateCase {
    environment_id: String,
    bandwidth: String,
    region: &'static str,
    attach_point: Value,
    description: Option<String>,
    tags: Option<Value>,
}

impl CreateCase {
    fn request(&self) -> Value {
        let mut request = Map::new();
        request.insert("environmentId".to_string(), json!(self.environment_id));
        request.insert("bandwidth".to_string(), json!(self.bandwidth));
        request.insert("attachPoint".to_string(), self.attach_point.clone());
        if let Some(description) = &self.description {
            request.insert("description".to_string(), json!(description));
        }
        if let Some(tags) = &self.tags {
            request.insert("tags".to_string(), tags.clone());
        }
        Value::Object(request)
    }
}

/// The create table: every bandwidth of every environment that can take a
/// connection, alternating the attach-point union member, the presence of the two
/// optional members, and the region the request is signed for.
fn create_table(catalog: &[Value]) -> Vec<(Value, CreateCase)> {
    let mut rows = Vec::new();
    let mut seed = 0u8;
    for environment in catalog {
        if text(environment, "state") == "unavailable" {
            continue;
        }
        for bandwidth in available_bandwidths(environment) {
            seed += 1;
            rows.push((
                environment.clone(),
                CreateCase {
                    environment_id: text(environment, "environmentId"),
                    bandwidth,
                    region: if seed.is_multiple_of(2) {
                        "eu-west-1"
                    } else {
                        "us-east-1"
                    },
                    attach_point: if seed.is_multiple_of(2) {
                        arn_attach_point(seed)
                    } else {
                        dxgw_attach_point(seed)
                    },
                    description: if seed.is_multiple_of(3) {
                        None
                    } else {
                        Some(format!("link {seed}"))
                    },
                    tags: match seed % 3 {
                        0 => None,
                        1 => Some(json!({ "env": "test" })),
                        _ => Some(json!({ "env": "test", "team": "network" })),
                    },
                },
            ));
        }
    }
    rows
}

/// Every server-owned member of a created Connection is computed from the server
/// and the referenced Environment, never echoed from the request — and the stored
/// record is what `GetConnection` reads back.
#[tokio::test]
async fn create_connection_computes_every_server_owned_member_and_persists_it() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let table = create_table(&catalog);
    assert!(
        table.len() >= 4,
        "the create table must span several environments and bandwidths, got {} rows",
        table.len()
    );

    let mut ids = BTreeSet::new();

    for (environment, case) in table {
        let (status, body) = emulator
            .call_in(case.region, "CreateConnection", case.request())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let connection = body["connection"].clone();

        let id = text(&connection, "id");
        assert!(
            is_connection_id(&id),
            "case {case:?}: expected a ConnectionId matching ^(mcc|lmcc)-[a-z0-9]{{8}}$, got {id:?}"
        );
        let prefix = expected_id_prefix(&environment);
        assert!(
            id.starts_with(&format!("{prefix}-")),
            "case {case:?}: expected the id prefix {prefix:?} for this provider kind, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: expected a freshly allocated id, got the already-issued {id:?}"
        );

        let arn = text(&connection, "arn");
        let expected_arn = format!(
            "arn:aws:{SIGNING_NAME}:{}:{ACCOUNT_ID}:connection/{id}",
            case.region
        );
        assert_eq!(
            arn, expected_arn,
            "case {case:?}: expected the ARN to carry the signed region and the new id, got {arn:?}"
        );
        assert!(
            (59..=150).contains(&arn.len()),
            "case {case:?}: expected AmazonResourceName length 59..150, got {}",
            arn.len()
        );

        // description is @required on the output but optional on the input, so the
        // server supplies one when the caller does not.
        let description = text(&connection, "description");
        let expected_description = case.description.clone().unwrap_or_else(|| id.clone());
        assert_eq!(
            description, expected_description,
            "case {case:?}: expected the supplied description, or the id when none was supplied, got {description:?}"
        );

        assert_eq!(
            text(&connection, "bandwidth"),
            case.bandwidth,
            "case {case:?}: expected the requested bandwidth, got {:?}",
            connection["bandwidth"]
        );
        assert_eq!(
            connection["attachPoint"], case.attach_point,
            "case {case:?}: expected the requested attach point, got {:?}",
            connection["attachPoint"]
        );
        assert_eq!(
            text(&connection, "environmentId"),
            case.environment_id,
            "case {case:?}: expected the requested environmentId, got {:?}",
            connection["environmentId"]
        );

        // provider / location / type are the Environment's, which the request does
        // not carry at all.
        assert_eq!(
            connection["provider"], environment["provider"],
            "case {case:?}: expected the environment's provider, got {:?}",
            connection["provider"]
        );
        assert_eq!(
            connection["location"], environment["location"],
            "case {case:?}: expected the environment's location, got {:?}",
            connection["location"]
        );
        assert_eq!(
            connection["type"], environment["type"],
            "case {case:?}: expected the environment's product type, got {:?}",
            connection["type"]
        );

        assert_eq!(
            text(&connection, "state"),
            "requested",
            "case {case:?}: expected the modelled initial state 'requested', got {:?}",
            connection["state"]
        );

        let shared_id = text(&connection, "sharedId");
        assert!(
            is_uuid(&shared_id),
            "case {case:?}: expected a UUID sharedId, got {shared_id:?}"
        );
        let activation_key = text(&connection, "activationKey");
        assert!(
            (1..=2048).contains(&activation_key.len()),
            "case {case:?}: expected ActivationKey length 1..2048, got {}",
            activation_key.len()
        );
        assert_eq!(
            text(&connection, "ownerAccount"),
            ACCOUNT_ID,
            "case {case:?}: expected the emulator account id, got {:?}",
            connection["ownerAccount"]
        );

        match &case.tags {
            Some(tags) => assert_eq!(
                connection["tags"], *tags,
                "case {case:?}: expected the supplied tags, got {:?}",
                connection["tags"]
            ),
            None => assert!(
                connection["tags"].is_null()
                    || connection["tags"] == json!({}),
                "case {case:?}: expected no tags, got {:?}",
                connection["tags"]
            ),
        }

        // `billingTier` is OPTIONAL and AWS publishes no rule for assigning it, so
        // the emulator leaves it unset rather than inventing an ordinal. What the
        // rule does require is that nothing outside the member's modelled
        // `@range(min: 0, max: 8)` is ever reported.
        assert!(
            connection["billingTier"].is_null()
                || connection["billingTier"]
                    .as_i64()
                    .is_some_and(|tier| (0..=8).contains(&tier)),
            "case {case:?}: expected billingTier absent, or within the modelled 0..8, got {:?}",
            connection["billingTier"]
        );

        // The stored record is what a read returns: same content, every member.
        let fetched = emulator
            .ok("GetConnection", json!({ "identifier": id }))
            .await;
        assert_eq!(
            fetched["connection"], connection,
            "case {case:?}: expected GetConnection to return the created record, got {:?}",
            fetched["connection"]
        );
    }

}

/// `@idempotencyToken`: a repeat of the same call with the same client token
/// replays the first connection; a different token allocates a new one.
#[tokio::test]
async fn create_connection_replays_a_repeated_client_token() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let table = create_table(&catalog);

    for (index, (environment, case)) in table.into_iter().enumerate() {
        let token = format!("token-{index}");
        let mut request = case.request();
        request["clientToken"] = json!(token);

        let first = emulator.ok("CreateConnection", request.clone()).await;
        let replay = emulator.ok("CreateConnection", request.clone()).await;
        assert_eq!(
            replay["connection"], first["connection"],
            "case {case:?}: expected the same connection back for client token {token:?}, got {:?}",
            replay["connection"]
        );

        let mut other = request.clone();
        other["clientToken"] = json!(format!("{token}-other"));
        let fresh = emulator.ok("CreateConnection", other).await;
        assert_ne!(
            fresh["connection"]["id"], first["connection"]["id"],
            "case {case:?}: expected a distinct connection for a distinct client token, got {:?}",
            fresh["connection"]["id"]
        );

        // Exactly two connections exist for this pair of tokens, not three.
        let listed = emulator
            .ok(
                "ListConnections",
                json!({ "environmentId": case.environment_id }),
            )
            .await;
        let for_token: Vec<&Value> = listed["connections"]
            .as_array()
            .expect("connections is a list")
            .iter()
            .filter(|summary| {
                summary["id"] == first["connection"]["id"]
                    || summary["id"] == fresh["connection"]["id"]
            })
            .collect();
        assert_eq!(
            for_token.len(),
            2,
            "case {case:?}: expected the two tokens to have produced two connections, got {for_token:?}"
        );

        // The token names the request it created a connection for: reusing it with
        // a different request is refused rather than answered with that
        // connection. The rows differ in WHICH request-owned member changed.
        let other_bandwidth = available_bandwidths(&environment)
            .into_iter()
            .find(|bandwidth| *bandwidth != case.bandwidth);
        let mut mismatches: Vec<(&str, Value)> = vec![
            ("a different description", {
                let mut request = request.clone();
                request["description"] = json!("a different description");
                request
            }),
            ("a different environment", {
                let mut request = request.clone();
                request["environmentId"] = json!("env-that-is-not-this-one");
                request
            }),
            ("different tags", {
                let mut request = request.clone();
                request["tags"] = json!({ "a-key-the-first-call-did-not-send": "v" });
                request
            }),
        ];
        if let Some(bandwidth) = other_bandwidth {
            let mut request = request.clone();
            request["bandwidth"] = json!(bandwidth);
            mismatches.push(("a different bandwidth", request));
        }

        for (label, mismatched) in mismatches {
            let (status, body) = emulator.call("CreateConnection", mismatched).await;
            assert_eq!(
                status, 400,
                "case {case:?}/{label}: expected the reused token to be refused, got {status} with {body:?}"
            );
            assert!(
                text(&body, "__type").contains("InterconnectValidationException"),
                "case {case:?}/{label}: expected InterconnectValidationException, got {body:?}"
            );
        }
    }
}

/// A connection is only created where the Environment allows it: the environment
/// must exist, must not be `unavailable`, and must currently offer the bandwidth.
#[tokio::test]
async fn create_connection_refuses_what_the_environment_does_not_offer() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;

    for environment in &catalog {
        let id = text(environment, "environmentId");
        let available = available_bandwidths(environment);
        let supported = supported_bandwidths(environment);

        // A bandwidth the environment supports but does not currently offer, plus
        // one it does not even support: both are refused, and 1 Mbps/one past the
        // largest offered value cover the ends of the range.
        let mut refused: Vec<String> = supported
            .iter()
            .filter(|bandwidth| !available.contains(bandwidth))
            .cloned()
            .collect();
        refused.push("1Mbps".to_string());
        refused.push("40Gbps".to_string());

        for bandwidth in refused {
            let request = json!({
                "environmentId": id,
                "bandwidth": bandwidth,
                "attachPoint": dxgw_attach_point(7),
            });
            let (status, body) = emulator.call("CreateConnection", request).await;
            let case = format!("environment {id}, bandwidth {bandwidth}");
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a bandwidth the environment does not offer, got {status} with {body:?}"
            );
            let kind = text(&body, "__type");
            let expected_kind = if text(environment, "state") == "unavailable" {
                "InterconnectClientException"
            } else {
                "InterconnectValidationException"
            };
            assert!(
                kind.contains(expected_kind),
                "case {case:?}: expected {expected_kind}, got {body:?}"
            );
            assert!(
                text(&body, "message").contains(&bandwidth)
                    || text(&body, "message").contains(&id),
                "case {case:?}: expected the refusal to name the member or the environment, got {body:?}"
            );
        }

        // And every bandwidth the environment DOES offer is accepted.
        for bandwidth in available {
            let request = json!({
                "environmentId": id,
                "bandwidth": bandwidth,
                "attachPoint": dxgw_attach_point(8),
            });
            let (status, body) = emulator.call("CreateConnection", request).await;
            assert_eq!(
                status, 200,
                "case {id:?}/{bandwidth:?}: expected an available bandwidth to be accepted, got {status} with {body:?}"
            );
        }
    }

    // An environment that does not exist is a 400 client refusal, and creates nothing.
    let (status, body) = emulator
        .call(
            "CreateConnection",
            json!({
                "environmentId": "env-does-not-exist",
                "bandwidth": "1Gbps",
                "attachPoint": dxgw_attach_point(9),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"unknown environment\": expected HTTP 400, got {status} with {body:?}"
    );
    let listed = emulator
        .ok(
            "ListConnections",
            json!({ "environmentId": "env-does-not-exist" }),
        )
        .await;
    assert_eq!(
        listed["connections"],
        json!([]),
        "case \"unknown environment\": expected no connection to have been created, got {:?}",
        listed["connections"]
    );
}

/// The store is bounded: once the account holds the maximum number of
/// connections, the next `CreateConnection` is refused with the quota error the
/// service models, and nothing is stored for it.
#[tokio::test]
async fn create_connection_stops_at_the_connection_quota() {
    use service_com_amazonaws_interconnect_interconnect::state::MAX_CONNECTIONS;

    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let (_, case) = create_table(&catalog)
        .into_iter()
        .next()
        .expect("the create table has rows");

    for index in 0..MAX_CONNECTIONS {
        let (status, body) = emulator.call("CreateConnection", case.request()).await;
        assert_eq!(
            status, 200,
            "case \"connection {index} of the quota\": expected HTTP 200, got {status} with {body:?}"
        );
    }

    let (status, body) = emulator.call("CreateConnection", case.request()).await;
    assert_eq!(
        status, 402,
        "case \"one past the quota\": expected the quota refusal's modelled status, got {status} with {body:?}"
    );
    assert!(
        text(&body, "__type").contains("ServiceQuotaExceededException"),
        "case \"one past the quota\": expected ServiceQuotaExceededException, got {body:?}"
    );

    // The refusal stored nothing: the store still holds exactly the quota.
    let mut counted = 0;
    let mut token: Option<String> = None;
    loop {
        let mut request = Map::new();
        request.insert("maxResults".to_string(), json!(100));
        if let Some(token) = &token {
            request.insert("nextToken".to_string(), json!(token));
        }
        let listed = emulator.ok("ListConnections", Value::Object(request)).await;
        counted += listed["connections"]
            .as_array()
            .expect("connections is a list")
            .len();
        token = listed["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        counted, MAX_CONNECTIONS,
        "case \"one past the quota\": expected the store to hold exactly the quota, got {counted}"
    );

    // A quota counts the connections the account HOLDS: deleting one frees a
    // place, and the next create is accepted once the clock has settled that
    // record into its terminal `deleted` state.
    let first_id = {
        let listed = emulator
            .ok("ListConnections", json!({ "maxResults": 1 }))
            .await;
        text(&listed["connections"][0], "id")
    };
    emulator
        .ok("DeleteConnection", json!({ "identifier": first_id }))
        .await;
    tokio::time::sleep(SETTLE_WAIT).await;

    let (status, body) = emulator.call("CreateConnection", case.request()).await;
    assert_eq!(
        status, 200,
        "case \"after deleting one of the quota\": expected the freed place to be usable, got {status} with {body:?}"
    );
    let replacement = text(&body["connection"], "id");
    assert_ne!(
        replacement, first_id,
        "case \"after deleting one of the quota\": expected a freshly allocated id, got the deleted {first_id:?}"
    );
}

// ---------------------------------------------------------------------------
// ListConnections
// ---------------------------------------------------------------------------

/// Every `ListConnections` filter is applied conjunctively over the persisted
/// store, and a paged walk visits each connection exactly once.
#[tokio::test]
async fn list_connections_applies_every_filter_and_pages_with_a_real_cursor() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let table = create_table(&catalog);

    let mut created: Vec<Value> = Vec::new();
    for (_, case) in &table {
        let body = emulator.ok("CreateConnection", case.request()).await;
        created.push(body["connection"].clone());
    }
    assert!(
        created.len() >= 4,
        "expected several connections to filter over, got {}",
        created.len()
    );

    // Give one of them a terminal state, so the `state` filter has two values to
    // separate. Settle it first, so no transient state is in flight while the
    // filter expectations below are computed from the full listing.
    let deleted_id = text(&created[0], "id");
    emulator
        .ok("DeleteConnection", json!({ "identifier": deleted_id }))
        .await;
    tokio::time::sleep(SETTLE_WAIT).await;

    let all = emulator.ok("ListConnections", json!({})).await;
    let all: Vec<Value> = all["connections"]
        .as_array()
        .expect("connections is a list")
        .clone();
    assert_eq!(
        all.len(),
        created.len(),
        "expected every created connection to be listed, got {all:?}"
    );

    let mut cases: Vec<(String, Value)> = vec![
        ("no filter".to_string(), json!({})),
        (
            "state requested".to_string(),
            json!({ "state": "requested" }),
        ),
        ("state deleting".to_string(), json!({ "state": "deleting" })),
        ("state deleted".to_string(), json!({ "state": "deleted" })),
        ("state available".to_string(), json!({ "state": "available" })),
    ];
    for summary in &all {
        cases.push((
            format!("environmentId {:?}", summary["environmentId"]),
            json!({ "environmentId": summary["environmentId"] }),
        ));
        cases.push((
            format!("provider {:?}", summary["provider"]),
            json!({ "provider": summary["provider"] }),
        ));
        cases.push((
            format!("attachPoint {:?}", summary["attachPoint"]),
            json!({ "attachPoint": summary["attachPoint"] }),
        ));
    }
    cases.push((
        "environmentId and state together".to_string(),
        json!({ "environmentId": all[0]["environmentId"], "state": all[0]["state"] }),
    ));
    cases.push((
        "environmentId and a state nothing is in".to_string(),
        json!({ "environmentId": all[0]["environmentId"], "state": "failed" }),
    ));

    for (case, filter) in cases {
        let expected: Vec<Value> = all
            .iter()
            .filter(|summary| {
                ["state", "environmentId", "provider", "attachPoint"]
                    .iter()
                    .all(|member| {
                        filter
                            .get(*member)
                            .is_none_or(|wanted| summary[*member] == *wanted)
                    })
            })
            .cloned()
            .collect();
        let listed = emulator.ok("ListConnections", filter).await;
        let actual = listed["connections"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the {} matching summaries, got {actual:?}",
            expected.len()
        );
    }

    // A paged walk, at every page size, equals the unpaged listing: `maxResults=1`
    // twice walks two DISTINCT connections.
    for page_size in 1..=(all.len() + 1) {
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = Map::new();
            request.insert("maxResults".to_string(), json!(page_size));
            if let Some(token) = &token {
                request.insert("nextToken".to_string(), json!(token));
            }
            let listed = emulator.ok("ListConnections", Value::Object(request)).await;
            walked.extend(listed["connections"].as_array().cloned().unwrap_or_default());
            token = listed["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= all.len(),
                "page size {page_size}: expected the walk to terminate, got {} items so far",
                walked.len()
            );
        }
        assert_eq!(
            walked, all,
            "page size {page_size}: expected the paged walk to equal the unpaged listing, got {walked:?}"
        );
    }

    // Same cursor rule here: a token ListConnections did not issue is refused.
    for token in ["not-a-cursor", "mcc-00000000", "env:env-csp-azure-us-east-1"] {
        let (status, body) = emulator
            .call("ListConnections", json!({ "nextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400 for a cursor ListConnections never issued, got {status} with {body:?}"
        );
    }

    // Every summary member is the stored connection's own.
    for summary in &all {
        let id = text(summary, "id");
        let fetched = emulator
            .ok("GetConnection", json!({ "identifier": id }))
            .await;
        let connection = &fetched["connection"];
        for member in [
            "id",
            "arn",
            "description",
            "bandwidth",
            "attachPoint",
            "environmentId",
            "provider",
            "location",
            "type",
            "state",
            "sharedId",
            "billingTier",
        ] {
            assert_eq!(
                summary[member], connection[member],
                "case {id:?}/{member}: expected the summary member to match the stored connection, got {:?} and {:?}",
                summary[member], connection[member]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// UpdateConnection
// ---------------------------------------------------------------------------

/// An update mutates the stored record in place, touching only the members the
/// request supplies; a bandwidth change also runs the connection through
/// `updating` before the clock settles it back.
#[tokio::test]
async fn update_connection_changes_only_the_members_it_is_given() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    // The clock-settle check costs real wall time, so it is made once rather than
    // on every row; the span is a single constant shared by all of them.
    let mut settle_checked = false;

    // Rows: description only, bandwidth only, both, and neither.
    for (environment, case) in create_table(&catalog) {
        let available = available_bandwidths(&environment);
        let other_bandwidth = available
            .iter()
            .find(|bandwidth| **bandwidth != case.bandwidth)
            .cloned();

        for (label, description, bandwidth) in [
            ("description only", Some("renamed link"), None),
            ("bandwidth only", None, other_bandwidth.clone()),
            (
                "description and bandwidth",
                Some("both changed"),
                other_bandwidth.clone(),
            ),
            ("neither", None, None),
        ] {
            let created = emulator.ok("CreateConnection", case.request()).await;
            let before = created["connection"].clone();
            let id = text(&before, "id");
            let label = format!("{label} on {id}");

            let mut request = Map::new();
            request.insert("identifier".to_string(), json!(id));
            if let Some(description) = description {
                request.insert("description".to_string(), json!(description));
            }
            if let Some(bandwidth) = &bandwidth {
                request.insert("bandwidth".to_string(), json!(bandwidth));
            }
            let updated = emulator
                .ok("UpdateConnection", Value::Object(request))
                .await;
            let after = updated["connection"].clone();

            // The supplied members changed; the omitted ones did not.
            let expected_description =
                description.map_or_else(|| text(&before, "description"), str::to_string);
            assert_eq!(
                text(&after, "description"),
                expected_description,
                "case {label:?}: expected description {expected_description:?}, got {:?}",
                after["description"]
            );
            let expected_bandwidth = bandwidth
                .clone()
                .unwrap_or_else(|| text(&before, "bandwidth"));
            assert_eq!(
                text(&after, "bandwidth"),
                expected_bandwidth,
                "case {label:?}: expected bandwidth {expected_bandwidth:?}, got {:?}",
                after["bandwidth"]
            );
            for member in [
                "id",
                "arn",
                "sharedId",
                "activationKey",
                "ownerAccount",
                "environmentId",
                "provider",
                "location",
                "type",
                "attachPoint",
            ] {
                assert_eq!(
                    after[member], before[member],
                    "case {label:?}: expected {member} to be untouched, got {:?} (was {:?})",
                    after[member], before[member]
                );
            }

            // A bandwidth change re-provisions the circuit: the connection reports
            // `updating` and settles back into the state it came from. A
            // description-only change touches no state.
            let changed_bandwidth = bandwidth.is_some_and(|new| new != text(&before, "bandwidth"));
            if changed_bandwidth {
                assert_eq!(
                    text(&after, "state"),
                    "updating",
                    "case {label:?}: expected a bandwidth change to report 'updating', got {:?}",
                    after["state"]
                );
                // Every member the update does not own is still untouched, the
                // unset `billingTier` included.
                assert_eq!(
                    after["billingTier"], before["billingTier"],
                    "case {label:?}: expected billingTier to be unchanged by a bandwidth change, got {:?} (was {:?})",
                    after["billingTier"], before["billingTier"]
                );
            } else {
                assert_eq!(
                    text(&after, "state"),
                    text(&before, "state"),
                    "case {label:?}: expected the state to be untouched, got {:?}",
                    after["state"]
                );
            }

            // The change is PERSISTED: a read observes it. While a bandwidth change
            // is in flight the reported state may already have settled, so the
            // members under test are compared rather than the whole record.
            let fetched = emulator
                .ok("GetConnection", json!({ "identifier": id }))
                .await;
            if changed_bandwidth {
                for member in ["description", "bandwidth", "billingTier", "id", "arn"] {
                    assert_eq!(
                        fetched["connection"][member], after[member],
                        "case {label:?}: expected GetConnection to observe {member}, got {:?}",
                        fetched["connection"][member]
                    );
                }
                let observed = text(&fetched["connection"], "state");
                assert!(
                    observed == "updating" || observed == text(&before, "state"),
                    "case {label:?}: expected the state to be 'updating' or the settled {:?}, got {observed:?}",
                    before["state"]
                );
            } else {
                assert_eq!(
                    fetched["connection"], after,
                    "case {label:?}: expected GetConnection to observe the update, got {:?}",
                    fetched["connection"]
                );
            }

            // The clock settles the transient state back, without anybody polling
            // in between. Checked once \u2014 the span is the same for every row.
            if changed_bandwidth && !settle_checked {
                settle_checked = true;
                tokio::time::sleep(SETTLE_WAIT).await;
                let settled = emulator
                    .ok("GetConnection", json!({ "identifier": id }))
                    .await;
                assert_eq!(
                    text(&settled["connection"], "state"),
                    text(&before, "state"),
                    "case {label:?}: expected the connection to settle back into its prior state, got {:?}",
                    settled["connection"]["state"]
                );
                assert_eq!(
                    text(&settled["connection"], "bandwidth"),
                    expected_bandwidth,
                    "case {label:?}: expected the new bandwidth to survive settling, got {:?}",
                    settled["connection"]["bandwidth"]
                );
            }
        }
    }
}

/// An update is refused where there is nothing to update, and refusing creates
/// nothing.
#[tokio::test]
async fn update_connection_refuses_an_unknown_or_unofferable_change() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let (environment, case) = create_table(&catalog)
        .into_iter()
        .next()
        .expect("the create table has rows");

    for identifier in ["mcc-00000000", "lmcc-zzzzzzzz"] {
        let (status, body) = emulator
            .call(
                "UpdateConnection",
                json!({ "identifier": identifier, "description": "new name" }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {identifier:?}: expected HTTP 404 for an unknown connection, got {status} with {body:?}"
        );
        let (status, body) = emulator
            .call("GetConnection", json!({ "identifier": identifier }))
            .await;
        assert_eq!(
            status, 404,
            "case {identifier:?}: expected the refused update to have created nothing, got {status} with {body:?}"
        );
    }

    // A bandwidth the environment does not currently offer is refused on update
    // for the same reason as on create.
    let created = emulator.ok("CreateConnection", case.request()).await;
    let id = text(&created["connection"], "id");
    let supported = supported_bandwidths(&environment);
    let available = available_bandwidths(&environment);
    let mut refused: Vec<String> = supported
        .into_iter()
        .filter(|bandwidth| !available.contains(bandwidth))
        .collect();
    refused.push("40Gbps".to_string());
    for bandwidth in refused {
        let (status, body) = emulator
            .call(
                "UpdateConnection",
                json!({ "identifier": id, "bandwidth": bandwidth }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {bandwidth:?}: expected HTTP 400 for a bandwidth the environment does not offer, got {status} with {body:?}"
        );
        let fetched = emulator
            .ok("GetConnection", json!({ "identifier": id }))
            .await;
        assert_eq!(
            fetched["connection"]["bandwidth"], created["connection"]["bandwidth"],
            "case {bandwidth:?}: expected the refused update to leave the record alone, got {:?}",
            fetched["connection"]["bandwidth"]
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteConnection
// ---------------------------------------------------------------------------

/// A delete starts `deleting` and the clock owes `deleted`; the record stays
/// readable in its terminal state, a repeat is idempotent, and an update is then
/// refused.
#[tokio::test]
async fn delete_connection_moves_through_deleting_to_deleted() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let mut deleted_ids: Vec<String> = Vec::new();

    for (_, case) in create_table(&catalog) {
        let created = emulator.ok("CreateConnection", case.request()).await;
        let id = text(&created["connection"], "id");

        let deleted = emulator
            .ok("DeleteConnection", json!({ "identifier": id }))
            .await;
        let state = text(&deleted["connection"], "state");
        assert_eq!(
            state, "deleting",
            "case {id:?}: expected DeleteConnection to report 'deleting', got {state:?}"
        );
        assert_eq!(
            deleted["connection"]["id"], created["connection"]["id"],
            "case {id:?}: expected the deleted connection to be the created one, got {:?}",
            deleted["connection"]
        );

        // A repeat neither fails nor restarts the transition.
        let repeat = emulator
            .ok("DeleteConnection", json!({ "identifier": id }))
            .await;
        assert_eq!(
            repeat["connection"], deleted["connection"],
            "case {id:?}: expected a repeated delete to replay the same record, got {:?}",
            repeat["connection"]
        );

        // While it is on its way out, it is no longer modifiable.
        let (status, body) = emulator
            .call(
                "UpdateConnection",
                json!({ "identifier": id, "description": "too late" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {id:?}: expected an update of a deleting connection to be refused, got {status} with {body:?}"
        );

        deleted_ids.push(id);
    }

    // One wait covers every row: each transition was due 400 ms after its own
    // delete, and nothing polled in between.
    tokio::time::sleep(SETTLE_WAIT).await;
    for id in &deleted_ids {
        let fetched = emulator
            .ok("GetConnection", json!({ "identifier": id }))
            .await;
        let state = text(&fetched["connection"], "state");
        assert_eq!(
            state, "deleted",
            "case {id:?}: expected the connection to have settled into 'deleted', got {state:?}"
        );

        // And the listing reports the terminal state, with the state filter
        // actually applied.
        for (wanted, should_contain) in [
            ("deleted", true),
            ("requested", false),
            ("available", false),
            ("deleting", false),
        ] {
            let listed = emulator
                .ok("ListConnections", json!({ "state": wanted }))
                .await;
            let present = listed["connections"]
                .as_array()
                .expect("connections is a list")
                .iter()
                .any(|summary| summary["id"] == json!(id));
            assert_eq!(
                present, should_contain,
                "case {id:?}/state={wanted}: expected present={should_contain}, got {:?}",
                listed["connections"]
            );
        }
    }

    // A delete of something that never existed is a 404.
    let (status, body) = emulator
        .call("DeleteConnection", json!({ "identifier": "mcc-ffffffff" }))
        .await;
    assert_eq!(
        status, 404,
        "case \"unknown connection\": expected HTTP 404, got {status} with {body:?}"
    );
}
