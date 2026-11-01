// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateVpcPeeringConnection` and `DescribeVpcPeeringConnections`,
//! driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by
//! the `ec2` signing name in the request's SigV4 credential scope, so every call here
//! carries one.
//!
//! The rules under test:
//!
//! * `CreateVpcPeeringConnection` RECORDS ONE REQUEST between the requester VPC the request
//!   names and the accepter VPC it asks to peer with, answers it `initiating-request` with
//!   the expiry the operation documents, and the clock owes it `pending-acceptance` one
//!   [`ec2::state::TRANSITION_DELAY`] later. The rows differ in the accepter VPC, the
//!   account that owns it, its Region and the tags the request asks for.
//! * A request between VPCs WITH OVERLAPPING CIDR BLOCKS is recorded `failed` instead —
//!   "If you create a VPC peering connection request between VPCs with overlapping CIDR
//!   blocks, the VPC peering connection has a status of failed"
//!   (`CreateVpcPeeringConnection`). The rows differ in how the two ranges meet.
//! * A request for a pair a connection of the account ALREADY PEERS is a duplicate, named
//!   in either direction; one whose earlier connection no longer holds the pair is not.
//! * `DescribeVpcPeeringConnections` REPORTS THE STORE: the set it answers is the account's
//!   connections narrowed by the `VpcPeeringConnectionIds` and `Filters` the request names
//!   and cut into `MaxResults`-sized pages. Every table row is a distinct input along what
//!   that generalises over — which filter, how many values, how many filters, which ids,
//!   which page size.
//! * A side's CIDR information is reported for the requester VPC from the moment the
//!   request is made and for the accepter's once the connection is `active` — "CIDR block
//!   information is only returned when describing an active VPC peering connection"
//!   (`VpcPeeringConnection$AccepterVpcInfo`).
//! * A request nobody answered by its own `ExpirationTime` is reported `expired`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ec2::ops::types::{Tag, VpcPeeringConnectionStateReason};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, TRANSITION_DELAY,
    VPC_PEERING_CONNECTION_ALREADY_EXISTS, VPC_PEERING_CONNECTION_FAILED_MESSAGE,
    VPC_PEERING_CONNECTION_ID_PREFIX, VPC_PEERING_CONNECTION_REQUEST_LIFETIME,
    VPC_PEERING_CONNECTION_STATE_ACTIVE, VPC_PEERING_CONNECTION_STATE_EXPIRED,
    VPC_PEERING_CONNECTION_STATE_FAILED, VPC_PEERING_CONNECTION_STATE_INITIATING_REQUEST,
    VPC_PEERING_CONNECTION_STATE_PENDING_ACCEPTANCE, VPC_PEERING_CONNECTION_STATE_PROVISIONING,
    VpcPeeringConnectionSpec, vpc_peering_connection_awaits_acceptance,
    vpc_peering_initiating_request_message, vpc_peering_pending_acceptance_message,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, error_code, error_message, filters, indexed};
use super::wire::{child, child_text, document, is_long_id, set_items};

/// The Region every call here is signed for: the requester VPC is in the Region the request
/// was made in, so this is the Region a peering's requester side reports.
const REGION: &str = "us-east-1";

/// A Region of the same partition that is NOT [`REGION`], for the accepter VPC of an
/// inter-Region request — "The Region code for the accepter VPC, if the accepter VPC is
/// located in a Region other than the Region in which you make the request"
/// (`CreateVpcPeeringConnectionRequest$PeerRegion`).
const OTHER_REGION: &str = "us-west-2";

/// An account that is not the one this emulator serves, for the accepter VPC of a
/// cross-account request — "The accepter VPC can belong to another Amazon Web Services
/// account" (`CreateVpcPeeringConnection`). Twelve digits, as an account id is.
const OTHER_ACCOUNT: &str = "210987654321";

/// A well-formed VPC id of [`OTHER_ACCOUNT`], which this store therefore holds no VPC for:
/// a cross-account accepter is not this account's to resolve.
const OTHER_ACCOUNT_VPC_ID: &str = "vpc-0a1b2c3d4e5f60718";

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `CreateVpcPeeringConnection` call carrying `params`, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateVpcPeeringConnection", params).await
}

/// One `DescribeVpcPeeringConnections` call carrying `params`, as its HTTP status and XML
/// body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeVpcPeeringConnections", params).await
}

/// One VPC created over the wire, by id — so a peering can name a VPC the account really
/// has without reaching into the store.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let params = [param("CidrBlock", cidr_block)];
    let (status, body) = call(router, REGION, "CreateVpc", &params).await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    super::wire::member(&format!("CreateVpc {cidr_block}"), &body, "vpcId")
}

/// One VPC created over the wire with an Amazon-provided IPv6 block, as its id and that
/// block — the input a peering's IPv6 reporting generalises over.
async fn create_dual_stack_vpc(router: &MultiServiceRouter, cidr_block: &str) -> (String, String) {
    let params = [
        param("CidrBlock", cidr_block),
        param("AmazonProvidedIpv6CidrBlock", "true"),
    ];
    let (status, body) = call(router, REGION, "CreateVpc", &params).await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block} (dual stack): expected HTTP 200, got {status} with body {body}"
    );
    let case = format!("CreateVpc {cidr_block} (dual stack)");
    let vpc_id = super::wire::member(&case, &body, "vpcId");
    let ipv6 = super::wire::ipv6_association(&case, &body, "ipv6CidrBlock");
    (vpc_id, ipv6)
}

/// One side of a reported peering connection, as the response wrote it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Side {
    vpc_id: String,
    owner_id: String,
    region: String,
    cidr_block: Option<String>,
    cidr_block_set: Vec<String>,
    ipv6_cidr_block_set: Vec<String>,
}

/// One reported peering connection, as the response wrote it — read through the XML tree, so
/// a member of a side is never confused with the identically-named member of the other.
#[derive(Debug, Clone, Default)]
struct Connection {
    id: String,
    status_code: String,
    status_message: Option<String>,
    expiration_time: Option<String>,
    requester: Option<Side>,
    accepter: Option<Side>,
    tags: Vec<(String, String)>,
}

/// The side `name` of `node`, or `None` when the connection carries none.
fn side(node: roxmltree::Node<'_, '_>, name: &str) -> Option<Side> {
    let info = child(node, name)?;
    let blocks = |set: &str, member: &str| -> Vec<String> {
        set_items(info, set)
            .into_iter()
            .filter_map(|item| child_text(item, member))
            .collect()
    };
    Some(Side {
        vpc_id: child_text(info, "vpcId").unwrap_or_default(),
        owner_id: child_text(info, "ownerId").unwrap_or_default(),
        region: child_text(info, "region").unwrap_or_default(),
        cidr_block: child_text(info, "cidrBlock"),
        cidr_block_set: blocks("cidrBlockSet", "cidrBlock"),
        ipv6_cidr_block_set: blocks("ipv6CidrBlockSet", "ipv6CidrBlock"),
    })
}

/// Every peering connection a response carries, in the order it reported them: the one a
/// create answers, or each item of a describe's set.
fn connections(case: &str, body: &str) -> Vec<Connection> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "vpcPeeringConnectionId").is_some())
        .map(|node| {
            let status = child(node, "status");
            Connection {
                id: child_text(node, "vpcPeeringConnectionId").unwrap_or_default(),
                status_code: status
                    .and_then(|status| child_text(status, "code"))
                    .unwrap_or_default(),
                status_message: status.and_then(|status| child_text(status, "message")),
                expiration_time: child_text(node, "expirationTime"),
                requester: side(node, "requesterVpcInfo"),
                accepter: side(node, "accepterVpcInfo"),
                tags: set_items(node, "tagSet")
                    .into_iter()
                    .map(|item| {
                        (
                            child_text(item, "key").unwrap_or_default(),
                            child_text(item, "value").unwrap_or_default(),
                        )
                    })
                    .collect(),
            }
        })
        .collect()
}

/// The ONE connection a response carries, or a panic naming the case.
fn one_connection(case: &str, body: &str) -> Connection {
    let reported = connections(case, body);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly one peering connection, got {reported:?}"
    );
    reported.into_iter().next().unwrap_or_default()
}

/// The connection `id` names, read back through the describe.
async fn described(router: &MultiServiceRouter, case: &str, id: &str) -> Connection {
    let params = indexed("VpcPeeringConnectionId", &[id]);
    let (status, body) = describe(router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    one_connection(case, &body)
}

/// The ids a successful describe reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    connections(case, &body)
        .into_iter()
        .map(|connection| connection.id)
        .collect()
}

/// The form fields that ask a create for `tags` on the connection it records, as one
/// `TagSpecification` naming the peering connection resource type.
fn tag_params(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        "vpc-peering-connection",
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push(param(
            &format!("TagSpecification.1.Tag.{position}.Key"),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    params
}

/// A tag, as the seeded fixtures carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// Seconds since the epoch, for comparing a reported `ExpirationTime` against the span the
/// operation documents.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account has requested no peering, so the operation reports an EMPTY set — not a
/// fabricated one — and describing is a READ: two calls report the same thing and the store
/// is untouched.
#[tokio::test]
async fn a_fresh_account_reports_no_peering_connection_and_reading_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, first) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {first}"
    );
    let first_ids: Vec<String> = connections("fresh account", &first)
        .into_iter()
        .map(|connection| connection.id)
        .collect();
    assert!(
        first_ids.is_empty(),
        "fresh account: expected no peering connection, got {first_ids:?}"
    );
    assert!(
        first.contains("vpcPeeringConnectionSet"),
        "fresh account: expected a vpcPeeringConnectionSet element, got {first}"
    );

    let second_ids = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second_ids, first_ids,
        "a read must not change what a later caller sees: {first_ids:?} then {second_ids:?}"
    );
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .vpc_peering_connections
            .is_empty(),
        "describing peering connections must not write to the store"
    );
}

// ---------------------------------------------------------------------------
// CreateVpcPeeringConnection, and the round trip through the describe
// ---------------------------------------------------------------------------

/// One row of the create table: which accepter VPC, whose account, which Region and which
/// tags.
struct CreateCase {
    name: &'static str,
    /// The requester VPC's IPv4 block; a VPC is created for the row with it.
    requester_cidr: &'static str,
    /// The accepter VPC's IPv4 block, when the accepter is a VPC of this account.
    accepter_cidr: Option<&'static str>,
    /// The account the request names as the accepter's owner, if any.
    peer_owner_id: Option<&'static str>,
    /// The Region the request names for the accepter VPC, if any.
    peer_region: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

/// Each create records one request between the two VPCs it names, answers it
/// `initiating-request` with the message and the expiry the operation documents, reports the
/// REQUESTER's address space and not the accepter's, and
/// `DescribeVpcPeeringConnections` reads every one of them back unchanged. The rows differ in
/// the accepter VPC, the account that owns it, its Region and how many tags the request asks
/// for.
#[tokio::test]
async fn a_created_peering_request_round_trips_through_the_describe() {
    assert_ne!(
        OTHER_ACCOUNT,
        engine::emulator_account_id(),
        "the cross-account rows need an account id this emulator does not serve"
    );
    let (router, _engine) = super::ec2_query::emulator();

    let cases = vec![
        CreateCase {
            name: "a VPC of this account, no tags",
            requester_cidr: "10.1.0.0/16",
            accepter_cidr: Some("10.2.0.0/16"),
            peer_owner_id: None,
            peer_region: None,
            tags: Vec::new(),
        },
        CreateCase {
            name: "a VPC of this account, two tags",
            requester_cidr: "10.3.0.0/16",
            accepter_cidr: Some("10.4.0.0/16"),
            peer_owner_id: None,
            peer_region: None,
            tags: vec![("Name", "east-west"), ("Owner", "TeamA")],
        },
        CreateCase {
            name: "a VPC of this account named with this account explicitly",
            requester_cidr: "10.5.0.0/16",
            accepter_cidr: Some("10.6.0.0/16"),
            peer_owner_id: Some(engine::emulator_account_id()),
            peer_region: None,
            tags: vec![("Note", "zone A/B, primary")],
        },
        CreateCase {
            name: "a VPC of this account in another Region",
            requester_cidr: "10.7.0.0/16",
            accepter_cidr: Some("10.8.0.0/16"),
            peer_owner_id: None,
            peer_region: Some(OTHER_REGION),
            tags: Vec::new(),
        },
        CreateCase {
            name: "a VPC of another account",
            requester_cidr: "10.9.0.0/16",
            accepter_cidr: None,
            peer_owner_id: Some(OTHER_ACCOUNT),
            peer_region: None,
            tags: vec![("Name", "cross-account")],
        },
        CreateCase {
            name: "a VPC of another account in another Region",
            requester_cidr: "10.10.0.0/16",
            accepter_cidr: None,
            peer_owner_id: Some(OTHER_ACCOUNT),
            peer_region: Some(OTHER_REGION),
            tags: Vec::new(),
        },
    ];

    let mut recorded: Vec<String> = Vec::new();
    for case in &cases {
        let requester_vpc_id = create_vpc(&router, case.requester_cidr).await;
        let (peer_vpc_id, peer_cidr) = match case.accepter_cidr {
            Some(cidr) => (create_vpc(&router, cidr).await, Some(cidr)),
            None => (OTHER_ACCOUNT_VPC_ID.to_string(), None),
        };
        let mut params = vec![
            param("VpcId", &requester_vpc_id),
            param("PeerVpcId", &peer_vpc_id),
        ];
        if let Some(owner) = case.peer_owner_id {
            params.push(param("PeerOwnerId", owner));
        }
        if let Some(region) = case.peer_region {
            params.push(param("PeerRegion", region));
        }
        params.extend(tag_params(&case.tags));

        let asked_at = SystemTime::now();
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with body {body}",
            name = case.name
        );
        let answered = one_connection(case.name, &body);

        assert!(
            is_long_id(VPC_PEERING_CONNECTION_ID_PREFIX, &answered.id),
            "case {name:?}: expected a {VPC_PEERING_CONNECTION_ID_PREFIX}-<17 hex> id, got {id:?}",
            name = case.name,
            id = answered.id,
        );
        assert_eq!(
            answered.status_code, VPC_PEERING_CONNECTION_STATE_INITIATING_REQUEST,
            "case {name:?}: expected a request EC2 has just taken, got {answered:?}",
            name = case.name
        );
        let expected_owner = case
            .peer_owner_id
            .unwrap_or_else(engine::emulator_account_id)
            .to_string();
        assert_eq!(
            answered.status_message,
            Some(vpc_peering_initiating_request_message(&expected_owner)),
            "case {name:?}: expected the status to name the accepter's account, got {answered:?}",
            name = case.name
        );
        // The WORDING itself, written out here rather than taken from the handler's own
        // helper: EC2 words a fresh request's status "Initiating Request to <the account the
        // request goes to>", with only the account coming from the request.
        assert_eq!(
            answered.status_message,
            Some(format!("Initiating Request to {expected_owner}")),
            "case {name:?}: expected EC2's wording for a request it has just taken, got \
             {answered:?}",
            name = case.name
        );

        // "The requester VPC and accepter VPC" the peering is between, each with the account
        // that owns it and the Region it is in.
        assert_eq!(
            answered.requester,
            Some(Side {
                vpc_id: requester_vpc_id.clone(),
                owner_id: engine::emulator_account_id().to_string(),
                region: REGION.to_string(),
                cidr_block: Some(case.requester_cidr.to_string()),
                cidr_block_set: vec![case.requester_cidr.to_string()],
                ipv6_cidr_block_set: Vec::new(),
            }),
            "case {name:?}: expected the requester side to report this account's VPC and its \
             ranges, got {answered:?}",
            name = case.name
        );
        assert_eq!(
            answered.accepter,
            Some(Side {
                vpc_id: peer_vpc_id.clone(),
                owner_id: expected_owner.clone(),
                region: case.peer_region.unwrap_or(REGION).to_string(),
                // "CIDR block information is only returned when describing an active VPC
                // peering connection": a request waiting to be accepted reports none, even
                // for an accepter VPC of this very account.
                cidr_block: None,
                cidr_block_set: Vec::new(),
                ipv6_cidr_block_set: Vec::new(),
            }),
            "case {name:?}: expected the accepter side to report the ids alone, got {answered:?}",
            name = case.name
        );
        assert!(
            peer_cidr.is_none_or(|cidr| !body.contains(cidr)),
            "case {name:?}: the accepter's range must not be reported before the connection \
             is active, got {body}",
            name = case.name
        );

        // "The VPC peering connection request expires after 7 days" — the span the
        // operation documents, from the moment the request was made.
        let expiration = answered
            .expiration_time
            .as_deref()
            .and_then(as_timestamp)
            .unwrap_or_else(|| {
                panic!(
                    "case {name:?}: expected an expirationTime, got {answered:?}",
                    name = case.name
                )
            });
        let expected = epoch_seconds(asked_at) + VPC_PEERING_CONNECTION_REQUEST_LIFETIME.as_secs_f64();
        assert!(
            (expiration - expected).abs() < 60.0,
            "case {name:?}: expected the request to expire about {expected} seconds into the \
             epoch, got {expiration}",
            name = case.name
        );

        let expected_tags: Vec<(String, String)> = case
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            answered.tags, expected_tags,
            "case {name:?}: expected the tags the request asked for, got {answered:?}",
            name = case.name
        );

        // The round trip: the describe reports the connection the create answered, member
        // for member.
        let read_back = described(&router, case.name, &answered.id).await;
        assert_eq!(
            read_back.id, answered.id,
            "case {name:?}: expected the describe to report the created connection, got \
             {read_back:?}",
            name = case.name
        );
        assert_eq!(
            read_back.requester, answered.requester,
            "case {name:?}: expected the describe to report the same requester side, got \
             {read_back:?}",
            name = case.name
        );
        assert_eq!(
            read_back.accepter, answered.accepter,
            "case {name:?}: expected the describe to report the same accepter side, got \
             {read_back:?}",
            name = case.name
        );
        assert_eq!(
            read_back.tags, answered.tags,
            "case {name:?}: expected the describe to report the same tags, got {read_back:?}",
            name = case.name
        );
        assert_eq!(
            read_back.expiration_time, answered.expiration_time,
            "case {name:?}: expected the describe to report the same expiry, got {read_back:?}",
            name = case.name
        );
        recorded.push(answered.id);
    }

    // Every request the table made is in the store, and the describe reports all of them —
    // the answer is computed from the store rather than fixed.
    let mut reported = described_ids(&router, "every request", &[]).await;
    reported.sort();
    let mut expected = recorded.clone();
    expected.sort();
    assert_eq!(
        reported, expected,
        "every request: expected every recorded connection, got {reported:?}"
    );
}

/// The move the clock owes a fresh request: `initiating-request` until
/// [`TRANSITION_DELAY`] has passed, `pending-acceptance` after it — "The owner of the
/// accepter VPC must accept the peering request to activate the peering connection". The
/// span is never zero, so a caller reading straight after the create still sees the
/// transient state.
#[tokio::test]
async fn the_clock_moves_a_fresh_request_to_pending_acceptance() {
    let (router, _engine) = super::ec2_query::emulator();
    let requester_vpc_id = create_vpc(&router, "10.30.0.0/16").await;
    let peer_vpc_id = create_vpc(&router, "10.31.0.0/16").await;

    let params = [
        param("VpcId", &requester_vpc_id),
        param("PeerVpcId", &peer_vpc_id),
    ];
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "the fresh request: expected HTTP 200, got {status} with body {body}"
    );
    let answered = one_connection("the fresh request", &body);
    assert_eq!(
        answered.status_code, VPC_PEERING_CONNECTION_STATE_INITIATING_REQUEST,
        "the fresh request: expected the transient state, got {answered:?}"
    );

    let straight_after = described(&router, "straight after the create", &answered.id).await;
    assert_eq!(
        straight_after.status_code, VPC_PEERING_CONNECTION_STATE_INITIATING_REQUEST,
        "straight after the create: expected the request still on its way in, got \
         {straight_after:?}"
    );

    tokio::time::sleep(past_the_transition()).await;
    let settled = described(&router, "past the transition", &answered.id).await;
    assert_eq!(
        settled.status_code, VPC_PEERING_CONNECTION_STATE_PENDING_ACCEPTANCE,
        "past the transition: expected the request to be waiting for the accepter, got \
         {settled:?}"
    );
    assert_eq!(
        settled.status_message,
        Some(vpc_peering_pending_acceptance_message(
            engine::emulator_account_id()
        )),
        "past the transition: expected the status to name the account that must accept, got \
         {settled:?}"
    );
    // The wording itself, as EC2 words a request waiting for its accepter.
    assert_eq!(
        settled.status_message,
        Some(format!(
            "Pending Acceptance by {account}",
            account = engine::emulator_account_id()
        )),
        "past the transition: expected EC2's wording for a waiting request, got {settled:?}"
    );
    assert_eq!(
        settled.expiration_time, answered.expiration_time,
        "past the transition: the request's expiry must not move with its status, got \
         {settled:?}"
    );
    assert_eq!(
        settled.requester, straight_after.requester,
        "past the transition: the move must not change the sides, got {settled:?}"
    );
}

// ---------------------------------------------------------------------------
// Overlapping address space
// ---------------------------------------------------------------------------

/// One row of the overlap table: two ranges, and whether they meet.
struct OverlapCase {
    name: &'static str,
    requester_cidr: &'static str,
    /// The accepter's range, or `None` to peer the requester VPC with ITSELF — a VPC's own
    /// range meets its own.
    accepter_cidr: Option<&'static str>,
    overlapping: bool,
}

/// "If you create a VPC peering connection request between VPCs with overlapping CIDR
/// blocks, the VPC peering connection has a status of failed"
/// (`CreateVpcPeeringConnection`), and a failed request has nothing left to accept: it
/// carries no expiry and the clock owes it no move. The rows differ in how the two ranges
/// meet — identically, one inside the other, one VPC with itself — and in not meeting at all.
#[tokio::test]
async fn overlapping_cidr_blocks_record_a_failed_request() {
    let (router, _engine) = super::ec2_query::emulator();

    let cases = vec![
        OverlapCase {
            name: "the same range on both sides",
            requester_cidr: "10.40.0.0/16",
            accepter_cidr: Some("10.40.0.0/16"),
            overlapping: true,
        },
        OverlapCase {
            name: "the accepter's range inside the requester's",
            requester_cidr: "10.41.0.0/16",
            accepter_cidr: Some("10.41.5.0/24"),
            overlapping: true,
        },
        OverlapCase {
            name: "the requester's range inside the accepter's",
            requester_cidr: "10.42.7.0/24",
            accepter_cidr: Some("10.42.0.0/16"),
            overlapping: true,
        },
        OverlapCase {
            name: "a VPC with itself",
            requester_cidr: "10.43.0.0/16",
            accepter_cidr: None,
            overlapping: true,
        },
        OverlapCase {
            name: "two ranges that do not meet",
            requester_cidr: "10.44.0.0/16",
            accepter_cidr: Some("10.45.0.0/16"),
            overlapping: false,
        },
        OverlapCase {
            name: "two ranges in different private blocks",
            requester_cidr: "172.20.0.0/16",
            accepter_cidr: Some("192.168.8.0/24"),
            overlapping: false,
        },
        OverlapCase {
            name: "two adjoining ranges",
            requester_cidr: "10.46.0.0/17",
            accepter_cidr: Some("10.46.128.0/17"),
            overlapping: false,
        },
    ];

    for case in &cases {
        let requester_vpc_id = create_vpc(&router, case.requester_cidr).await;
        let peer_vpc_id = match case.accepter_cidr {
            Some(cidr) => create_vpc(&router, cidr).await,
            None => requester_vpc_id.clone(),
        };
        let params = [
            param("VpcId", &requester_vpc_id),
            param("PeerVpcId", &peer_vpc_id),
        ];
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with body {body}",
            name = case.name
        );
        let answered = one_connection(case.name, &body);

        let expected_code = if case.overlapping {
            VPC_PEERING_CONNECTION_STATE_FAILED
        } else {
            VPC_PEERING_CONNECTION_STATE_INITIATING_REQUEST
        };
        assert_eq!(
            answered.status_code, expected_code,
            "case {name:?}: expected the status {expected_code}, got {answered:?}",
            name = case.name
        );
        if case.overlapping {
            assert_eq!(
                answered.status_message,
                Some(VPC_PEERING_CONNECTION_FAILED_MESSAGE.to_string()),
                "case {name:?}: expected the failure to say why, got {answered:?}",
                name = case.name
            );
            assert_eq!(
                answered.expiration_time, None,
                "case {name:?}: a failed request has nothing to accept, so it expires at no \
                 instant, got {answered:?}",
                name = case.name
            );
        } else {
            assert!(
                answered.expiration_time.is_some(),
                "case {name:?}: expected a request that can still be accepted to carry its \
                 expiry, got {answered:?}",
                name = case.name
            );
        }

        // The clock owes a failed request nothing: past the span a fresh request moves in,
        // it is still failed.
        tokio::time::sleep(past_the_transition()).await;
        let later = described(&router, case.name, &answered.id).await;
        let expected_later = if case.overlapping {
            VPC_PEERING_CONNECTION_STATE_FAILED
        } else {
            VPC_PEERING_CONNECTION_STATE_PENDING_ACCEPTANCE
        };
        assert_eq!(
            later.status_code, expected_later,
            "case {name:?}: expected the status {expected_later} once the span had passed, got \
             {later:?}",
            name = case.name
        );
    }
}

// ---------------------------------------------------------------------------
// One connection per pair
// ---------------------------------------------------------------------------

/// Two VPCs are peered by at most one connection: a second request for a pair a connection
/// still holds is refused whichever way round the pair is named, while a pair whose only
/// connection FAILED may be requested again.
#[tokio::test]
async fn a_second_request_for_a_peered_pair_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let first_vpc_id = create_vpc(&router, "10.50.0.0/16").await;
    let second_vpc_id = create_vpc(&router, "10.51.0.0/16").await;

    let (status, body) = create(
        &router,
        &[
            param("VpcId", &first_vpc_id),
            param("PeerVpcId", &second_vpc_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the first request: expected HTTP 200, got {status} with body {body}"
    );
    let first = one_connection("the first request", &body);

    for (case, vpc_id, peer_vpc_id) in [
        ("the same pair again", &first_vpc_id, &second_vpc_id),
        ("the same pair reversed", &second_vpc_id, &first_vpc_id),
    ] {
        let (status, body) = create(
            &router,
            &[param("VpcId", vpc_id), param("PeerVpcId", peer_vpc_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            VPC_PEERING_CONNECTION_ALREADY_EXISTS,
            "case {case:?}: expected the duplicate code, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(&first.id)
                && message.contains(vpc_id.as_str())
                && message.contains(peer_vpc_id.as_str()),
            "case {case:?}: expected the refusal to name the connection and both VPCs, got \
             {message:?}"
        );
    }

    assert_eq!(
        engine
            .state::<Ec2State>()
            .lock()
            .vpc_peering_connections
            .len(),
        1,
        "a refused duplicate must store nothing"
    );

    // A pair whose request FAILED is not peered, so it may be requested again — the rule is
    // about the connections that still hold the pair, not about every connection there has
    // ever been.
    let overlapping_vpc_id = create_vpc(&router, "10.52.0.0/16").await;
    let overlapping_peer_id = create_vpc(&router, "10.52.0.0/16").await;
    let mut failed_ids: Vec<String> = Vec::new();
    for case in ["the first failed request", "the second failed request"] {
        let (status, body) = create(
            &router,
            &[
                param("VpcId", &overlapping_vpc_id),
                param("PeerVpcId", &overlapping_peer_id),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let answered = one_connection(case, &body);
        assert_eq!(
            answered.status_code, VPC_PEERING_CONNECTION_STATE_FAILED,
            "case {case:?}: expected a failed request, got {answered:?}"
        );
        failed_ids.push(answered.id);
    }
    assert_ne!(
        failed_ids[0], failed_ids[1],
        "a second request for a pair nothing peers records a second connection, got \
         {failed_ids:?}"
    );
}

// ---------------------------------------------------------------------------
// What a create refuses
// ---------------------------------------------------------------------------

/// One row of the refusal table: the members the request carries, and the code it earns.
struct RefusalCase {
    name: &'static str,
    /// `None` where the row uses the VPC this test created.
    vpc_id: Option<&'static str>,
    peer_vpc_id: Option<&'static str>,
    peer_owner_id: Option<&'static str>,
    peer_region: Option<&'static str>,
    code: &'static str,
}

/// A request this surface cannot serve is refused with the code EC2 answers for that
/// member, and stores nothing. The rows differ in WHICH member is wrong and in the shape of
/// the value: an absent accepter VPC, one this account holds no VPC for, one that is no VPC
/// id at all, an account id that is not twelve digits and a Region that is no Region.
#[tokio::test]
async fn a_request_this_surface_cannot_serve_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let requester_vpc_id = create_vpc(&router, "10.60.0.0/16").await;

    let cases = vec![
        RefusalCase {
            name: "no accepter VPC at all",
            vpc_id: None,
            peer_vpc_id: None,
            peer_owner_id: None,
            peer_region: None,
            code: "MissingParameter",
        },
        RefusalCase {
            name: "an empty accepter VPC id",
            vpc_id: None,
            peer_vpc_id: Some(""),
            peer_owner_id: None,
            peer_region: None,
            code: "MissingParameter",
        },
        RefusalCase {
            name: "a requester VPC this account does not hold",
            vpc_id: Some("vpc-00000000000000009"),
            peer_vpc_id: Some(OTHER_ACCOUNT_VPC_ID),
            peer_owner_id: Some(OTHER_ACCOUNT),
            peer_region: None,
            code: "InvalidVpcID.NotFound",
        },
        RefusalCase {
            name: "an accepter VPC of this account that it does not hold",
            vpc_id: None,
            peer_vpc_id: Some("vpc-00000000000000008"),
            peer_owner_id: None,
            peer_region: None,
            code: "InvalidVpcID.NotFound",
        },
        RefusalCase {
            name: "an accepter VPC of another account that is no VPC id",
            vpc_id: None,
            peer_vpc_id: Some("not-a-vpc"),
            peer_owner_id: Some(OTHER_ACCOUNT),
            peer_region: None,
            code: "InvalidVpcID.Malformed",
        },
        RefusalCase {
            name: "an accepter VPC of this account that is no VPC id",
            vpc_id: None,
            peer_vpc_id: Some("vpc-nothex"),
            peer_owner_id: None,
            peer_region: None,
            code: "InvalidVpcID.Malformed",
        },
        RefusalCase {
            name: "a requester VPC that is no VPC id",
            vpc_id: Some("my-vpc"),
            peer_vpc_id: Some(OTHER_ACCOUNT_VPC_ID),
            peer_owner_id: Some(OTHER_ACCOUNT),
            peer_region: None,
            code: "InvalidVpcID.Malformed",
        },
        RefusalCase {
            name: "a requester VPC id of another resource's kind",
            vpc_id: Some("subnet-00000000000000007"),
            peer_vpc_id: Some(OTHER_ACCOUNT_VPC_ID),
            peer_owner_id: Some(OTHER_ACCOUNT),
            peer_region: None,
            code: "InvalidVpcID.Malformed",
        },
        RefusalCase {
            name: "an accepter owner that is not twelve digits",
            vpc_id: None,
            peer_vpc_id: Some(OTHER_ACCOUNT_VPC_ID),
            peer_owner_id: Some("12345"),
            peer_region: None,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "an accepter owner that is not digits at all",
            vpc_id: None,
            peer_vpc_id: Some(OTHER_ACCOUNT_VPC_ID),
            peer_owner_id: Some("my-other-account"),
            peer_region: None,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "an accepter Region no Region code names",
            vpc_id: None,
            peer_vpc_id: Some(OTHER_ACCOUNT_VPC_ID),
            peer_owner_id: Some(OTHER_ACCOUNT),
            peer_region: Some("us-west-9"),
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "an accepter Region that is not a Region code at all",
            vpc_id: None,
            peer_vpc_id: Some(OTHER_ACCOUNT_VPC_ID),
            peer_owner_id: Some(OTHER_ACCOUNT),
            peer_region: Some("somewhere else"),
            code: "InvalidParameterValue",
        },
    ];

    for case in &cases {
        let mut params = vec![param("VpcId", case.vpc_id.unwrap_or(&requester_vpc_id))];
        if let Some(peer_vpc_id) = case.peer_vpc_id {
            params.push(param("PeerVpcId", peer_vpc_id));
        }
        if let Some(owner) = case.peer_owner_id {
            params.push(param("PeerOwnerId", owner));
        }
        if let Some(region) = case.peer_region {
            params.push(param("PeerRegion", region));
        }

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}",
            name = case.name
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {name:?}: expected the code {code}, got {body}",
            name = case.name,
            code = case.code
        );
        assert!(
            engine
                .state::<Ec2State>()
                .lock()
                .vpc_peering_connections
                .is_empty(),
            "case {name:?}: a refused request must store nothing",
            name = case.name
        );
    }
}

/// `DryRun` reports the outcome of a request that would have succeeded and writes nothing —
/// "If you have the required permissions, the error response is DryRunOperation" — on the
/// create and on the describe alike.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let requester_vpc_id = create_vpc(&router, "10.70.0.0/16").await;
    let peer_vpc_id = create_vpc(&router, "10.71.0.0/16").await;

    for (case, action, params) in [
        (
            "the create",
            "CreateVpcPeeringConnection",
            vec![
                param("VpcId", &requester_vpc_id),
                param("PeerVpcId", &peer_vpc_id),
                param("DryRun", "true"),
            ],
        ),
        (
            "the describe",
            "DescribeVpcPeeringConnections",
            vec![param("DryRun", "true")],
        ),
    ] {
        let (status, body) = call(&router, REGION, action, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected the dry-run code, got {body}"
        );
    }
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .vpc_peering_connections
            .is_empty(),
        "a dry run must store nothing"
    );
}

// ---------------------------------------------------------------------------
// An active connection reports both sides' ranges
// ---------------------------------------------------------------------------

/// "CIDR block information is only returned when describing an active VPC peering
/// connection" (`VpcPeeringConnection$AccepterVpcInfo`): once the connection is active the
/// accepter's ranges are reported too, read off the VPC the store holds — every IPv4 block
/// it holds and every IPv6 block, so a dual-stack VPC reports both families.
#[tokio::test]
async fn an_active_connection_reports_both_sides_ranges() {
    let (router, engine) = super::ec2_query::emulator();
    let (requester_vpc_id, requester_ipv6) =
        create_dual_stack_vpc(&router, "10.80.0.0/16").await;
    let peer_vpc_id = create_vpc(&router, "10.81.0.0/16").await;

    let (status, body) = create(
        &router,
        &[
            param("VpcId", &requester_vpc_id),
            param("PeerVpcId", &peer_vpc_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the request: expected HTTP 200, got {status} with body {body}"
    );
    let answered = one_connection("the request", &body);

    // `AcceptVpcPeeringConnection` is still a scaffold stub with an issue of its own, so the
    // active connection is seeded through the store — the same way this crate's other suites
    // seed a state a create never mints.
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let record = guard
            .vpc_peering_connections
            .get_mut(&answered.id)
            .expect("the connection just created is stored");
        record.view.status = Some(VpcPeeringConnectionStateReason {
            code: Some(VPC_PEERING_CONNECTION_STATE_ACTIVE.to_string()),
            message: None,
        });
        // An accepted request is no longer waiting, so it carries no expiry.
        record.view.expiration_time = None;
        record.pending = None;
    }

    let active = described(&router, "the active connection", &answered.id).await;
    assert_eq!(
        active.status_code, VPC_PEERING_CONNECTION_STATE_ACTIVE,
        "the active connection: expected the seeded status, got {active:?}"
    );
    assert_eq!(
        active.accepter,
        Some(Side {
            vpc_id: peer_vpc_id.clone(),
            owner_id: engine::emulator_account_id().to_string(),
            region: REGION.to_string(),
            cidr_block: Some("10.81.0.0/16".to_string()),
            cidr_block_set: vec!["10.81.0.0/16".to_string()],
            ipv6_cidr_block_set: Vec::new(),
        }),
        "the active connection: expected the accepter's ranges to be reported, got {active:?}"
    );
    assert_eq!(
        active.requester,
        Some(Side {
            vpc_id: requester_vpc_id.clone(),
            owner_id: engine::emulator_account_id().to_string(),
            region: REGION.to_string(),
            cidr_block: Some("10.80.0.0/16".to_string()),
            cidr_block_set: vec!["10.80.0.0/16".to_string()],
            ipv6_cidr_block_set: vec![requester_ipv6.clone()],
        }),
        "the active connection: expected the requester's IPv4 and IPv6 ranges, got {active:?}"
    );

    // The ranges are READ off the VPC rather than copied onto the peering when it was
    // requested: a block the VPC takes afterwards is reported by the next describe.
    let late_block = "10.82.0.0/16";
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let association_id = guard.next_id("vpc-cidr-assoc");
        let vpc = guard
            .vpcs
            .get_mut(&peer_vpc_id)
            .expect("the accepter VPC is stored");
        vpc.cidr_block_association_set
            .as_mut()
            .expect("a stored VPC holds its IPv4 associations")
            .push(ec2::ops::types::VpcCidrBlockAssociation {
                association_id: Some(association_id),
                cidr_block: Some(late_block.to_string()),
                cidr_block_state: Some(ec2::ops::types::VpcCidrBlockState {
                    state: Some("associated".to_string()),
                    status_message: None,
                }),
            });
    }
    let after = described(&router, "after a second block", &answered.id).await;
    assert_eq!(
        after.accepter.as_ref().map(|side| side.cidr_block_set.clone()),
        Some(vec!["10.81.0.0/16".to_string(), late_block.to_string()]),
        "after a second block: expected every block the VPC holds, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// An unaccepted request expires
// ---------------------------------------------------------------------------

/// "The VPC peering connection request expires after 7 days, after which it cannot be
/// accepted or rejected" (`CreateVpcPeeringConnection`): a request whose own
/// `ExpirationTime` has passed is reported `expired`, and one made inside that span is not.
/// The span is read off the instant the request was made, so a request seeded older than it
/// is already expired.
#[tokio::test]
async fn a_request_past_its_expiration_time_is_reported_expired() {
    let (router, engine) = super::ec2_query::emulator();
    let requester_vpc_id = create_vpc(&router, "10.90.0.0/16").await;
    let fresh_peer_id = create_vpc(&router, "10.91.0.0/16").await;
    let stale_peer_id = create_vpc(&router, "10.92.0.0/16").await;

    let cases = [
        ("a request made a day ago", Duration::from_secs(24 * 60 * 60), false),
        (
            "a request made a day past its lifetime",
            VPC_PEERING_CONNECTION_REQUEST_LIFETIME + Duration::from_secs(24 * 60 * 60),
            true,
        ),
    ];
    let mut ids: Vec<(&str, String, bool)> = Vec::new();
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        for (index, (case, age, expired)) in cases.iter().enumerate() {
            let peer_vpc_id = if index == 0 {
                fresh_peer_id.clone()
            } else {
                stale_peer_id.clone()
            };
            let id = guard.insert_vpc_peering_connection(
                VpcPeeringConnectionSpec {
                    vpc_id: requester_vpc_id.clone(),
                    peer_vpc_id,
                    peer_owner_id: engine::emulator_account_id().to_string(),
                    region: REGION.to_string(),
                    peer_region: REGION.to_string(),
                    tags: Vec::new(),
                },
                SystemTime::now() - *age,
            );
            ids.push((case, id, *expired));
        }
    }

    for (case, id, expired) in &ids {
        let reported = described(&router, case, id).await;
        if *expired {
            assert_eq!(
                reported.status_code, VPC_PEERING_CONNECTION_STATE_EXPIRED,
                "case {case:?}: expected a request past its expiry to be expired, got \
                 {reported:?}"
            );
            assert_eq!(
                reported.status_message, None,
                "case {case:?}: an expired request waits for nobody, so it carries no \
                 message, got {reported:?}"
            );
            assert!(
                !vpc_peering_connection_awaits_acceptance(Some(&reported.status_code)),
                "case {case:?}: an expired request can no longer be accepted, got {reported:?}"
            );
        } else {
            assert!(
                vpc_peering_connection_awaits_acceptance(Some(&reported.status_code)),
                "case {case:?}: expected a request inside its lifetime to be waiting, got \
                 {reported:?}"
            );
        }
        // Reading again reports the same thing: the expiry was owed at its instant, not
        // caused by the read.
        let again = described(&router, case, id).await;
        assert_eq!(
            again.status_code, reported.status_code,
            "case {case:?}: a read must not change what a later caller sees, got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The ids, the filters and the pages
// ---------------------------------------------------------------------------

/// What one seeded connection is written with, beyond the id and the requester account the
/// service mints itself.
struct SeedFixture {
    /// Which VPC of [`Seeded::vpc_ids`] requests the peering.
    requester_index: usize,
    /// Which VPC of [`Seeded::vpc_ids`] it asks to peer with.
    accepter_index: usize,
    /// The account the accepter VPC belongs to.
    peer_owner_id: &'static str,
    /// The Region the accepter VPC is in.
    peer_region: &'static str,
    /// The `VpcPeeringConnectionStateReasonCode` the connection is left in, or `None` to
    /// leave the state the insert gives a fresh request.
    status: Option<&'static str>,
    tags: Vec<Tag>,
}

/// What [`seed`] wrote: the VPCs the connections are between, their ranges, and the
/// connection ids in store order.
struct Seeded {
    vpc_ids: Vec<String>,
    cidr_blocks: Vec<&'static str>,
    ids: Vec<String>,
}

/// The connections the filter and paging tables are written against. They differ along every
/// attribute the documented filters select on: the requester VPC, the accepter VPC, the
/// accepter's account and Region, the status and the tags.
fn seeded_fixtures() -> Vec<SeedFixture> {
    vec![
        SeedFixture {
            requester_index: 0,
            accepter_index: 1,
            peer_owner_id: "",
            peer_region: REGION,
            status: None,
            tags: vec![tag("Name", "east-west"), tag("Owner", "TeamA")],
        },
        SeedFixture {
            requester_index: 0,
            accepter_index: 2,
            peer_owner_id: "",
            peer_region: REGION,
            status: Some(VPC_PEERING_CONNECTION_STATE_ACTIVE),
            tags: vec![tag("Name", "north-south")],
        },
        SeedFixture {
            requester_index: 1,
            accepter_index: 2,
            peer_owner_id: "",
            peer_region: OTHER_REGION,
            status: Some(VPC_PEERING_CONNECTION_STATE_PROVISIONING),
            tags: Vec::new(),
        },
        SeedFixture {
            requester_index: 1,
            accepter_index: 3,
            peer_owner_id: OTHER_ACCOUNT,
            peer_region: REGION,
            status: None,
            tags: vec![tag("Name", "east-west"), tag("Tier", "prod")],
        },
        SeedFixture {
            requester_index: 2,
            accepter_index: 3,
            peer_owner_id: OTHER_ACCOUNT,
            peer_region: OTHER_REGION,
            status: Some(VPC_PEERING_CONNECTION_STATE_FAILED),
            tags: vec![tag("Owner", "TeamB")],
        },
        SeedFixture {
            requester_index: 3,
            accepter_index: 0,
            peer_owner_id: "",
            peer_region: REGION,
            status: Some(VPC_PEERING_CONNECTION_STATE_EXPIRED),
            tags: Vec::new(),
        },
        SeedFixture {
            requester_index: 3,
            accepter_index: 1,
            peer_owner_id: "",
            peer_region: REGION,
            status: Some(VPC_PEERING_CONNECTION_STATE_ACTIVE),
            tags: vec![tag("Name", "east-west")],
        },
    ]
}

/// Write the seeded connections into the engine's store, the way
/// `CreateVpcPeeringConnection` would, and report what they were given. Seeding through the
/// store rather than the wire is what lets the filter table cover every status: a create
/// only ever records `initiating-request` or `failed`.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let cidr_blocks = vec!["10.100.0.0/16", "10.101.0.0/16", "10.102.0.0/16", "10.103.0.0/16"];
    let vpc_ids: Vec<String> = cidr_blocks
        .iter()
        .map(|cidr_block| guard.insert_vpc(cidr_block, false, "default", "available", REGION, Vec::new()))
        .collect();

    let ids = seeded_fixtures()
        .into_iter()
        .map(|fixture| {
            let id = guard.insert_vpc_peering_connection(
                VpcPeeringConnectionSpec {
                    vpc_id: vpc_ids[fixture.requester_index].clone(),
                    peer_vpc_id: vpc_ids[fixture.accepter_index].clone(),
                    peer_owner_id: if fixture.peer_owner_id.is_empty() {
                        engine::emulator_account_id().to_string()
                    } else {
                        fixture.peer_owner_id.to_string()
                    },
                    region: REGION.to_string(),
                    peer_region: fixture.peer_region.to_string(),
                    tags: fixture.tags,
                },
                SystemTime::now(),
            );
            if let Some(status) = fixture.status {
                let record = guard
                    .vpc_peering_connections
                    .get_mut(&id)
                    .expect("the connection just seeded is stored");
                record.view.status = Some(VpcPeeringConnectionStateReason {
                    code: Some(status.to_string()),
                    message: None,
                });
                // A connection nobody is waiting on carries no expiry, and the clock owes it
                // no move.
                if !vpc_peering_connection_awaits_acceptance(Some(status)) {
                    record.view.expiration_time = None;
                }
                record.pending = None;
            }
            id
        })
        .collect();

    Seeded {
        vpc_ids,
        cidr_blocks,
        ids,
    }
}

/// The ids of `seeded` at `indices`, which is what a row of the filter table names.
fn ids_at(seeded: &Seeded, indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| seeded.ids[*index].clone()).collect()
}

/// Every stored connection is reported, in store order; a connection written after a call
/// shows up in the next one.
#[tokio::test]
async fn every_stored_peering_connection_is_reported_in_store_order() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all connections", &[]).await;
    assert_eq!(
        reported, seeded.ids,
        "all connections: expected every stored connection in store order, got {reported:?}"
    );

    // Narrowing by id is narrowing of that same set, in that same order.
    for (case, indices) in [
        ("one id", vec![2]),
        ("two ids", vec![0, 4]),
        ("the ids out of order", vec![5, 1]),
        ("a repeated id", vec![3, 3]),
    ] {
        let named = ids_at(&seeded, &indices);
        let params = indexed(
            "VpcPeeringConnectionId",
            &named.iter().map(String::as_str).collect::<Vec<&str>>(),
        );
        let reported = described_ids(&router, case, &params).await;
        let mut expected: Vec<String> = named.clone();
        expected.sort();
        expected.dedup();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the connections named, got {reported:?}"
        );
    }

    // An id this account holds no connection for is an error, not an absent row.
    let params = indexed(
        "VpcPeeringConnectionId",
        &[seeded.ids[0].as_str(), "pcx-00000000000000009"],
    );
    let (status, body) = describe(&router, &params).await;
    assert_eq!(
        status, 400,
        "an unknown id: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidVpcPeeringConnectionID.NotFound",
        "an unknown id: expected the not-found code, got {body}"
    );

    // A value that is no peering connection id at all is a different mistake, refused
    // before anything is looked up. The rows differ in the shape of the unreadable value.
    for malformed in ["not-an-id", "pcx-", "pcx-nothexdigits0", "vpc-00000000000000001"] {
        let params = indexed("VpcPeeringConnectionId", &[malformed]);
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {malformed:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVpcPeeringConnectionID.Malformed",
            "case {malformed:?}: expected the malformed code, got {body}"
        );
    }
}

/// One row of the filter table: the filters a request carries, and which seeded connections
/// they select.
struct FilterCase {
    name: &'static str,
    /// The filters, built from the seeded fixtures so a row names no literal id.
    entries: Vec<(&'static str, Vec<String>)>,
    expected: Vec<usize>,
}

/// Every documented filter selects on the attribute it names, joined with AND across
/// filters and with OR within one. The rows differ in which filter, how many values, how
/// many filters and whether the value carries a wildcard.
#[tokio::test]
async fn the_documented_filters_select_the_connections_they_name() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();

    let cases = vec![
        FilterCase {
            name: "status-code, one value",
            entries: vec![("status-code", vec![VPC_PEERING_CONNECTION_STATE_ACTIVE.to_string()])],
            expected: vec![1, 6],
        },
        FilterCase {
            name: "status-code, two values",
            entries: vec![(
                "status-code",
                vec![
                    VPC_PEERING_CONNECTION_STATE_FAILED.to_string(),
                    VPC_PEERING_CONNECTION_STATE_EXPIRED.to_string(),
                ],
            )],
            expected: vec![4, 5],
        },
        FilterCase {
            name: "requester-vpc-info.vpc-id",
            entries: vec![(
                "requester-vpc-info.vpc-id",
                vec![seeded.vpc_ids[1].clone()],
            )],
            expected: vec![2, 3],
        },
        FilterCase {
            name: "accepter-vpc-info.vpc-id, two values",
            entries: vec![(
                "accepter-vpc-info.vpc-id",
                vec![seeded.vpc_ids[0].clone(), seeded.vpc_ids[3].clone()],
            )],
            expected: vec![3, 4, 5],
        },
        FilterCase {
            name: "accepter-vpc-info.owner-id, another account",
            entries: vec![(
                "accepter-vpc-info.owner-id",
                vec![OTHER_ACCOUNT.to_string()],
            )],
            expected: vec![3, 4],
        },
        FilterCase {
            name: "requester-vpc-info.owner-id, this account",
            entries: vec![("requester-vpc-info.owner-id", vec![account.clone()])],
            expected: vec![0, 1, 2, 3, 4, 5, 6],
        },
        FilterCase {
            name: "requester-vpc-info.cidr-block",
            entries: vec![(
                "requester-vpc-info.cidr-block",
                vec![seeded.cidr_blocks[0].to_string()],
            )],
            expected: vec![0, 1],
        },
        FilterCase {
            name: "accepter-vpc-info.cidr-block, reported only when active",
            entries: vec![(
                "accepter-vpc-info.cidr-block",
                vec![seeded.cidr_blocks[1].to_string()],
            )],
            expected: vec![6],
        },
        FilterCase {
            name: "status-message, the request's own wording",
            entries: vec![(
                "status-message",
                vec![format!("Initiating Request to {OTHER_ACCOUNT}")],
            )],
            expected: vec![3],
        },
        FilterCase {
            name: "status-message with a wildcard",
            entries: vec![("status-message", vec!["Initiating Request to *".to_string()])],
            expected: vec![0, 3],
        },
        FilterCase {
            name: "tag:Name",
            entries: vec![("tag:Name", vec!["east-west".to_string()])],
            expected: vec![0, 3, 6],
        },
        FilterCase {
            name: "tag:Name with a wildcard",
            entries: vec![("tag:Name", vec!["*south".to_string()])],
            expected: vec![1],
        },
        FilterCase {
            name: "tag-key",
            entries: vec![("tag-key", vec!["Owner".to_string()])],
            expected: vec![0, 4],
        },
        FilterCase {
            name: "vpc-peering-connection-id",
            entries: vec![(
                "vpc-peering-connection-id",
                vec![seeded.ids[2].clone(), seeded.ids[4].clone()],
            )],
            expected: vec![2, 4],
        },
        FilterCase {
            name: "two filters, joined with AND",
            entries: vec![
                ("tag:Name", vec!["east-west".to_string()]),
                (
                    "status-code",
                    vec![VPC_PEERING_CONNECTION_STATE_ACTIVE.to_string()],
                ),
            ],
            expected: vec![6],
        },
        FilterCase {
            name: "three filters, joined with AND",
            entries: vec![
                ("requester-vpc-info.owner-id", vec![account.clone()]),
                (
                    "accepter-vpc-info.owner-id",
                    vec![OTHER_ACCOUNT.to_string()],
                ),
                (
                    "status-code",
                    vec![VPC_PEERING_CONNECTION_STATE_FAILED.to_string()],
                ),
            ],
            expected: vec![4],
        },
        FilterCase {
            name: "a filter nothing matches",
            entries: vec![("tag:Name", vec!["no-such-peering".to_string()])],
            expected: Vec::new(),
        },
        FilterCase {
            name: "a filter carrying no value at all",
            entries: vec![("status-code", Vec::new())],
            expected: Vec::new(),
        },
    ];

    for case in &cases {
        let entries: Vec<(&str, Vec<&str>)> = case
            .entries
            .iter()
            .map(|(name, values)| (*name, values.iter().map(String::as_str).collect()))
            .collect();
        let params = filters(
            &entries
                .iter()
                .map(|(name, values)| (*name, values.as_slice()))
                .collect::<Vec<(&str, &[&str])>>(),
        );
        let reported = described_ids(&router, case.name, &params).await;
        assert_eq!(
            reported,
            ids_at(&seeded, &case.expected),
            "case {name:?}: expected the connections the filters name, got {reported:?}",
            name = case.name
        );
    }

    // `expiration-time` selects on the instant a request expires, compared in the ISO 8601
    // rendering the wire carries a modelled timestamp as. The instant is the store's, not a
    // literal: it is read back off the connection the seed made and then filtered on.
    let fresh = described(&router, "the fresh request", &seeded.ids[0]).await;
    let expiration = fresh
        .expiration_time
        .clone()
        .unwrap_or_else(|| panic!("a request awaiting acceptance carries an expiry: {fresh:?}"));
    let params = filters(&[("expiration-time", &[expiration.as_str()])]);
    let reported = described_ids(&router, "expiration-time", &params).await;
    assert!(
        reported.contains(&seeded.ids[0]),
        "expiration-time: expected the request that expires then, got {reported:?}"
    );
    for id in &reported {
        let connection = described(&router, "expiration-time", id).await;
        assert_eq!(
            connection.expiration_time,
            Some(expiration.clone()),
            "expiration-time: expected every selected connection to expire at that instant, \
             got {connection:?}"
        );
    }
    let params = filters(&[("expiration-time", &["2020-01-01T00:00:00Z"])]);
    let reported = described_ids(&router, "expiration-time, another instant", &params).await;
    assert!(
        reported.is_empty(),
        "expiration-time, another instant: expected no connection, got {reported:?}"
    );

    // A filter name the operation does not document is refused rather than read as "no
    // results".
    for undocumented in ["status", "tag:", "requester-vpc-info.cidr", "Status-Code"] {
        let params = filters(&[(undocumented, &["anything"])]);
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {undocumented:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {undocumented:?}: expected the invalid-filter code, got {body}"
        );
        assert!(
            error_message(&body).contains(undocumented),
            "case {undocumented:?}: expected the refusal to name the filter, got {body}"
        );
    }
}

/// The page size cuts the SAME ordered set into pages, and the token names the connection
/// the next page starts at — so following the tokens reports every connection exactly once,
/// whatever the page size.
#[tokio::test]
async fn the_pages_cut_the_reported_set_and_the_token_advances() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for page_size in [5usize, 6, 7, 1000] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let case = format!("page size {page_size}, page {pages}");
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = &token {
                params.push(param("NextToken", token));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page: Vec<String> = connections(&case, &body)
                .into_iter()
                .map(|connection| connection.id)
                .collect();
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} connections, got {page:?}"
            );
            collected.extend(page);
            token = super::ec2_query::element(&body, "nextToken");
            pages += 1;
            if token.is_none() || pages > seeded.ids.len() + 2 {
                break;
            }
        }
        assert_eq!(
            collected, seeded.ids,
            "page size {page_size}: expected every connection exactly once, in store order, \
             got {collected:?}"
        );
    }

    // A token this service did not mint names no page, so it is refused rather than read as
    // an id.
    for token in ["not-a-token", "cGN4LTk5OQ"] {
        let params = [param("NextToken", token)];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected a refusal of the token, got {body}"
        );
    }
}
