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
//! the attach point the connection is created on and the region the request is signed
//! for; the refusal table's rows differ in the SHAPE of the unresolvable key, including
//! both published `ActivationKey` length bounds and one character past the upper one.
//!
//! The second half of the rule: a key resolves to a proposal only while the connection
//! it names is one a `DeleteConnection` has NOT been made on — the connectivity the
//! proposal offered ceases to exist from the moment the delete is called, so the handle
//! is void and describing it is the same 404 an unissued key gets, while the record
//! itself stays readable by its identifier.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::emulator::{code, message, remote_account_for, Emulator};

/// The gateways the rows below attach their connections to: an attach point IS a
/// Direct Connect gateway of the account, so the suite creates its own and the rows
/// differ in which one they use, in its name and in its `amazonSideAsn`.
const GATEWAY_ROWS: [(&str, Option<i64>); 2] = [
    ("ic-proposal-a", None),
    ("ic-proposal-b", Some(64_512)),
];

/// Every stored connection of `region`, as `ListConnections` reports it there — a
/// connection belongs to the region its create was signed for, so each region's
/// endpoint lists only its own.
async fn connections_in(emulator: &Emulator, region: &str) -> Vec<Value> {
    let listed = emulator
        .ic_ok_in(region, "ListConnections", json!({}))
        .await;
    listed["connections"]
        .as_array()
        .expect("connections is a list")
        .clone()
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
        // The identifying information this Environment asks every create on it for.
        if let Some(remote_account) = remote_account_for(&self.environment) {
            request.insert("remoteAccount".to_string(), remote_account);
        }
        Value::Object(request)
    }
}

/// The proposal table: every bandwidth of every environment that can take a
/// connection, alternating the gateway the connection is attached to, the presence of
/// the optional description, and the region the request is signed for.
///
/// Each row carries the Environment ITS OWN Region's endpoint offers for that partner
/// offering, because an Environment belongs to the Region whose connections it
/// produces.
async fn proposal_table(emulator: &Emulator, gateways: &[String]) -> Vec<ProposalCase> {
    let catalog = emulator.environments().await;
    let mut rows = Vec::new();
    let mut seed = 0usize;
    for environment in &catalog {
        if text(environment, "state") == "unavailable" {
            continue;
        }
        for bandwidth in available_bandwidths(environment) {
            seed += 1;
            let region = if seed.is_multiple_of(2) {
                "eu-west-1"
            } else {
                "us-east-1"
            };
            rows.push(ProposalCase {
                environment: emulator.counterpart_in(region, environment).await,
                bandwidth,
                region,
                attach_point: json!({
                    "directConnectGateway": gateways[seed % gateways.len()]
                }),
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
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let table = proposal_table(&emulator, &gateways).await;
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
            .ic_in(case.region, "CreateConnection", case.request())
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
        let label = format!(
            "{} / {} in {}",
            case.environment_id(),
            case.bandwidth,
            case.region
        );

        let proposal = emulator
            .ic_ok_in(
                case.region,
                "DescribeConnectionProposal",
                json!({ "activationKey": activation_key }),
            )
            .await;
        assert_proposal_matches(&label, &proposal, &case.environment, &case.bandwidth);

        // The proposal and the connection are one record read two ways: the three
        // members both views carry agree.
        let fetched = emulator
            .ic_ok_in(
                case.region,
                "GetConnection",
                json!({ "identifier": connection_id }),
            )
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
            .ic_ok_in(
                case.region,
                "DescribeConnectionProposal",
                json!({ "activationKey": activation_key }),
            )
            .await;
        assert_eq!(
            again, proposal,
            "case {label:?}: expected a repeated describe to answer identically, got {again:?}"
        );
        let refetched = emulator
            .ic_ok_in(
                case.region,
                "GetConnection",
                json!({ "identifier": connection_id }),
            )
            .await;
        assert_eq!(
            refetched, fetched,
            "case {label:?}: expected describing the proposal to leave the connection untouched, got {refetched:?}"
        );
    }

    // And nothing was created by the reads: the store still holds exactly the
    // connections the creates made. Each region's endpoint lists only the
    // connections created through it, so the whole set is the union over the
    // regions the rows were created in.
    let mut regions: Vec<&str> = minted.values().map(|(case, _)| case.region).collect();
    regions.sort_unstable();
    regions.dedup();
    let mut stored = 0usize;
    for region in &regions {
        stored += connections_in(&emulator, region).await.len();
    }
    assert_eq!(
        stored,
        minted.len(),
        "expected the describes to have created nothing, got {stored} connections for {} proposals",
        minted.len()
    );
}

/// How long a wait for the state the clock owes a connection runs before the case
/// fails: a create is provisioned a span at a time, and `DeleteConnection` starts
/// `deleting` with the clock owing `deleted`.
const CLOCK_BUDGET: Duration = Duration::from_secs(5);

/// How often that wait re-reads.
const STATE_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The `ConnectionState` `GetConnection` reports for `identifier` in `region`.
async fn connection_state(emulator: &Emulator, region: &str, identifier: &str) -> String {
    let fetched = emulator
        .ic_ok_in(region, "GetConnection", json!({ "identifier": identifier }))
        .await;
    text(&fetched["connection"], "state")
}

/// Wait until `GetConnection` reports `identifier` in `wanted` — a state the clock
/// owes it, never one this test sets.
async fn wait_for_state(
    emulator: &Emulator,
    region: &str,
    identifier: &str,
    wanted: &str,
    case: &str,
) {
    let started = Instant::now();
    loop {
        let state = connection_state(emulator, region, identifier).await;
        if state == wanted {
            return;
        }
        assert!(
            started.elapsed() < CLOCK_BUDGET,
            "case {case:?}: expected {identifier} to reach {wanted:?} within {CLOCK_BUDGET:?}, it is still {state:?}"
        );
        tokio::time::sleep(STATE_POLL_INTERVAL).await;
    }
}

/// `DescribeConnectionProposal` for one key, as a `(status, body)` pair.
async fn describe(emulator: &Emulator, region: &str, activation_key: &str) -> (u16, Value) {
    emulator
        .ic_in(
            region,
            "DescribeConnectionProposal",
            json!({ "activationKey": activation_key }),
        )
        .await
}

/// The key names no describable proposal: the operation's modelled 404, worded the way
/// this service words a key that resolves to nothing, and never echoing the
/// `@sensitive` key itself.
fn assert_void_handle(case: &str, status: u16, body: &Value, activation_key: &str) {
    use service_com_amazonaws_interconnect_interconnect::state::proposal_not_retrievable_message;

    assert_eq!(
        status, 404,
        "case {case:?}: expected the modelled 404 for a key that names no proposal, got {status} with {body:?}"
    );
    assert!(
        code(body).contains("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        code(body)
    );
    assert_eq!(
        message(body),
        proposal_not_retrievable_message(),
        "case {case:?}: expected the service's own refusal wording, got {:?}",
        message(body)
    );
    assert!(
        !message(body).contains(activation_key),
        "case {case:?}: expected the sensitive activationKey not to be echoed, got {:?}",
        message(body)
    );
}

/// A connection a `DeleteConnection` has been made on has no proposal left: its
/// activation key is refused with the operation's modelled 404 from the instant the
/// delete is called — while the record is still `deleting`, and once the clock has
/// settled it into the terminal `deleted` — while the record stays readable by its
/// identifier and every other key still names its own proposal.
#[tokio::test]
async fn a_deleted_connections_activation_key_names_no_proposal() {
    use service_com_amazonaws_interconnect_interconnect::state::{
        DELETED_CONNECTION_STATE, DELETING_CONNECTION_STATE,
    };

    let emulator = Emulator::new();
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let table = proposal_table(&emulator, &gateways).await;
    assert!(
        table.len() >= 4,
        "the proposal table must span several environments and bandwidths, got {} rows",
        table.len()
    );

    // Rows differ in the table's own axes — the environment, the bandwidth, the attach
    // point and the region the request is signed for — and in the state the connection
    // is in when the delete lands: one deleted straight after the create, while its
    // proposal is still awaiting acceptance, and one deleted only once the clock has
    // provisioned the connection all the way to `available`.
    for (index, case) in table.iter().take(4).enumerate() {
        let provisioned_first = index.is_multiple_of(2);
        let label = format!(
            "{} / {} in {} / {}",
            case.environment_id(),
            case.bandwidth,
            case.region,
            if provisioned_first {
                "deleted once provisioned"
            } else {
                "deleted while awaiting acceptance"
            }
        );

        // A sibling proposal in the same region, so what is refused below is THIS key
        // and not the store having stopped answering for every proposal.
        let sibling = emulator
            .ic_ok_in(case.region, "CreateConnection", case.request())
            .await["connection"]
            .clone();
        let sibling_key = text(&sibling, "activationKey");

        let created = emulator
            .ic_ok_in(case.region, "CreateConnection", case.request())
            .await["connection"]
            .clone();
        let activation_key = text(&created, "activationKey");
        let identifier = text(&created, "id");
        if provisioned_first {
            wait_for_state(&emulator, case.region, &identifier, "available", &label).await;
        }

        // The live baseline: the key describes the proposal it names.
        let (status, before) = describe(&emulator, case.region, &activation_key).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected a live proposal to describe, got {status} with {before:?}"
        );
        assert_proposal_matches(&label, &before, &case.environment, &case.bandwidth);

        // The delete starts the teardown the clock then finishes.
        let deleted = emulator
            .ic_ok_in(
                case.region,
                "DeleteConnection",
                json!({ "identifier": identifier }),
            )
            .await["connection"]
            .clone();
        assert_eq!(
            text(&deleted, "state"),
            DELETING_CONNECTION_STATE,
            "case {label:?}: expected DeleteConnection to report the teardown state, got {:?}",
            deleted["state"]
        );

        // From that instant the key names nothing to describe or confirm.
        let row = format!("{label} / right after the delete");
        let (status, body) = describe(&emulator, case.region, &activation_key).await;
        assert_void_handle(&row, status, &body, &activation_key);

        // And once the clock has settled the terminal state, the same answer — twice
        // over, because describing is a read.
        wait_for_state(
            &emulator,
            case.region,
            &identifier,
            DELETED_CONNECTION_STATE,
            &label,
        )
        .await;
        let row = format!("{label} / once deleted");
        let (status, body) = describe(&emulator, case.region, &activation_key).await;
        assert_void_handle(&row, status, &body, &activation_key);
        let (again_status, again_body) = describe(&emulator, case.region, &activation_key).await;
        assert_eq!(
            (again_status, &again_body),
            (status, &body),
            "case {row:?}: expected a repeated describe to answer identically, got {again_status} with {again_body:?}"
        );

        // The record itself is still there to be read by its identifier: only the
        // proposal handle is void, which is what lets a caller poll the delete.
        let state = connection_state(&emulator, case.region, &identifier).await;
        assert_eq!(
            state, DELETED_CONNECTION_STATE,
            "case {label:?}: expected the deleted record to still report its terminal state, got {state:?}"
        );

        // Every other key still names its own proposal.
        let row = format!("{label} / a sibling proposal");
        let (status, sibling_body) = describe(&emulator, case.region, &sibling_key).await;
        assert_eq!(
            status, 200,
            "case {row:?}: expected a live sibling proposal to still describe, got {status} with {sibling_body:?}"
        );
        assert_proposal_matches(&row, &sibling_body, &case.environment, &case.bandwidth);
    }
}

/// An `activationKey` that is base64 text but resolves to no stored proposal is the
/// operation's modelled 404, whatever shape the key has — and the refusal never echoes
/// the key, which `ActivationKey` marks `@sensitive`. (Text that is not base64 at all is
/// a fault in the REQUEST, which [`crate::activation_key_is_base64`] covers.)
#[tokio::test]
async fn describe_connection_proposal_refuses_a_key_that_names_no_proposal() {
    use service_com_amazonaws_interconnect_interconnect::state::proposal_not_retrievable_message;

    let emulator = Emulator::new();
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let case = proposal_table(&emulator, &gateways)
        .await
        .into_iter()
        .next()
        .expect("the proposal table has rows");

    // One real proposal, so what is refused below is the key and not the store being
    // empty.
    let created = emulator.ic_ok("CreateConnection", case.request()).await;
    let known = text(&created["connection"], "activationKey");
    emulator
        .ic_ok(
            "DescribeConnectionProposal",
            json!({ "activationKey": known }),
        )
        .await;

    // Rows differ in the SHAPE of the unresolvable key, every one of them base64 text so
    // that what they test is the resolution and not the encoding: unrelated, a prefix of
    // a real one, a real one with a suffix, a case variant, the shortest text the
    // encoding admits and the published `ActivationKey` maximum (`@length(max: 2048)`).
    // A key whose text is not base64 at all is a fault in the REQUEST, which
    // [`crate::activation_key_is_base64`] covers at every length including the published
    // minimum.
    let mut absent: Vec<String> = vec![
        "bm90QW5BY3RpdmF0aW9uS2V5".to_string(),
        known[..known.len() - 1].to_string(),
        format!("{known}MORE"),
        known.to_ascii_lowercase(),
        "AA".to_string(),
        "A".repeat(2048),
    ];
    absent.retain(|candidate| *candidate != known);

    for key in &absent {
        let (status, body) = emulator
            .ic(
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
            .ic(
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
    let (status, body) = emulator.ic("DescribeConnectionProposal", json!({})).await;
    assert_eq!(
        status, 400,
        "case \"no activationKey at all\": expected HTTP 400 for a missing required member, got {status} with {body:?}"
    );
}
