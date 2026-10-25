// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetNetworkTelemetry`, driven over HTTP.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 route, the
//! `coreNetworkId` / `registeredGatewayArn` / `awsRegion` / `accountId` / `resourceType` /
//! `resourceArn` / `maxResults` / `nextToken` query bindings and the error envelopes are
//! exercised exactly as a client hits them.
//!
//! The rule under test: `GetNetworkTelemetry` reports the session health of the
//! connections REGISTERED with the addressed global network — one entry per registration,
//! narrowed by every filter the request carries, paginated by `MaxResults`/`NextToken` —
//! and refuses a `ResourceType` outside the three the member documents, and a global
//! network the store does not hold.
//!
//! The remaining operations that register such a connection
//! (`AssociateTransitGatewayConnectPeer`, `RegisterTransitGateway`) are still scaffold
//! stubs, each
//! with its own issue, so no request of this suite's own creates one: these suites SEED the
//! store through `crate::state` and then read it back over the wire. Nothing else about
//! the answer is seeded — the ids, account ids and health a telemetry entry reports are
//! computed by the handler out of those records. The one registration a request DOES make
//! is covered by `crate::connect_peer_association_lifecycle`, which associates a core
//! network Connect peer and reads its session back through this operation.

use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_networkmanager_networkmanager::state::{
    self, TelemetryResourceRecord, CONNECTION_STATUS_DOWN, CONNECTION_STATUS_UP,
    CONNECTION_TYPE_BGP, CONNECTION_TYPE_IPSEC, CONNECT_PEER_TYPE, SETTLE_DELAY,
    TELEMETRY_RESOURCE_TYPES, TRANSIT_GATEWAY_CONNECT_PEER_TYPE, VPN_CONNECTION_TYPE,
};

use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::resource_tagging::escape;

/// The account this emulator's own resources belong to, written out because `AccountId` is
/// modelled `length 12..12` and a telemetry entry reports the account its resource's ARN
/// carries, not the one the emulator runs as.
const ACCOUNT: &str = "111122223333";
/// A second account, for a connection owned by another account of the organization.
const OTHER_ACCOUNT: &str = "444455556666";

/// The two core networks the Cloud WAN rows below belong to. The ids satisfy the modelled
/// `CoreNetworkId` pattern `^core-network-([0-9a-f]{8,17})$`
/// (`smithy_query shape=CoreNetworkId`), so they survive the `coreNetworkId` query binding.
const CORE_NETWORK_A: &str = "core-network-0123456789abcdef0";
/// See [`CORE_NETWORK_A`].
const CORE_NETWORK_B: &str = "core-network-fedcba9876543210f";

/// The `GetNetworkTelemetry` URI of `network`, with `query` (already escaped) appended
/// when it is not empty.
fn telemetry_uri(network: &str, query: &str) -> String {
    let path = format!("/global-networks/{network}/network-telemetry");
    if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    }
}

/// The ARN of an Amazon VPC resource: the owner of a transit gateway Connect peer and of a
/// Site-to-Site VPN connection, so the ARN carries both a Region and an account.
fn vpc_arn(kind: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:aws:ec2:{region}:{account}:{kind}/{id}")
}

/// The ARN of a Cloud WAN Connect peer: a resource of Network Manager, which is a GLOBAL
/// service, so the Region segment of its ARN is EMPTY — the Region such an entry reports
/// is the edge location recorded with the registration.
fn connect_peer_arn(account: &str, id: &str) -> String {
    format!("arn:aws:networkmanager::{account}:connect-peer/{id}")
}

/// One connection to register with a global network: a row of the table below.
#[derive(Debug, Clone)]
struct Registration {
    /// What makes this row different from the others.
    case: &'static str,
    /// One of [`TELEMETRY_RESOURCE_TYPES`].
    resource_type: &'static str,
    resource_arn: String,
    aws_region: &'static str,
    /// Set for a Cloud WAN Connect peer, which belongs to a core network.
    core_network_id: Option<&'static str>,
    /// Set for a connection reached through a registered gateway.
    registered_gateway_arn: Option<String>,
    address: Option<&'static str>,
}

impl Registration {
    /// The stored record for this row, registered `ago` before now — so `ago` at or past
    /// [`SETTLE_DELAY`] is a session that has already come up.
    fn record(&self, global_network: &str, ago: Duration) -> TelemetryResourceRecord {
        let registered_at = SystemTime::now() - ago;
        TelemetryResourceRecord {
            global_network_id: global_network.to_string(),
            resource_type: self.resource_type.to_string(),
            resource_arn: self.resource_arn.clone(),
            aws_region: self.aws_region.to_string(),
            core_network_id: self.core_network_id.map(str::to_string),
            registered_gateway_arn: self.registered_gateway_arn.clone(),
            address: self.address.map(str::to_string),
            registered_at,
            connected_at: registered_at + SETTLE_DELAY,
        }
    }

    /// The connection type a telemetry entry for this row must report: IPsec for a VPN
    /// connection, BGP for a Connect peer of either kind.
    fn expected_connection_type(&self) -> &'static str {
        if self.resource_type == VPN_CONNECTION_TYPE {
            CONNECTION_TYPE_IPSEC
        } else {
            CONNECTION_TYPE_BGP
        }
    }

    /// The account that owns this row's resource: the one its ARN carries.
    fn expected_account_id(&self) -> &str {
        self.resource_arn
            .split(':')
            .nth(4)
            .unwrap_or_else(|| panic!("row {}: the ARN must carry an account", self.case))
    }

    /// The id a telemetry entry for this row must report: the tail of its ARN.
    fn expected_resource_id(&self) -> &str {
        self.resource_arn
            .rsplit('/')
            .next()
            .unwrap_or_else(|| panic!("row {}: the ARN must carry a resource id", self.case))
    }
}

/// The registrations the suites below seed, spanning the class the rule generalises over:
/// each of the three documented resource types, several Regions, two owning accounts, the
/// Cloud WAN side (a core network, no gateway) and the gateway side (a registered gateway,
/// no core network), and an address both recorded and absent.
fn table() -> Vec<Registration> {
    vec![
        Registration {
            case: "a Cloud WAN Connect peer with an address",
            resource_type: CONNECT_PEER_TYPE,
            resource_arn: connect_peer_arn(ACCOUNT, "connect-peer-0a1b2c3d4e5f60718"),
            aws_region: "us-west-2",
            core_network_id: Some(CORE_NETWORK_A),
            registered_gateway_arn: None,
            address: Some("10.0.0.1"),
        },
        Registration {
            case: "a Cloud WAN Connect peer in another Region, no address recorded",
            resource_type: CONNECT_PEER_TYPE,
            resource_arn: connect_peer_arn(ACCOUNT, "connect-peer-1b2c3d4e5f6071829"),
            aws_region: "eu-central-1",
            core_network_id: Some(CORE_NETWORK_A),
            registered_gateway_arn: None,
            address: None,
        },
        Registration {
            case: "a Cloud WAN Connect peer of another account and core network",
            resource_type: CONNECT_PEER_TYPE,
            resource_arn: connect_peer_arn(OTHER_ACCOUNT, "connect-peer-2c3d4e5f607182930"),
            aws_region: "us-west-2",
            core_network_id: Some(CORE_NETWORK_B),
            registered_gateway_arn: None,
            address: Some("10.1.0.1"),
        },
        Registration {
            case: "a transit gateway Connect peer",
            resource_type: TRANSIT_GATEWAY_CONNECT_PEER_TYPE,
            resource_arn: vpc_arn(
                "transit-gateway-connect-peer",
                "us-west-2",
                ACCOUNT,
                "tgw-connect-peer-0123456789abcdef0",
            ),
            aws_region: "us-west-2",
            core_network_id: None,
            registered_gateway_arn: Some(vpc_arn(
                "transit-gateway",
                "us-west-2",
                ACCOUNT,
                "tgw-0123456789abcdef0",
            )),
            address: Some("169.254.6.1"),
        },
        Registration {
            case: "a transit gateway Connect peer of another account and Region",
            resource_type: TRANSIT_GATEWAY_CONNECT_PEER_TYPE,
            resource_arn: vpc_arn(
                "transit-gateway-connect-peer",
                "us-east-1",
                OTHER_ACCOUNT,
                "tgw-connect-peer-123456789abcdef01",
            ),
            aws_region: "us-east-1",
            core_network_id: None,
            registered_gateway_arn: Some(vpc_arn(
                "transit-gateway",
                "us-east-1",
                OTHER_ACCOUNT,
                "tgw-123456789abcdef01",
            )),
            address: Some("169.254.6.5"),
        },
        Registration {
            case: "a Site-to-Site VPN connection",
            resource_type: VPN_CONNECTION_TYPE,
            resource_arn: vpc_arn(
                "vpn-connection",
                "us-west-2",
                ACCOUNT,
                "vpn-0123456789abcdef0",
            ),
            aws_region: "us-west-2",
            core_network_id: None,
            registered_gateway_arn: Some(vpc_arn(
                "customer-gateway",
                "us-west-2",
                ACCOUNT,
                "cgw-0123456789abcdef0",
            )),
            address: Some("203.0.113.12"),
        },
        Registration {
            case: "a Site-to-Site VPN connection of another account, no address recorded",
            resource_type: VPN_CONNECTION_TYPE,
            resource_arn: vpc_arn(
                "vpn-connection",
                "ap-southeast-2",
                OTHER_ACCOUNT,
                "vpn-123456789abcdef01",
            ),
            aws_region: "ap-southeast-2",
            core_network_id: None,
            registered_gateway_arn: Some(vpc_arn(
                "customer-gateway",
                "ap-southeast-2",
                OTHER_ACCOUNT,
                "cgw-123456789abcdef01",
            )),
            address: None,
        },
    ]
}

/// The `NetworkTelemetry` entries one answer carries, in the order returned.
fn entries_of(body: &Value) -> &Vec<Value> {
    body["NetworkTelemetry"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a NetworkTelemetry array in {body}"))
}

/// The resource ARNs one answer reports, in the order returned.
fn arns_of(body: &Value) -> Vec<String> {
    entries_of(body)
        .iter()
        .map(|entry| {
            entry["ResourceArn"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a ResourceArn in {entry}"))
                .to_string()
        })
        .collect()
}

/// `rows`' ARNs in the order the listing reports them: a global network's registrations are
/// held keyed by resource ARN, so one page is ordered by ARN.
fn expected_arns(rows: &[&Registration]) -> Vec<String> {
    let mut arns: Vec<String> = rows.iter().map(|row| row.resource_arn.clone()).collect();
    arns.sort();
    arns
}

/// Seed `rows` into the store `emulator` serves from, each registered `ago` before now.
fn register_all(
    engine: &engine::InMemoryEngine,
    global_network: &str,
    rows: &[Registration],
    ago: Duration,
) {
    let store = state::service_state(engine).expect("the networkmanager store must be reachable");
    let mut guard = store.lock();
    for row in rows {
        guard.register_telemetry_resource(row.record(global_network, ago));
    }
}

#[tokio::test]
async fn every_registered_connection_of_its_own_global_network_is_reported_with_derived_fields() {
    let (emulator, engine) = Emulator::with_engine();
    let network = emulator.create_global_network("telemetry").await;
    let other = emulator.create_global_network("another network").await;
    let bare = emulator.create_global_network("nothing registered").await;

    let rows = table();
    register_all(&engine, &network, &rows, PAST_SETTLE);
    // The first two rows are registered with ANOTHER global network as well, under the
    // same ARNs: a listing that leaked across global networks would report them twice.
    register_all(&engine, &other, &rows[..2], PAST_SETTLE);

    let (status, body) = emulator.get(&telemetry_uri(&network, "")).await;
    assert_eq!(
        status, 200,
        "GetNetworkTelemetry: expected 200, got {status} with {body}"
    );
    assert_eq!(
        arns_of(&body),
        expected_arns(&rows.iter().collect::<Vec<_>>()),
        "every registration of the addressed global network must be reported once, in ARN order, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "a listing that fits one page carries no token, got {body}"
    );

    for row in &rows {
        let entry = entries_of(&body)
            .iter()
            .find(|entry| entry["ResourceArn"].as_str() == Some(row.resource_arn.as_str()))
            .unwrap_or_else(|| panic!("case {}: no entry for its ARN in {body}", row.case));
        assert_eq!(
            entry["ResourceType"].as_str(),
            Some(row.resource_type),
            "case {}: expected the registered resource type, got {entry}",
            row.case
        );
        assert_eq!(
            entry["ResourceId"].as_str(),
            Some(row.expected_resource_id()),
            "case {}: the id must be the one its ARN names, got {entry}",
            row.case
        );
        assert_eq!(
            entry["AccountId"].as_str(),
            Some(row.expected_account_id()),
            "case {}: the account must be the one its ARN names, got {entry}",
            row.case
        );
        assert_eq!(
            entry["AwsRegion"].as_str(),
            Some(row.aws_region),
            "case {}: expected the registered Region, got {entry}",
            row.case
        );
        assert_eq!(
            entry["CoreNetworkId"].as_str(),
            row.core_network_id,
            "case {}: expected the registered core network, got {entry}",
            row.case
        );
        assert_eq!(
            entry["RegisteredGatewayArn"].as_str(),
            row.registered_gateway_arn.as_deref(),
            "case {}: expected the registered gateway, got {entry}",
            row.case
        );
        assert_eq!(
            entry["Address"].as_str(),
            row.address,
            "case {}: expected the recorded address, got {entry}",
            row.case
        );
        assert_eq!(
            entry["Health"]["Type"].as_str(),
            Some(row.expected_connection_type()),
            "case {}: expected the connection type of its kind, got {entry}",
            row.case
        );
        assert_eq!(
            entry["Health"]["Status"].as_str(),
            Some(CONNECTION_STATUS_UP),
            "case {}: a session registered longer ago than the settle span is up, got {entry}",
            row.case
        );
        assert!(
            entry["Health"]["Timestamp"].is_number(),
            "case {}: the health carries the instant its status took effect, got {entry}",
            row.case
        );
    }

    // The other global network reports only the two registrations it holds, and one that
    // holds none reports the empty telemetry it really holds.
    let (status, body) = emulator.get(&telemetry_uri(&other, "")).await;
    assert_eq!(
        status, 200,
        "case the other network: expected 200, got {status} with {body}"
    );
    assert_eq!(
        arns_of(&body),
        expected_arns(&rows[..2].iter().collect::<Vec<_>>()),
        "case the other network: expected only its own two registrations, got {body}"
    );
    let (status, body) = emulator.get(&telemetry_uri(&bare, "")).await;
    assert_eq!(
        status, 200,
        "case a network with no registration: expected 200, got {status} with {body}"
    );
    assert!(
        entries_of(&body).is_empty(),
        "case a network with no registration: expected an empty listing, got {body}"
    );
}

#[tokio::test]
async fn each_documented_filter_narrows_the_listing_to_the_registrations_that_match() {
    let (emulator, engine) = Emulator::with_engine();
    let network = emulator.create_global_network("filters").await;
    let rows = table();
    register_all(&engine, &network, &rows, PAST_SETTLE);

    // Each row is one documented filter (or a combination of two), with the predicate the
    // answer must equal: the registrations the filter matches, never a page of everything
    // and never an empty one where something matches.
    type Matcher = fn(&Registration) -> bool;
    let cases: Vec<(&str, String, Matcher)> = vec![
        (
            "resourceType=connect-peer",
            format!("resourceType={}", escape(CONNECT_PEER_TYPE)),
            |row| row.resource_type == CONNECT_PEER_TYPE,
        ),
        (
            "resourceType=transit-gateway-connect-peer",
            format!("resourceType={}", escape(TRANSIT_GATEWAY_CONNECT_PEER_TYPE)),
            |row| row.resource_type == TRANSIT_GATEWAY_CONNECT_PEER_TYPE,
        ),
        (
            "resourceType=vpn-connection",
            format!("resourceType={}", escape(VPN_CONNECTION_TYPE)),
            |row| row.resource_type == VPN_CONNECTION_TYPE,
        ),
        (
            "awsRegion of three registrations",
            "awsRegion=us-west-2".to_string(),
            |row| row.aws_region == "us-west-2",
        ),
        (
            "awsRegion of one registration",
            "awsRegion=eu-central-1".to_string(),
            |row| row.aws_region == "eu-central-1",
        ),
        (
            "awsRegion no registration lives in",
            "awsRegion=me-south-1".to_string(),
            |_| false,
        ),
        (
            "accountId of this account",
            format!("accountId={ACCOUNT}"),
            |row| row.expected_account_id() == ACCOUNT,
        ),
        (
            "accountId of the other account",
            format!("accountId={OTHER_ACCOUNT}"),
            |row| row.expected_account_id() == OTHER_ACCOUNT,
        ),
        (
            "accountId nothing is owned by",
            "accountId=999988887777".to_string(),
            |_| false,
        ),
        (
            "coreNetworkId of two Connect peers",
            format!("coreNetworkId={CORE_NETWORK_A}"),
            |row| row.core_network_id == Some(CORE_NETWORK_A),
        ),
        (
            "coreNetworkId of one Connect peer",
            format!("coreNetworkId={CORE_NETWORK_B}"),
            |row| row.core_network_id == Some(CORE_NETWORK_B),
        ),
        (
            "registeredGatewayArn of one transit gateway",
            format!(
                "registeredGatewayArn={}",
                escape(&vpc_arn(
                    "transit-gateway",
                    "us-west-2",
                    ACCOUNT,
                    "tgw-0123456789abcdef0"
                ))
            ),
            |row| {
                row.registered_gateway_arn.as_deref()
                    == Some(
                        vpc_arn(
                            "transit-gateway",
                            "us-west-2",
                            ACCOUNT,
                            "tgw-0123456789abcdef0",
                        )
                        .as_str(),
                    )
            },
        ),
        (
            "resourceArn of one registration",
            format!(
                "resourceArn={}",
                escape(&connect_peer_arn(ACCOUNT, "connect-peer-0a1b2c3d4e5f60718"))
            ),
            |row| {
                row.resource_arn == connect_peer_arn(ACCOUNT, "connect-peer-0a1b2c3d4e5f60718")
            },
        ),
        (
            "resourceArn nothing is registered under",
            format!(
                "resourceArn={}",
                escape(&connect_peer_arn(ACCOUNT, "connect-peer-99999999999999999"))
            ),
            |_| false,
        ),
        (
            "two filters together",
            format!(
                "resourceType={}&awsRegion=us-west-2",
                escape(CONNECT_PEER_TYPE)
            ),
            |row| row.resource_type == CONNECT_PEER_TYPE && row.aws_region == "us-west-2",
        ),
        (
            "a core network filter on a gateway-side kind",
            format!(
                "resourceType={}&coreNetworkId={CORE_NETWORK_A}",
                escape(VPN_CONNECTION_TYPE)
            ),
            |_| false,
        ),
    ];

    for (case, query, matches) in cases {
        let (status, body) = emulator.get(&telemetry_uri(&network, &query)).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let wanted: Vec<&Registration> = rows.iter().filter(|row| matches(row)).collect();
        assert_eq!(
            arns_of(&body),
            expected_arns(&wanted),
            "case {case}: expected the {} matching registrations, got {body}",
            wanted.len()
        );
    }
}

#[tokio::test]
async fn a_resource_type_outside_the_documented_set_is_refused_with_the_modelled_validation_error() {
    let (emulator, engine) = Emulator::with_engine();
    let network = emulator.create_global_network("telemetry types").await;
    register_all(&engine, &network, &table(), PAST_SETTLE);

    // Each row is a different WAY of being outside the three types the member's own
    // documentation enumerates (`smithy_query query="doc"
    // shape="GetNetworkTelemetryRequest" member="ResourceType"`): a network-resource type
    // that has no session to measure, a documented type in the wrong case or spelling, a
    // prefix of one, an invented name, and an identifier that is not a type at all.
    for (case, resource_type) in [
        ("a network resource with no session", "site"),
        ("another network resource with no session", "device"),
        ("a Cloud WAN core network", "core-network"),
        ("an attachment", "attachment"),
        ("a documented type in the wrong case", "Connect-Peer"),
        ("a documented type pluralised", "connect-peers"),
        ("a documented type with a trailing space", "connect-peer "),
        ("a prefix of a documented type", "vpn"),
        ("an underscore instead of a hyphen", "vpn_connection"),
        ("an invented type", "not-a-real-type"),
        (
            "an ARN instead of a type",
            "arn:aws:ec2:us-west-2:111122223333:vpn-connection/vpn-0123456789abcdef0",
        ),
    ] {
        let query = format!("resourceType={}", escape(resource_type));
        let (status, body) = emulator.get(&telemetry_uri(&network, &query)).await;
        assert_eq!(
            status, 400,
            "case {case} ({resource_type:?}): expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case} ({resource_type:?}): wrong error code in {body}"
        );
    }

    // The three documented types are accepted on the same global network, so the refusal
    // above is the set boundary and not a blanket refusal of the member.
    for resource_type in TELEMETRY_RESOURCE_TYPES {
        let query = format!("resourceType={}", escape(resource_type));
        let (status, body) = emulator.get(&telemetry_uri(&network, &query)).await;
        assert_eq!(
            status, 200,
            "case {resource_type}: expected 200, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_global_network_the_store_does_not_hold_is_refused_with_the_modelled_not_found() {
    let emulator = Emulator::new();
    let deleted = emulator.create_global_network("to be deleted").await;
    let (status, body) = emulator.delete(&format!("/global-networks/{deleted}")).await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200, got {status} with {body}"
    );

    for (case, network) in [
        ("never created", "global-network-00000000000000099".to_string()),
        ("deleted", deleted),
        ("not an id of this service", "nope".to_string()),
    ] {
        // This operation models `ResourceNotFoundException`
        // (`smithy_query operation="GetNetworkTelemetry"`), so the addressed resource is
        // answered 404 — and before the query string is judged, so an unsupported
        // `resourceType` on an unknown global network is still the 404.
        for query in ["", "resourceType=connect-peer", "resourceType=site"] {
            let (status, body) = emulator.get(&telemetry_uri(&network, query)).await;
            assert_eq!(
                status, 404,
                "case {case} ({query:?}): expected 404, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case} ({query:?}): wrong error code in {body}"
            );
            assert_eq!(
                body["ResourceType"].as_str(),
                Some("global-network"),
                "case {case} ({query:?}): the refusal names the kind not found, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn pagination_walks_every_registration_exactly_once_whatever_the_page_size() {
    let (emulator, engine) = Emulator::with_engine();
    let network = emulator.create_global_network("paging").await;
    let rows = table();
    register_all(&engine, &network, &rows, PAST_SETTLE);
    let all = expected_arns(&rows.iter().collect::<Vec<_>>());

    // `MaxResults` is modelled `range 1..500`, so the rows are no page size at all, each
    // published bound, and sizes either side of the number of registrations held.
    for page_size in [None, Some(1), Some(2), Some(3), Some(6), Some(7), Some(8), Some(500)] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(next) = &token {
                query.push(format!("nextToken={}", escape(next)));
            }
            let (status, body) = emulator
                .get(&telemetry_uri(&network, &query.join("&")))
                .await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size:?}: expected 200, got {status} with {body}"
            );
            let page = arns_of(&body);
            assert!(
                !page.is_empty(),
                "case maxResults={page_size:?}: a page while items remain is never empty, got {body}"
            );
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case maxResults={page_size:?}: page of {} exceeds the requested size, got {body}",
                    page.len()
                );
            }
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case maxResults={page_size:?}: pagination did not terminate after {pages} pages"
            );
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, all,
            "case maxResults={page_size:?}: expected every registration exactly once in ARN order, got {collected:?}"
        );
    }

    // A token this service did not issue names no offset into the listing, so it is
    // refused rather than silently read as "start from the beginning".
    for (case, bad_token) in [
        ("not hex at all", "not-a-token".to_string()),
        ("hex of text with no prefix of ours", hex::encode("elsewhere:0")),
        (
            "a truncated token of ours",
            hex::encode("networkmanager")[..8].to_string(),
        ),
    ] {
        let query = format!("nextToken={}", escape(&bad_token));
        let (status, body) = emulator.get(&telemetry_uri(&network, &query)).await;
        assert_eq!(
            status, 400,
            "case {case} ({bad_token:?}): expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case} ({bad_token:?}): wrong error code in {body}"
        );
    }
}

/// `at` as the epoch-seconds a restJson1 `DateTime` travels as.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock must be past the epoch")
        .as_secs_f64()
}

/// The `Health.Timestamp` of the one entry an answer carries.
fn single_health(body: &Value) -> (&str, f64) {
    let entries = entries_of(body);
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one telemetry entry in {body}"
    );
    let health = &entries[0]["Health"];
    let status = health["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a Health.Status in {body}"));
    let timestamp = health["Timestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("expected a Health.Timestamp in {body}"));
    (status, timestamp)
}

#[tokio::test]
async fn health_comes_up_once_the_session_settles_and_reading_telemetry_changes_nothing() {
    let (emulator, engine) = Emulator::with_engine();
    let network = emulator.create_global_network("health").await;

    // Registered NOW, so the session the registration starts is still coming up: the
    // settle span is never zero, and nothing sleeps for it — the due instant is stored.
    let row = table()
        .into_iter()
        .find(|row| row.resource_type == TRANSIT_GATEWAY_CONNECT_PEER_TYPE)
        .expect("the table holds a transit gateway Connect peer");
    let record = row.record(&network, Duration::ZERO);
    let registered_at = epoch_seconds(record.registered_at);
    let connected_at = epoch_seconds(record.connected_at);
    {
        let store =
            state::service_state(&*engine).expect("the networkmanager store must be reachable");
        store.lock().register_telemetry_resource(record);
    }

    let (status, body) = emulator.get(&telemetry_uri(&network, "")).await;
    assert_eq!(
        status, 200,
        "case while coming up: expected 200, got {status} with {body}"
    );
    let (down_status, down_timestamp) = single_health(&body);
    assert_eq!(
        down_status, CONNECTION_STATUS_DOWN,
        "case while coming up: a session that has not come up yet is {CONNECTION_STATUS_DOWN}, got {body}"
    );
    assert!(
        (down_timestamp - registered_at).abs() < 0.05,
        "case while coming up: the timestamp is the instant the status took effect, expected {registered_at}, got {down_timestamp}"
    );

    // Reading telemetry is a read: repeating it reports the identical health.
    for repeat in 1..=3 {
        let (status, again) = emulator.get(&telemetry_uri(&network, "")).await;
        assert_eq!(
            status, 200,
            "case repeat {repeat}: expected 200, got {status} with {again}"
        );
        assert_eq!(
            single_health(&again),
            (down_status, down_timestamp),
            "case repeat {repeat}: reading telemetry changed the health, got {again}"
        );
    }

    tokio::time::sleep(PAST_SETTLE).await;
    let (status, body) = emulator.get(&telemetry_uri(&network, "")).await;
    assert_eq!(
        status, 200,
        "case once settled: expected 200, got {status} with {body}"
    );
    let (up_status, up_timestamp) = single_health(&body);
    assert_eq!(
        up_status, CONNECTION_STATUS_UP,
        "case once settled: the session is {CONNECTION_STATUS_UP} past the settle span, got {body}"
    );
    assert!(
        (up_timestamp - connected_at).abs() < 0.05,
        "case once settled: the timestamp is the instant the session came up, expected {connected_at}, got {up_timestamp}"
    );
    assert!(
        up_timestamp > down_timestamp,
        "case once settled: the status took effect after the registration, got {up_timestamp} and {down_timestamp}"
    );
}
