// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `DescribeConnectionProposal`, driven over the wire the way a
//! client drives it: `POST /` with `X-Amz-Target: Interconnect.<Operation>` on
//! `awsJson1_0`, through the same `MultiServiceRouter::dispatch` the socket front
//! door uses.
//!
//! The RULE under test: an `activationKey` resolves to the proposal the stored
//! connection record holds, and the describe reports that record's bandwidth and
//! environmentId together with the named Environment's provider and location — a
//! key that resolves to no stored proposal is refused with the operation's modelled
//! 404. Every assertion here is checked over a TABLE built from the live seeded
//! catalog, whose rows differ in the environment, the provider kind, the bandwidth,
//! the attach-point union member and the region the request is signed for; the
//! refusal table's rows differ in the SHAPE of the unresolvable key, including both
//! published `ActivationKey` length bounds and one character past the upper one.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The signing name the model gives this service; also its ARN namespace.
const SIGNING_NAME: &str = "interconnect";
/// The account id every emulator ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

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

    /// Every stored connection, as `ListConnections` reports it.
    async fn connections(&self) -> Vec<Value> {
        let listed = self.ok("ListConnections", json!({})).await;
        listed["connections"]
            .as_array()
            .expect("connections is a list")
            .clone()
    }
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

/// An attach point naming a Direct Connect gateway by UUID.
fn dxgw_attach_point(seed: u8) -> Value {
    json!({ "directConnectGateway": format!("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee{seed:02}") })
}

/// An attach point naming another connection by full ARN.
fn arn_attach_point(seed: u8) -> Value {
    json!({
        "arn": format!(
            "arn:aws:interconnect:us-east-1:{ACCOUNT_ID}:connection/mcc-aaaaaa{seed:02}"
        )
    })
}

/// One row of the proposal table: a connection to create, and therefore a proposal
/// whose activation key `DescribeConnectionProposal` must resolve.
#[derive(Debug, Clone)]
struct ProposalCase {
    environment: Value,
    bandwidth: String,
    region: &'static str,
    attach_point: Value,
    description: Option<String>,
}

impl ProposalCase {
    fn environment_id(&self) -> String {
        text(&self.environment, "environmentId")
    }

    fn request(&self) -> Value {
        let mut request = serde_json::Map::new();
        request.insert("environmentId".to_string(), json!(self.environment_id()));
        request.insert("bandwidth".to_string(), json!(self.bandwidth));
        request.insert("attachPoint".to_string(), self.attach_point.clone());
        if let Some(description) = &self.description {
            request.insert("description".to_string(), json!(description));
        }
        Value::Object(request)
    }
}

/// The proposal table: every bandwidth of every environment that can take a
/// connection, alternating the attach-point union member, the presence of the
/// optional description, and the region the request is signed for.
fn proposal_table(catalog: &[Value]) -> Vec<ProposalCase> {
    let mut rows = Vec::new();
    let mut seed = 0u8;
    for environment in catalog {
        if text(environment, "state") == "unavailable" {
            continue;
        }
        for bandwidth in available_bandwidths(environment) {
            seed += 1;
            rows.push(ProposalCase {
                environment: environment.clone(),
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
                    Some(format!("proposal {seed}"))
                },
            });
        }
    }
    rows
}

/// The four members the model marks `@required` on the response, each within its
/// modelled constraints, and each equal to what the proposal's record and
/// Environment hold.
fn assert_proposal_matches(case: &str, proposal: &Value, environment: &Value, bandwidth: &str) {
    let reported_bandwidth = text(proposal, "bandwidth");
    assert_eq!(
        reported_bandwidth, bandwidth,
        "case {case:?}: expected the proposal's bandwidth, got {reported_bandwidth:?}"
    );
    assert!(
        is_bandwidth(&reported_bandwidth),
        "case {case:?}: expected a ConnectionBandwidth (^\\d+[MG]bps$, len 1..8), got {reported_bandwidth:?}"
    );

    let reported_environment = text(proposal, "environmentId");
    assert_eq!(
        reported_environment,
        text(environment, "environmentId"),
        "case {case:?}: expected the proposal's environmentId, got {reported_environment:?}"
    );
    assert!(
        (1..=64).contains(&reported_environment.len()),
        "case {case:?}: expected environmentId length 1..64, got {}",
        reported_environment.len()
    );

    assert_eq!(
        proposal["provider"], environment["provider"],
        "case {case:?}: expected the Environment's provider, got {:?}",
        proposal["provider"]
    );
    assert!(
        proposal["provider"]["cloudServiceProvider"].is_string()
            ^ proposal["provider"]["lastMileProvider"].is_string(),
        "case {case:?}: expected exactly one Provider union member, got {:?}",
        proposal["provider"]
    );

    let reported_location = text(proposal, "location");
    assert_eq!(
        reported_location,
        text(environment, "location"),
        "case {case:?}: expected the Environment's location, got {reported_location:?}"
    );
    assert!(
        reported_location.len() <= 255,
        "case {case:?}: expected location length at most 255, got {}",
        reported_location.len()
    );
}

/// Every activation key a `CreateConnection` minted describes the proposal that
/// call stored: its bandwidth and environmentId, and the provider and location of
/// the Environment it names. Checked over the whole table at once, so a key resolves
/// to ITS own proposal and not merely to some stored one.
#[tokio::test]
async fn describe_connection_proposal_reports_the_proposal_each_activation_key_names() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let table = proposal_table(&catalog);
    assert!(
        table.len() >= 4,
        "the proposal table must span several environments and bandwidths, got {} rows",
        table.len()
    );

    // Create every row FIRST, so each describe below runs against a store holding
    // many proposals at once.
    let mut minted: BTreeMap<String, (ProposalCase, String)> = BTreeMap::new();
    for case in table {
        let (status, body) = emulator
            .call_in(case.region, "CreateConnection", case.request())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateConnection, got {status} with {body:?}"
        );
        let connection = body["connection"].clone();
        let activation_key = text(&connection, "activationKey");
        let connection_id = text(&connection, "id");
        assert!(
            (1..=2048).contains(&activation_key.len()),
            "case {case:?}: expected ActivationKey length 1..2048, got {}",
            activation_key.len()
        );
        assert!(
            minted
                .insert(activation_key.clone(), (case.clone(), connection_id))
                .is_none(),
            "case {case:?}: expected a distinct activation key per connection, got a repeat"
        );
    }

    for (activation_key, (case, connection_id)) in &minted {
        let label = format!("{} / {}", case.environment_id(), case.bandwidth);

        let proposal = emulator
            .ok(
                "DescribeConnectionProposal",
                json!({ "activationKey": activation_key }),
            )
            .await;
        assert_proposal_matches(&label, &proposal, &case.environment, &case.bandwidth);

        // The proposal and the connection are one record read two ways: the three
        // members both views carry agree.
        let fetched = emulator
            .ok("GetConnection", json!({ "identifier": connection_id }))
            .await;
        let connection = &fetched["connection"];
        for member in ["bandwidth", "environmentId", "provider", "location"] {
            assert_eq!(
                proposal[member], connection[member],
                "case {label:?}: expected the proposal's {member} to agree with GetConnection, got {:?} vs {:?}",
                proposal[member], connection[member]
            );
        }

        // Describing is a READ: a second call answers identically and the connection
        // it names is untouched.
        let again = emulator
            .ok(
                "DescribeConnectionProposal",
                json!({ "activationKey": activation_key }),
            )
            .await;
        assert_eq!(
            again, proposal,
            "case {label:?}: expected a repeated describe to answer identically, got {again:?}"
        );
        let refetched = emulator
            .ok("GetConnection", json!({ "identifier": connection_id }))
            .await;
        assert_eq!(
            refetched, fetched,
            "case {label:?}: expected describing the proposal to leave the connection untouched, got {refetched:?}"
        );
    }

    // And nothing was created by the reads: the store still holds exactly the
    // connections the creates made.
    let stored = emulator.connections().await;
    assert_eq!(
        stored.len(),
        minted.len(),
        "expected the describes to have created nothing, got {} connections for {} proposals",
        stored.len(),
        minted.len()
    );
}

/// An `activationKey` that resolves to no stored proposal is the operation's
/// modelled 404, whatever shape the key has — and the refusal never echoes the key,
/// which `ActivationKey` marks `@sensitive`.
#[tokio::test]
async fn describe_connection_proposal_refuses_a_key_that_names_no_proposal() {
    use service_com_amazonaws_interconnect_interconnect::state::proposal_not_retrievable_message;

    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let case = proposal_table(&catalog)
        .into_iter()
        .next()
        .expect("the proposal table has rows");

    // One real proposal, so what is refused below is the key and not the store being
    // empty.
    let created = emulator.ok("CreateConnection", case.request()).await;
    let known = text(&created["connection"], "activationKey");
    emulator
        .ok(
            "DescribeConnectionProposal",
            json!({ "activationKey": known }),
        )
        .await;

    // Rows differ in the SHAPE of the unresolvable key: unrelated, a prefix of a real
    // one, a real one with a suffix, a case variant, and both published
    // `ActivationKey` length bounds (`@length(min: 1, max: 2048)`).
    let mut absent: Vec<String> = vec![
        "not-an-activation-key".to_string(),
        known[..known.len() - 1].to_string(),
        format!("{known}EXTRA"),
        known.to_ascii_lowercase(),
        "A".to_string(),
        "A".repeat(2048),
    ];
    absent.retain(|candidate| *candidate != known);

    for key in &absent {
        let (status, body) = emulator
            .call(
                "DescribeConnectionProposal",
                json!({ "activationKey": key }),
            )
            .await;
        let label = format!("absent key of {} characters", key.len());
        assert_eq!(
            status, 404,
            "case {label:?}: expected the modelled 404 for a key that names no proposal, got {status} with {body:?}"
        );
        assert!(
            text(&body, "__type").contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            body["__type"]
        );
        let message = text(&body, "message");
        assert_eq!(
            message,
            proposal_not_retrievable_message(),
            "case {label:?}: expected the service's own refusal wording, got {message:?}"
        );
        assert!(
            !message.contains(key.as_str()),
            "case {label:?}: expected the sensitive activationKey not to be echoed, got {message:?}"
        );
    }

    // One character PAST the upper bound, and one short of the lower one, are refused
    // at the boundary as malformed requests — a 400, never the 404 above and never a
    // 501.
    for (label, key) in [
        ("one past the 2048-character maximum", "A".repeat(2049)),
        ("one short of the 1-character minimum", String::new()),
    ] {
        let (status, body) = emulator
            .call(
                "DescribeConnectionProposal",
                json!({ "activationKey": key }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for a key outside the modelled length, got {status} with {body:?}"
        );
    }

    // A missing required member is likewise a 400, not a 404.
    let (status, body) = emulator.call("DescribeConnectionProposal", json!({})).await;
    assert_eq!(
        status, 400,
        "case \"no activationKey at all\": expected HTTP 400 for a missing required member, got {status} with {body:?}"
    );
}
