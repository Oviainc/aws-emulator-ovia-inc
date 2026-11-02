// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the transit-gateway-Connect-peer-association slice
//! (`AssociateTransitGatewayConnectPeer`, `GetTransitGatewayConnectPeerAssociations`,
//! `DisassociateTransitGatewayConnectPeer`), driven over HTTP.
//!
//! The rule under test: an association of this kind is a STORED tie between one Connect
//! peer of a transit gateway of Amazon VPC and one device — optionally one link of that
//! device — of one global network, identified by the global network plus the peer's ARN and
//! carrying no id of its own. The associate stores it `PENDING` and registers the peering
//! with the global network's telemetry, the clock settles it `AVAILABLE`; the reader
//! answers the stored ties of its global network and nothing else; the disassociate REMOVES
//! the stored tie, reports the removed one `DELETING`, deregisters the peering, and leaves
//! a later read with nothing to report, so a repeat of it is a not-found.
//!
//! The rule holds for every well-formed transit gateway Connect peer ARN and for every way
//! the optional link member can be spelled, not only the one shape a recording happens to
//! carry, so the table below spans the ARN's partition, Region and id width, a device that
//! sits at a site and one that sits at none, and the link member named, omitted and empty.
//! The ARN's account is fixed: a peer sits on a transit gateway registered with the global
//! network, and such a gateway belongs to the account that owns it.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! label/query bindings and the error envelopes are exercised as a client hits them.
//! Nothing reaches into `crate::state`: an association is only ever read back through
//! `GetTransitGatewayConnectPeerAssociations`, and the peering it registers through
//! `GetNetworkTelemetry`.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::{json, Value};

use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::resource_tagging::escape;
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};

/// The resource type a refusal names for an association, which has no id of its own.
pub(crate) const ASSOCIATION_KIND: &str = "transit-gateway-connect-peer-association";

/// The request member a refusal names when the Connect peer ARN is at fault: Network
/// Manager names the offending member in its own lowerCamelCase spelling.
const ARN_MEMBER: &str = "transitGatewayConnectPeerArn";

/// The ARN of a Connect peer of a transit gateway. A peer belongs to Amazon VPC, so the
/// form is the EC2 skeleton every EC2 ARN this model types spells out —
/// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$` for `VpcArn`
/// (`smithy_query shape=VpcArn`) — with the `transit-gateway-connect-peer` resource type and
/// a `tgw-connect-peer-` id; `digits` is how many hex digits that id carries, which that
/// pattern writes as 8 to 17.
pub(crate) fn connect_peer_arn(
    partition: &str,
    region: &str,
    account: &str,
    index: usize,
    digits: usize,
) -> String {
    format!(
        "arn:{partition}:ec2:{region}:{account}:transit-gateway-connect-peer/tgw-connect-peer-{index:0width$x}",
        width = digits
    )
}

/// How a row spells `AssociateTransitGatewayConnectPeerRequest$LinkId`: the modelled member
/// is optional and its pattern `^[\s\S]*$` admits the empty string, which names no link
/// (`smithy_query query="operation" operation="AssociateTransitGatewayConnectPeer"`).
enum LinkMember {
    /// The link the device is associated with.
    Associated,
    /// The member is absent from the request.
    Absent,
    /// The member is present and empty.
    Empty,
}

/// One row of the table: the association one `AssociateTransitGatewayConnectPeer` asks for.
struct Case {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The ARN of the Connect peer the row ties.
    arn: String,
    /// The Region the row's peer lives in, which is the Region its transit gateway is in.
    region: &'static str,
    /// The account that owns the row's peer, which owns its transit gateway too.
    account: String,
    /// Whether the row associates the device that sits at a site.
    sited_device: bool,
    /// How the row spells the optional `LinkId`.
    link: LinkMember,
}

/// The rows, spanning what the rule generalises over: the ARN's partition, Region and id
/// width, a device at a site and one with none, and all three spellings of the optional
/// link member.
fn cases() -> Vec<Case> {
    let account = own_account();
    vec![
        Case {
            name: "the shortest published id, over a link of the device's site",
            arn: connect_peer_arn("aws", "us-west-2", &account, 1, 8),
            region: "us-west-2",
            account: account.clone(),
            sited_device: true,
            link: LinkMember::Associated,
        },
        Case {
            name: "the longest published id in another Region, naming no link at all",
            arn: connect_peer_arn("aws", "eu-central-1", &account, 2, 17),
            region: "eu-central-1",
            account: account.clone(),
            sited_device: true,
            link: LinkMember::Absent,
        },
        Case {
            name: "another partition and Region, tied to a device that sits at no site",
            arn: connect_peer_arn("aws-cn", "cn-north-1", &account, 3, 12),
            region: "cn-north-1",
            account: account.clone(),
            sited_device: false,
            link: LinkMember::Absent,
        },
        Case {
            name: "a third Region, spelling the link member empty",
            arn: connect_peer_arn("aws", "ap-southeast-2", &account, 4, 10),
            region: "ap-southeast-2",
            account,
            sited_device: false,
            link: LinkMember::Empty,
        },
    ]
}

impl Case {
    /// The device this row associates.
    fn device(&self, fixture: &Fixture) -> String {
        if self.sited_device {
            fixture.sited_device.clone()
        } else {
            fixture.siteless_device.clone()
        }
    }

    /// The link this row expects the stored association to carry, or `None` where it named
    /// none.
    fn expected_link(&self, fixture: &Fixture) -> Option<String> {
        match self.link {
            LinkMember::Associated => Some(fixture.link.clone()),
            LinkMember::Absent | LinkMember::Empty => None,
        }
    }

    /// The `AssociateTransitGatewayConnectPeer` body this row sends.
    fn request(&self, fixture: &Fixture) -> Value {
        let mut body = json!({
            "TransitGatewayConnectPeerArn": self.arn,
            "DeviceId": self.device(fixture),
        });
        let object = body
            .as_object_mut()
            .unwrap_or_else(|| panic!("case {:?}: the request body must be an object", self.name));
        match self.link {
            LinkMember::Associated => {
                object.insert("LinkId".to_string(), json!(fixture.link));
            }
            LinkMember::Empty => {
                object.insert("LinkId".to_string(), json!(""));
            }
            LinkMember::Absent => {}
        }
        body
    }
}

/// A built fixture: a global network, a site, a device at that site and one with none, the
/// site's link (associated with the sited device) and a second link nothing is associated
/// with, plus one registered transit gateway per row of [`cases`] so every row's peer sits
/// on a gateway this global network holds a registration for.
struct Fixture {
    global_network: String,
    /// The device at a site, which is the only device a link can be associated with.
    sited_device: String,
    /// A device created with no site at all.
    siteless_device: String,
    /// The link associated with [`Self::sited_device`].
    link: String,
    /// A link of the same site that NO device is associated with.
    unassociated_link: String,
}

/// `AssociateTransitGatewayConnectPeer` on `network` with `body`.
pub(crate) async fn associate(emulator: &Emulator, network: &str, body: Value) -> (u16, Value) {
    emulator
        .post(
            &format!("/global-networks/{network}/transit-gateway-connect-peer-associations"),
            body,
        )
        .await
}

/// `DisassociateTransitGatewayConnectPeer` for `arn` on `network`. The ARN travels in a
/// NON-greedy `@httpLabel`, so a client sends its `:` and `/` percent-encoded.
pub(crate) async fn disassociate(emulator: &Emulator, network: &str, arn: &str) -> (u16, Value) {
    emulator
        .delete(&format!(
            "/global-networks/{network}/transit-gateway-connect-peer-associations/{}",
            escape(arn)
        ))
        .await
}

/// `GetTransitGatewayConnectPeerAssociations` on `network` with `query` appended to the URI.
pub(crate) async fn get_associations(
    emulator: &Emulator,
    network: &str,
    query: &str,
) -> (u16, Value) {
    let uri = if query.is_empty() {
        format!("/global-networks/{network}/transit-gateway-connect-peer-associations")
    } else {
        format!("/global-networks/{network}/transit-gateway-connect-peer-associations?{query}")
    };
    emulator.get(&uri).await
}

/// A read that must succeed, answering the whole response.
async fn get_associations_ok(emulator: &Emulator, case: &str, network: &str, query: &str) -> Value {
    let (status, body) = get_associations(emulator, network, query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetTransitGatewayConnectPeerAssociations expected 200, got {status} with {body}"
    );
    body
}

/// The `TransitGatewayConnectPeerArn`s one page lists, in the order returned.
pub(crate) fn arns_of(case: &str, body: &Value) -> Vec<String> {
    body["TransitGatewayConnectPeerAssociations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a TransitGatewayConnectPeerAssociations array in {body}")
        })
        .iter()
        .map(|association| {
            association["TransitGatewayConnectPeerArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no TransitGatewayConnectPeerArn in {association}")
                })
                .to_string()
        })
        .collect()
}

/// The one association the reader answers for `arn`, or `None` when it answers none.
async fn read_one(emulator: &Emulator, case: &str, network: &str, arn: &str) -> Option<Value> {
    let body = get_associations_ok(
        emulator,
        case,
        network,
        &format!("transitGatewayConnectPeerArns={}", escape(arn)),
    )
    .await;
    let page = body["TransitGatewayConnectPeerAssociations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a TransitGatewayConnectPeerAssociations array in {body}")
        })
        .clone();
    assert!(
        page.len() <= 1,
        "case {case:?}: one ARN selects at most one association, got {body}"
    );
    page.into_iter().next()
}

/// `RegisterTransitGateway` that must succeed.
pub(crate) async fn register_gateway(emulator: &Emulator, network: &str, arn: &str) {
    let (status, body) = emulator
        .post(
            &format!("/global-networks/{network}/transit-gateway-registrations"),
            json!({ "TransitGatewayArn": arn }),
        )
        .await;
    assert_eq!(
        status, 200,
        "RegisterTransitGateway({arn}): expected 200, got {status} with {body}"
    );
}

/// `GetNetworkTelemetry` for `arn` on `network`: the one entry it reports for that
/// connection, or `None` when it reports none.
async fn telemetry_of(emulator: &Emulator, case: &str, network: &str, arn: &str) -> Option<Value> {
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/network-telemetry?resourceArn={}",
            escape(arn)
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetNetworkTelemetry expected 200, got {status} with {body}"
    );
    body["NetworkTelemetry"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a NetworkTelemetry array in {body}"))
        .first()
        .cloned()
}

/// Build a fixture named `name`. `base` offsets the transit gateway ids it registers, so two
/// fixtures of one emulator register different gateways — a gateway is registered with at
/// most one global network.
async fn fixture(emulator: &Emulator, name: &str, base: usize) -> Fixture {
    let global_network = emulator.create_global_network(name).await;
    let site = emulator.create_site(&global_network, name).await;
    let sited_device = create_device_ok(emulator, &global_network, json!({ "SiteId": site })).await;
    let siteless_device =
        create_device_ok(emulator, &global_network, json!({ "Description": name })).await;
    let link = create_link_ok(emulator, &global_network, &site, &plain_link()).await;
    let unassociated_link = create_link_ok(emulator, &global_network, &site, &plain_link()).await;

    // The link a row names has to be associated with the device it names, which is what
    // `AssociateLink` records.
    let (status, body) = emulator
        .post(
            &format!("/global-networks/{global_network}/link-associations"),
            json!({ "DeviceId": sited_device, "LinkId": link }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {name:?}: AssociateLink expected 200, got {status} with {body}"
    );

    // A Connect peer is created on a transit gateway that has to be registered with the
    // global network, and such a gateway lives in the peer's own Region and account, so one
    // gateway is registered per row.
    for (index, case) in cases().iter().enumerate() {
        register_gateway(
            emulator,
            &global_network,
            &transit_gateway_arn("aws", case.region, &case.account, base + index, 17),
        )
        .await;
    }

    Fixture {
        global_network,
        sited_device,
        siteless_device,
        link,
        unassociated_link,
    }
}

/// Assert one reported association against the row that made it.
fn assert_association(case: &Case, fixture: &Fixture, association: &Value, state: &str) {
    let name = case.name;
    assert_eq!(
        association["TransitGatewayConnectPeerArn"].as_str(),
        Some(case.arn.as_str()),
        "case {name:?}: expected the association to name peer {}, got {association}",
        case.arn
    );
    assert_eq!(
        association["GlobalNetworkId"].as_str(),
        Some(fixture.global_network.as_str()),
        "case {name:?}: expected global network {}, got {association}",
        fixture.global_network
    );
    assert_eq!(
        association["DeviceId"].as_str(),
        Some(case.device(fixture).as_str()),
        "case {name:?}: expected device {}, got {association}",
        case.device(fixture)
    );
    match case.expected_link(fixture) {
        Some(link) => assert_eq!(
            association["LinkId"].as_str(),
            Some(link.as_str()),
            "case {name:?}: expected link {link}, got {association}"
        ),
        None => assert!(
            association["LinkId"].is_null(),
            "case {name:?}: a row naming no link must report none, got {association}"
        ),
    }
    assert_eq!(
        association["State"].as_str(),
        Some(state),
        "case {name:?}: expected state {state}, got {association}"
    );
}

/// Associate every row, asserting each call succeeded, and answer the associations in row
/// order.
async fn associate_all(emulator: &Emulator, fixture: &Fixture, rows: &[Case]) -> Vec<Value> {
    let mut made: Vec<Value> = Vec::new();
    for case in rows {
        let (status, body) = associate(emulator, &fixture.global_network, case.request(fixture))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: AssociateTransitGatewayConnectPeer expected 200, got {status} with {body}",
            case.name
        );
        made.push(body["TransitGatewayConnectPeerAssociation"].clone());
    }
    made
}

#[tokio::test]
async fn every_association_round_trips_through_stored_state_and_is_removed_by_the_disassociate() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "associations", 0).await;
    let rows = cases();

    // The association is PENDING while the transition the clock owes is still due, which is
    // the state both the making call and a read straight after it report.
    let made = associate_all(&emulator, &fixture, &rows).await;
    for (index, case) in rows.iter().enumerate() {
        assert_association(case, &fixture, &made[index], "PENDING");
        let stored = read_one(&emulator, case.name, &fixture.global_network, &case.arn)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the reader must list the association", case.name));
        assert_association(case, &fixture, &stored, "PENDING");

        // Tying the peering to the global network registers its BGP session, which starts
        // DOWN: the telemetry entry is computed from the stored registration, so the Region
        // and the resource id are the peer ARN's own.
        let entry = telemetry_of(&emulator, case.name, &fixture.global_network, &case.arn)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the peering must be registered for telemetry",
                    case.name
                )
            });
        assert_eq!(
            entry["ResourceType"].as_str(),
            Some("transit-gateway-connect-peer"),
            "case {:?}: expected the peer's resource type, got {entry}",
            case.name
        );
        assert_eq!(
            entry["AwsRegion"].as_str(),
            Some(case.region),
            "case {:?}: expected Region {}, got {entry}",
            case.name,
            case.region
        );
        assert_eq!(
            entry["AccountId"].as_str(),
            Some(case.account.as_str()),
            "case {:?}: expected account {}, got {entry}",
            case.name,
            case.account
        );
        assert_eq!(
            entry["Health"]["Status"].as_str(),
            Some("DOWN"),
            "case {:?}: a fresh peering's session must still be DOWN, got {entry}",
            case.name
        );
    }

    // Every association of the global network is reported, in the stable key order of the
    // store, and nothing else is.
    let page = get_associations_ok(&emulator, "every association", &fixture.global_network, "")
        .await;
    let mut expected: Vec<String> = rows.iter().map(|case| case.arn.clone()).collect();
    expected.sort();
    assert_eq!(
        arns_of("every association", &page),
        expected,
        "case every association: expected exactly {expected:?}, got {page}"
    );
    assert!(
        page["NextToken"].is_null(),
        "case every association: an unpaged read must carry no NextToken, got {page}"
    );

    // Reading twice changes nothing a later caller sees.
    let again = get_associations_ok(&emulator, "repeated read", &fixture.global_network, "").await;
    assert_eq!(
        arns_of("repeated read", &again),
        expected,
        "case repeated read: a read must not change what a later caller sees, got {again}"
    );

    // Once the span the clock owes has elapsed every association is AVAILABLE and every
    // session is UP, both computed from the stored instants rather than written by anything.
    tokio::time::sleep(PAST_SETTLE).await;
    for case in &rows {
        let settled = read_one(&emulator, case.name, &fixture.global_network, &case.arn)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the reader must list the association", case.name));
        assert_association(case, &fixture, &settled, "AVAILABLE");
        let entry = telemetry_of(&emulator, case.name, &fixture.global_network, &case.arn)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the peering must still be registered", case.name));
        assert_eq!(
            entry["Health"]["Status"].as_str(),
            Some("UP"),
            "case {:?}: the transition the clock owed must have fallen due, got {entry}",
            case.name
        );
    }

    // DISASSOCIATE: the removed association is reported DELETING, a later read no longer
    // lists it, its peering is no longer registered, and a repeat of the same call is the
    // not-found this operation models.
    for case in &rows {
        let name = case.name;
        let (status, removed) = disassociate(&emulator, &fixture.global_network, &case.arn).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DisassociateTransitGatewayConnectPeer expected 200, got {status} with {removed}"
        );
        assert_association(
            case,
            &fixture,
            &removed["TransitGatewayConnectPeerAssociation"],
            "DELETING",
        );

        assert!(
            read_one(&emulator, name, &fixture.global_network, &case.arn)
                .await
                .is_none(),
            "case {name:?}: a removed association must not be listed any more"
        );
        assert!(
            telemetry_of(&emulator, name, &fixture.global_network, &case.arn)
                .await
                .is_none(),
            "case {name:?}: a removed association must deregister its peering"
        );

        let (status, body) = disassociate(&emulator, &fixture.global_network, &case.arn).await;
        assert_eq!(
            status, 404,
            "case {name:?} repeated: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name:?} repeated: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!(ASSOCIATION_KIND),
            "case {name:?} repeated: the refusal must name the association kind, got {body}"
        );
    }

    let empty = get_associations_ok(
        &emulator,
        "read after every disassociate",
        &fixture.global_network,
        "",
    )
    .await;
    assert_eq!(
        empty["TransitGatewayConnectPeerAssociations"],
        json!([]),
        "case read after every disassociate: expected an empty page, got {empty}"
    );
}

#[tokio::test]
async fn the_arn_filter_narrows_the_listing_and_the_page_tokens_reach_every_association_once() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "filters", 0).await;
    let rows = cases();
    associate_all(&emulator, &fixture, &rows).await;

    // An ARN of the same well-formed shape that names no stored association.
    let unknown = connect_peer_arn("aws", "us-west-2", &own_account(), 0xdead, 17);
    let all: Vec<String> = {
        let mut arns: Vec<String> = rows.iter().map(|case| case.arn.clone()).collect();
        arns.sort();
        arns
    };
    let sorted = |arns: &[&str]| {
        let mut selected: Vec<String> = arns.iter().map(|arn| (*arn).to_string()).collect();
        selected.sort();
        selected
    };

    let table: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("the filter absent", Vec::new(), all.clone()),
        (
            "one ARN",
            vec![rows[0].arn.clone()],
            vec![rows[0].arn.clone()],
        ),
        (
            "two ARNs",
            vec![rows[1].arn.clone(), rows[3].arn.clone()],
            sorted(&[&rows[1].arn, &rows[3].arn]),
        ),
        (
            "every ARN",
            rows.iter().map(|case| case.arn.clone()).collect(),
            all.clone(),
        ),
        ("one unknown ARN", vec![unknown.clone()], Vec::new()),
        (
            "a known and an unknown ARN",
            vec![rows[2].arn.clone(), unknown.clone()],
            vec![rows[2].arn.clone()],
        ),
        (
            "the same ARN twice",
            vec![rows[0].arn.clone(), rows[0].arn.clone()],
            vec![rows[0].arn.clone()],
        ),
        (
            // `TransitGatewayConnectPeerArns` is "One or more transit gateway Connect peer
            // Amazon Resource Names (ARNs)." (`smithy_query query="doc"
            // shape="GetTransitGatewayConnectPeerAssociationsRequest"
            // member="TransitGatewayConnectPeerArns"`), which documents none of the "The
            // maximum is 10." bound the id-list filters of the other listings carry, so a
            // selector well past ten ARNs is answered rather than refused.
            "far more ARNs than the bounded filters of this service allow",
            {
                let mut arns: Vec<String> = (0..26)
                    .map(|index| connect_peer_arn("aws", "us-west-2", &own_account(), 0xf00 + index, 17))
                    .collect();
                arns.extend(rows.iter().map(|case| case.arn.clone()));
                arns
            },
            all.clone(),
        ),
    ];

    for (case, arns, expected) in table {
        let query = arns
            .iter()
            .map(|arn| format!("transitGatewayConnectPeerArns={}", escape(arn)))
            .collect::<Vec<String>>()
            .join("&");
        let page = get_associations_ok(&emulator, case, &fixture.global_network, &query).await;
        assert_eq!(
            arns_of(case, &page),
            expected,
            "case {case:?}: expected exactly {expected:?}, got {page}"
        );
    }

    // A page size walks the same listing one association at a time, and the tokens reach
    // every one of them exactly once.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for step in 0..=rows.len() {
        let query = match &token {
            Some(next) => format!("maxResults=1&nextToken={}", escape(next)),
            None => "maxResults=1".to_string(),
        };
        let page = get_associations_ok(&emulator, "paged read", &fixture.global_network, &query)
            .await;
        let listed = arns_of("paged read", &page);
        assert!(
            listed.len() <= 1,
            "case paged read {step}: a page of one must hold at most one item, got {page}"
        );
        walked.extend(listed);
        token = page["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, all,
        "case paged read: following every token must reach each association once, got {walked:?}"
    );

    // A token this service did not mint is refused rather than read as an offset.
    let (status, refused) =
        get_associations(&emulator, &fixture.global_network, "nextToken=not-a-token").await;
    assert_eq!(
        status, 400,
        "case a foreign page token: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case a foreign page token: expected ValidationException, got {refused}"
    );
}

#[tokio::test]
async fn a_peer_is_tied_to_one_device_and_link_of_one_global_network() {
    let emulator = Emulator::new();
    let first = fixture(&emulator, "first", 0).await;
    let second = fixture(&emulator, "second", 100).await;
    let rows = cases();
    associate_all(&emulator, &first, &rows).await;

    for case in &rows {
        let name = case.name;

        // The identity of the association is the global network plus the peer, so the SAME
        // request again reports the stored association and restarts nothing: once the settle
        // span has passed, a repeat still answers AVAILABLE.
        tokio::time::sleep(PAST_SETTLE).await;
        let (status, repeated) =
            associate(&emulator, &first.global_network, case.request(&first)).await;
        assert_eq!(
            status, 200,
            "case {name:?} repeated: expected 200, got {status} with {repeated}"
        );
        assert_association(
            case,
            &first,
            &repeated["TransitGatewayConnectPeerAssociation"],
            "AVAILABLE",
        );

        // "You cannot associate a transit gateway Connect peer with more than one device and
        // link.", so the same peer offered under ANOTHER device is the modelled conflict and
        // leaves the stored association alone.
        let other_device = if case.sited_device {
            first.siteless_device.clone()
        } else {
            first.sited_device.clone()
        };
        let (status, conflict) = associate(
            &emulator,
            &first.global_network,
            json!({
                "TransitGatewayConnectPeerArn": case.arn,
                "DeviceId": other_device,
            }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {name:?} another device: expected 409, got {status} with {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {name:?} another device: expected ConflictException, got {conflict}"
        );
        assert_eq!(
            conflict["ResourceType"],
            json!(ASSOCIATION_KIND),
            "case {name:?} another device: the refusal must name the association kind, got {conflict}"
        );
        let stored = read_one(&emulator, name, &first.global_network, &case.arn)
            .await
            .unwrap_or_else(|| panic!("case {name:?}: the stored association must survive"));
        assert_association(case, &first, &stored, "AVAILABLE");

        // An association made in one global network is not addressable through another: the
        // other one lists none of them and its disassociate is a not-found that leaves the
        // stored tie alone.
        assert!(
            read_one(&emulator, name, &second.global_network, &case.arn)
                .await
                .is_none(),
            "case {name:?} elsewhere: another global network must list nothing"
        );
        let (status, body) = disassociate(&emulator, &second.global_network, &case.arn).await;
        assert_eq!(
            status, 404,
            "case {name:?} disassociate elsewhere: expected 404, got {status} with {body}"
        );
        assert!(
            read_one(&emulator, name, &first.global_network, &case.arn)
                .await
                .is_some(),
            "case {name:?} disassociate elsewhere: the stored association must survive"
        );

        // Disassociating frees the peer, so it can then be tied to the other device.
        let (status, removed) = disassociate(&emulator, &first.global_network, &case.arn).await;
        assert_eq!(
            status, 200,
            "case {name:?} disassociate: expected 200, got {status} with {removed}"
        );
        let (status, moved) = associate(
            &emulator,
            &first.global_network,
            json!({
                "TransitGatewayConnectPeerArn": case.arn,
                "DeviceId": other_device,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?} associate after disassociate: expected 200, got {status} with {moved}"
        );
        assert_eq!(
            moved["TransitGatewayConnectPeerAssociation"]["DeviceId"],
            json!(other_device),
            "case {name:?} associate after disassociate: the tie must name the new device, got {moved}"
        );
    }
}

#[tokio::test]
async fn an_associate_that_cannot_hold_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "refusals", 0).await;
    let rows = cases();
    let case = &rows[0];
    let absent_id = "device-0000000000000dead";

    // A reference that names nothing, and one stored under another global network, are both
    // the missing-resource answer; a link not associated with the named device, and a peer
    // no registered gateway of this global network can carry, are both refused as 400.
    let refusals: Vec<(&str, u16, &str, Value)> = vec![
        (
            "an unknown device",
            404,
            "ResourceNotFoundException",
            json!({ "TransitGatewayConnectPeerArn": case.arn, "DeviceId": absent_id }),
        ),
        (
            "an unknown link",
            404,
            "ResourceNotFoundException",
            json!({
                "TransitGatewayConnectPeerArn": case.arn,
                "DeviceId": fixture.sited_device,
                "LinkId": "link-0000000000000dead",
            }),
        ),
        (
            "a link the device is not associated with",
            400,
            "ValidationException",
            json!({
                "TransitGatewayConnectPeerArn": case.arn,
                "DeviceId": fixture.sited_device,
                "LinkId": fixture.unassociated_link,
            }),
        ),
        (
            "a link of another device",
            400,
            "ValidationException",
            json!({
                "TransitGatewayConnectPeerArn": case.arn,
                "DeviceId": fixture.siteless_device,
                "LinkId": fixture.link,
            }),
        ),
        (
            // No gateway of this Region and account is registered with the global network,
            // so no transit gateway it holds can have been the one the peer was created on.
            "a peer of a Region no registered gateway is in",
            400,
            "ValidationException",
            json!({
                "TransitGatewayConnectPeerArn":
                    connect_peer_arn("aws", "sa-east-1", &own_account(), 7, 17),
                "DeviceId": fixture.sited_device,
            }),
        ),
        (
            // Same Region as a registered gateway, but another account owns the peer.
            "a peer of an account no registered gateway belongs to",
            400,
            "ValidationException",
            json!({
                "TransitGatewayConnectPeerArn":
                    connect_peer_arn("aws", "us-west-2", "999988887777", 8, 17),
                "DeviceId": fixture.sited_device,
            }),
        ),
    ];

    for (name, expected_status, expected_code, body) in &refusals {
        let (status, refused) = associate(&emulator, &fixture.global_network, body.clone()).await;
        assert_eq!(
            status, *expected_status,
            "case {name:?}: expected {expected_status}, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            *expected_code,
            "case {name:?}: expected {expected_code}, got {refused}"
        );
    }

    let empty = get_associations_ok(
        &emulator,
        "read after the refusals",
        &fixture.global_network,
        "",
    )
    .await;
    assert_eq!(
        empty["TransitGatewayConnectPeerAssociations"],
        json!([]),
        "case read after the refusals: a refused associate must store nothing, got {empty}"
    );
}

#[tokio::test]
async fn a_string_that_names_no_connect_peer_is_refused_on_the_member_that_carried_it() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "malformed peers", 0).await;

    // The rows span the ways the EC2 ARN skeleton can fail: not an ARN at all, another
    // service's namespace, another resource type, and each segment the skeleton requires
    // left empty.
    let malformed = [
        ("a bare peer id", "tgw-connect-peer-0123456789abcdef1".to_string()),
        (
            "another service's namespace",
            "arn:aws:networkmanager:us-west-2:123456789012:transit-gateway-connect-peer/tgw-connect-peer-0123abcd"
                .to_string(),
        ),
        (
            "another resource type",
            "arn:aws:ec2:us-west-2:123456789012:transit-gateway/tgw-0123abcd".to_string(),
        ),
        (
            "no partition",
            "arn::ec2:us-west-2:123456789012:transit-gateway-connect-peer/tgw-connect-peer-0123abcd"
                .to_string(),
        ),
        (
            "no Region",
            "arn:aws:ec2::123456789012:transit-gateway-connect-peer/tgw-connect-peer-0123abcd"
                .to_string(),
        ),
        (
            "no resource id",
            "arn:aws:ec2:us-west-2:123456789012:transit-gateway-connect-peer/".to_string(),
        ),
    ];

    for (name, value) in &malformed {
        let (status, refused) = associate(
            &emulator,
            &fixture.global_network,
            json!({
                "TransitGatewayConnectPeerArn": value,
                "DeviceId": fixture.sited_device,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?} associate: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {name:?} associate: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused["Fields"][0]["Name"],
            json!(ARN_MEMBER),
            "case {name:?} associate: the refusal must name the member at fault, got {refused}"
        );

        let (status, refused) = disassociate(&emulator, &fixture.global_network, value).await;
        assert_eq!(
            status, 400,
            "case {name:?} disassociate: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {name:?} disassociate: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused["Fields"][0]["Name"],
            json!(ARN_MEMBER),
            "case {name:?} disassociate: the refusal must name the member at fault, got {refused}"
        );
    }
}

#[tokio::test]
async fn a_global_network_that_is_not_stored_is_refused_on_every_operation() {
    let emulator = Emulator::new();
    let absent = "global-network-0000000000000dead";
    let arn = connect_peer_arn("aws", "us-west-2", "123456789012", 9, 17);

    // The REPORTING and the DISASSOCIATING calls are addressed at state the store already
    // holds, so a global network it does not hold is the missing-resource answer.
    let calls: [(&str, (u16, Value)); 2] = [
        (
            "GetTransitGatewayConnectPeerAssociations",
            get_associations(&emulator, absent, "").await,
        ),
        (
            "DisassociateTransitGatewayConnectPeer",
            disassociate(&emulator, absent, &arn).await,
        ),
    ];

    for (case, (status, body)) in &calls {
        assert_eq!(
            *status, 404,
            "case {case}: expected 404 for an unknown global network, got {status} with {body}"
        );
        assert_eq!(
            error_code(body),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("global-network"),
            "case {case}: the refusal must name the global network, got {body}"
        );
    }

    // ASSOCIATING stores a new record under the global network the path addresses, so that
    // parent reference is validated before anything is stored: the refusal is the field
    // failure the creates under a global network answer, naming the member at fault.
    let (status, body) = associate(
        &emulator,
        absent,
        json!({
            "TransitGatewayConnectPeerArn": arn,
            "DeviceId": "device-0000000000000beef",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case AssociateTransitGatewayConnectPeer: expected 400 for an unknown global network, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case AssociateTransitGatewayConnectPeer: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some("FieldValidationFailed"),
        "case AssociateTransitGatewayConnectPeer: expected Reason FieldValidationFailed, got {body}"
    );
    assert_eq!(
        body["Fields"],
        json!([{ "Name": "globalNetworkId", "Message": "Global network not found." }]),
        "case AssociateTransitGatewayConnectPeer: expected one Fields entry naming globalNetworkId, got {body}"
    );

    // The combined case: a request whose peer ARN names no Connect peer at all is a FIELD
    // failure, decided before any stored state is consulted — the same order the
    // transit-gateway-registration slice of this service resolves its own free-form ARN in —
    // so it answers 400 even under a global network the store does not hold.
    let combined: [(&str, (u16, Value)); 2] = [
        (
            "AssociateTransitGatewayConnectPeer",
            associate(
                &emulator,
                absent,
                json!({
                    "TransitGatewayConnectPeerArn": "tgw-connect-peer-0123456789abcdef1",
                    "DeviceId": "device-0000000000000beef",
                }),
            )
            .await,
        ),
        (
            "DisassociateTransitGatewayConnectPeer",
            disassociate(&emulator, absent, "tgw-connect-peer-0123456789abcdef1").await,
        ),
    ];
    for (case, (status, body)) in &combined {
        assert_eq!(
            *status, 400,
            "case {case} with a malformed ARN: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(body),
            "ValidationException",
            "case {case} with a malformed ARN: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["Fields"][0]["Name"],
            json!(ARN_MEMBER),
            "case {case} with a malformed ARN: the refusal must name the member at fault, got {body}"
        );
    }
}
