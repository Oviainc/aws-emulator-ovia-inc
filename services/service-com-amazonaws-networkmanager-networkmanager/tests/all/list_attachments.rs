// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListAttachments`, driven over HTTP through the real router (the
//! harness lives in `crate::global_network_lifecycle`, the core-network-with-edges helper in
//! `crate::attachment_lifecycle` and the acceptance-policy helper in
//! `crate::attachment_acceptance`).
//!
//! The rule under test: `ListAttachments` reports the stored `Attachment` envelope of every
//! attachment the emulator holds, of whatever kind, in attachment-id order; each of
//! `CoreNetworkId`, `AttachmentType`, `EdgeLocation` and `State` NARROWS that listing, so a
//! value naming nothing stored selects no attachment rather than failing the read; the
//! envelope reported is the one the reader of the attachment's own kind answers, the state
//! being the one the attachment is in as of the call; the listing is paged by `MaxResults`
//! and resumed by a `NextToken` this service minted, and a token it did not mint is refused
//! as a 400 `ValidationException`; and the read leaves the store exactly as the next caller
//! would have found it.
//!
//! The tables below span that class: attachments differing in KIND (all five
//! `AttachmentType` values), in core network (three of them), in edge location (five, one of
//! them an attachment spanning TWO), and in state (the two the clock decides plus the one it
//! never moves); filters differing in which member is named, in how many are combined, and
//! in whether the value names anything stored; page sizes at the modelled bounds and either
//! side of the number of attachments there are; and tokens differing in how they fail to be
//! one this service issued. Nothing reaches into `crate::state`: every attachment is created
//! and read back through an operation.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::attachment_acceptance::core_network_with;
use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, create_vpc_attachment, subnet_arn,
    vpc_arn,
};
use crate::direct_connect_gateway_attachment_lifecycle::gateway_arn;
use crate::global_network_lifecycle::{error_code, ids_of, Emulator, PAST_SETTLE};
use crate::resource_tagging::escape;

/// The five `AttachmentType` values the model enumerates (`smithy_query
/// shape=AttachmentType`), which is where the fixture's kinds come from.
const VPC: &str = "VPC";
/// See [`VPC`].
const CONNECT: &str = "CONNECT";
/// See [`VPC`].
const SITE_TO_SITE_VPN: &str = "SITE_TO_SITE_VPN";
/// See [`VPC`].
const DIRECT_CONNECT_GATEWAY: &str = "DIRECT_CONNECT_GATEWAY";
/// See [`VPC`].
const TRANSIT_GATEWAY_ROUTE_TABLE: &str = "TRANSIT_GATEWAY_ROUTE_TABLE";

/// The `AttachmentState` values this suite selects on (`smithy_query
/// shape=AttachmentState`): the two the clock decides, the one only an acceptance moves, and
/// two the fixture puts nothing in.
const CREATING: &str = "CREATING";
/// See [`CREATING`].
const AVAILABLE: &str = "AVAILABLE";
/// See [`CREATING`].
const PENDING_ATTACHMENT_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";
/// See [`CREATING`].
const REJECTED: &str = "REJECTED";
/// See [`CREATING`].
const DELETING: &str = "DELETING";

/// The widest page size the model admits: `MaxResults` carries `range 1..500`
/// (`smithy_query query="operation" operation="ListAttachments"`).
const MAX_PAGE: usize = 500;

/// The `ListAttachments` route (`GET /attachments`).
const LIST_URI: &str = "/attachments";

/// The edge locations the fixture's core networks configure.
const WEST: &str = "us-west-2";
/// See [`WEST`].
const CENTRAL: &str = "eu-central-1";
/// See [`WEST`].
const SOUTHEAST: &str = "ap-southeast-4";
/// See [`WEST`].
const EAST: &str = "sa-east-1";
/// See [`WEST`]: the edge of the core network whose policy asks for attachment acceptance.
const NORTHEAST: &str = "ap-northeast-1";

/// The `ListAttachments` URI carrying `query` (already percent-encoded).
fn list_uri(query: &str) -> String {
    if query.is_empty() {
        LIST_URI.to_string()
    } else {
        format!("{LIST_URI}?{query}")
    }
}

/// `ListAttachments` with `query`, asserting it is served, and the body it answered.
async fn list(emulator: &Emulator, case: &str, query: &str) -> Value {
    let (status, body) = emulator.get(&list_uri(query)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAttachments expected 200, got {status} with {body}"
    );
    body
}

/// The attachment ids one page reports, in the order reported.
fn reported(body: &Value) -> Vec<String> {
    ids_of(body, "Attachments", "AttachmentId")
}

/// The fixture: the attachments created, each under the label every assertion names it by,
/// and the core networks they sit on.
struct Fixture {
    /// Label to minted attachment id.
    attachments: BTreeMap<&'static str, String>,
    /// The three core networks, in the order [`seed`] creates them.
    core_networks: [String; 3],
}

impl Fixture {
    /// The ids of `labels`, in the order a listing reports them (ascending `AttachmentId`,
    /// the order the store keys attachments in).
    fn ids(&self, labels: &[&str]) -> Vec<String> {
        let mut ids: Vec<String> = labels
            .iter()
            .map(|label| {
                self.attachments
                    .get(label)
                    .unwrap_or_else(|| panic!("the fixture must hold the attachment {label:?}"))
                    .clone()
            })
            .collect();
        ids.sort();
        ids
    }

    /// Every label the fixture holds.
    fn all_labels(&self) -> Vec<&'static str> {
        self.attachments.keys().copied().collect()
    }
}

/// Create the fixture: three core networks and one attachment of each modelled kind.
///
/// The first core network configures edges in [`WEST`] and [`CENTRAL`] and carries a VPC
/// attachment, a Connect attachment over it, a site-to-site VPN attachment and a Direct
/// Connect gateway attachment SPANNING both of its edges; the second configures
/// [`SOUTHEAST`] and [`EAST`] and carries a VPC attachment and a transit gateway route table
/// attachment over a peering; the third's policy asks for attachment acceptance, so the one
/// attachment on it waits in [`PENDING_ATTACHMENT_ACCEPTANCE`] and no span of the clock
/// moves it.
async fn seed(emulator: &Emulator) -> Fixture {
    let first = core_network_at(emulator, "first listing network", &[WEST, CENTRAL])
        .await
        .1;
    let second = core_network_at(emulator, "second listing network", &[SOUTHEAST, EAST])
        .await
        .1;
    // The published Cloud WAN policy key `require-attachment-acceptance`, the same source
    // `crate::attachment_acceptance` builds its rules from.
    let acceptance_rules = json!([{
        "rule-number": 100,
        "conditions": [{ "type": "any" }],
        "action": {
            "association-method": "constant",
            "segment": "shared",
            "require-attachment-acceptance": true,
        },
    }]);
    let third = core_network_with(
        emulator,
        "acceptance listing network",
        NORTHEAST,
        &acceptance_rules,
    )
    .await;

    let mut attachments: BTreeMap<&'static str, String> = BTreeMap::new();

    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": first,
            "VpcArn": vpc_arn(WEST, 1),
            "SubnetArns": [subnet_arn(WEST, 1)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the west VPC attachment must be created, got {status} with {created}"
    );
    let west_vpc = attachment_id("the west VPC attachment", &created["VpcAttachment"]);
    attachments.insert("west vpc", west_vpc.clone());

    let (status, created) = create_connect_attachment(
        emulator,
        json!({
            "CoreNetworkId": first,
            "EdgeLocation": WEST,
            "TransportAttachmentId": west_vpc,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the west Connect attachment must be created, got {status} with {created}"
    );
    attachments.insert(
        "west connect",
        attachment_id("the west Connect attachment", &created["ConnectAttachment"]),
    );

    let (status, created) = emulator
        .post(
            "/site-to-site-vpn-attachments",
            json!({
                "CoreNetworkId": first,
                "VpnConnectionArn": format!(
                    "arn:aws:ec2:{CENTRAL}:{}:vpn-connection/vpn-0123abcd",
                    engine::emulator_account_id()
                ),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the central VPN attachment must be created, got {status} with {created}"
    );
    attachments.insert(
        "central vpn",
        attachment_id(
            "the central VPN attachment",
            &created["SiteToSiteVpnAttachment"],
        ),
    );

    let (status, created) = emulator
        .post(
            "/direct-connect-gateway-attachments",
            json!({
                "CoreNetworkId": first,
                "DirectConnectGatewayArn": gateway_arn(1),
                // The one kind that spans SEVERAL edges, so a listing selected on either of
                // them has to report it.
                "EdgeLocations": [CENTRAL, WEST],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the spanning Direct Connect gateway attachment must be created, got {status} with {created}"
    );
    attachments.insert(
        "spanning direct connect gateway",
        attachment_id(
            "the spanning Direct Connect gateway attachment",
            &created["DirectConnectGatewayAttachment"],
        ),
    );

    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": second,
            "VpcArn": vpc_arn(SOUTHEAST, 2),
            "SubnetArns": [subnet_arn(SOUTHEAST, 2)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the southeast VPC attachment must be created, got {status} with {created}"
    );
    attachments.insert(
        "southeast vpc",
        attachment_id("the southeast VPC attachment", &created["VpcAttachment"]),
    );

    let (status, peering) = emulator
        .post(
            "/transit-gateway-peerings",
            json!({
                "CoreNetworkId": second,
                "TransitGatewayArn":
                    format!("arn:aws:ec2:{EAST}:111122223333:transit-gateway/tgw-0123abcd"),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the east peering must be created, got {status} with {peering}"
    );
    let peering_id = peering["TransitGatewayPeering"]["Peering"]["PeeringId"]
        .as_str()
        .unwrap_or_else(|| panic!("no PeeringId in {peering}"))
        .to_string();
    let (status, created) = emulator
        .post(
            "/transit-gateway-route-table-attachments",
            json!({
                "PeeringId": peering_id,
                "TransitGatewayRouteTableArn": format!(
                    "arn:aws:ec2:{EAST}:111122223333:transit-gateway-route-table/tgw-rtb-0123abcd"
                ),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the east route table attachment must be created, got {status} with {created}"
    );
    attachments.insert(
        "east route table",
        attachment_id(
            "the east route table attachment",
            &created["TransitGatewayRouteTableAttachment"],
        ),
    );

    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": third,
            "VpcArn": vpc_arn(NORTHEAST, 3),
            "SubnetArns": [subnet_arn(NORTHEAST, 3)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the attachment awaiting acceptance must be created, got {status} with {created}"
    );
    attachments.insert(
        "northeast vpc awaiting acceptance",
        attachment_id(
            "the attachment awaiting acceptance",
            &created["VpcAttachment"],
        ),
    );

    Fixture {
        attachments,
        core_networks: [first, second, third],
    }
}

/// One row of the filter table: the query a caller sends and the labels it must select.
struct FilterCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The query string, built from the fixture's minted ids.
    query: String,
    /// The labels the listing must report, in any order (the assertion sorts both sides by
    /// id, which is the order the listing uses).
    expected: Vec<&'static str>,
}

/// The table: one row per member a filter names, per combination of members, and per value
/// that names nothing stored.
fn filter_cases(fixture: &Fixture) -> Vec<FilterCase> {
    let [first, second, third] = &fixture.core_networks;
    vec![
        FilterCase {
            name: "no filter at all reports every stored attachment",
            query: String::new(),
            expected: fixture.all_labels(),
        },
        FilterCase {
            name: "the first core network",
            query: format!("coreNetworkId={first}"),
            expected: vec![
                "central vpn",
                "spanning direct connect gateway",
                "west connect",
                "west vpc",
            ],
        },
        FilterCase {
            name: "the second core network",
            query: format!("coreNetworkId={second}"),
            expected: vec!["east route table", "southeast vpc"],
        },
        FilterCase {
            name: "the core network whose policy asks for acceptance",
            query: format!("coreNetworkId={third}"),
            expected: vec!["northeast vpc awaiting acceptance"],
        },
        FilterCase {
            name: "every VPC attachment, across core networks",
            query: format!("attachmentType={VPC}"),
            expected: vec![
                "northeast vpc awaiting acceptance",
                "southeast vpc",
                "west vpc",
            ],
        },
        FilterCase {
            name: "every Connect attachment",
            query: format!("attachmentType={CONNECT}"),
            expected: vec!["west connect"],
        },
        FilterCase {
            name: "every site-to-site VPN attachment",
            query: format!("attachmentType={SITE_TO_SITE_VPN}"),
            expected: vec!["central vpn"],
        },
        FilterCase {
            name: "every Direct Connect gateway attachment",
            query: format!("attachmentType={DIRECT_CONNECT_GATEWAY}"),
            expected: vec!["spanning direct connect gateway"],
        },
        FilterCase {
            name: "every transit gateway route table attachment",
            query: format!("attachmentType={TRANSIT_GATEWAY_ROUTE_TABLE}"),
            expected: vec!["east route table"],
        },
        FilterCase {
            name: "the west edge, which the spanning attachment also sits at",
            query: format!("edgeLocation={WEST}"),
            expected: vec![
                "spanning direct connect gateway",
                "west connect",
                "west vpc",
            ],
        },
        FilterCase {
            name: "the central edge, the spanning attachment's other one",
            query: format!("edgeLocation={CENTRAL}"),
            expected: vec!["central vpn", "spanning direct connect gateway"],
        },
        FilterCase {
            name: "the southeast edge",
            query: format!("edgeLocation={SOUTHEAST}"),
            expected: vec!["southeast vpc"],
        },
        FilterCase {
            name: "the east edge, where the route table attachment sits",
            query: format!("edgeLocation={EAST}"),
            expected: vec!["east route table"],
        },
        FilterCase {
            name: "the northeast edge, where the attachment awaiting acceptance sits",
            query: format!("edgeLocation={NORTHEAST}"),
            expected: vec!["northeast vpc awaiting acceptance"],
        },
        FilterCase {
            name: "an edge location no core network of this emulator configures",
            query: "edgeLocation=me-south-1".to_string(),
            expected: Vec::new(),
        },
        FilterCase {
            name: "an edge location of 63 characters, the width the member's bound allows",
            query: format!("edgeLocation={}", "r".repeat(63)),
            expected: Vec::new(),
        },
        FilterCase {
            name: "the first core network's VPC attachments",
            query: format!("coreNetworkId={first}&attachmentType={VPC}"),
            expected: vec!["west vpc"],
        },
        FilterCase {
            name: "the first core network's attachments at the central edge",
            query: format!("coreNetworkId={first}&edgeLocation={CENTRAL}"),
            expected: vec!["central vpn", "spanning direct connect gateway"],
        },
        FilterCase {
            name: "all three selectors at once",
            query: format!(
                "coreNetworkId={second}&attachmentType={VPC}&edgeLocation={SOUTHEAST}"
            ),
            expected: vec!["southeast vpc"],
        },
        FilterCase {
            name: "a core network holding an attachment of another kind than the one asked for",
            query: format!("coreNetworkId={second}&attachmentType={CONNECT}"),
            expected: Vec::new(),
        },
        FilterCase {
            name: "an attachment type at an edge location another core network holds",
            query: format!("attachmentType={CONNECT}&edgeLocation={SOUTHEAST}"),
            expected: Vec::new(),
        },
        FilterCase {
            name: "a core network id nothing is stored for",
            query: "coreNetworkId=core-network-0000000000000ffff".to_string(),
            expected: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn an_emulator_holding_no_attachment_reports_the_empty_listing() {
    let emulator = Emulator::new();

    // Nothing has been created, so the listing is the empty one the store really holds —
    // not a refusal and not a placeholder entry.
    for (case, query) in [
        ("no filter at all", String::new()),
        ("a core network id nothing is stored for", "coreNetworkId=core-network-0000000000000ffff".to_string()),
        ("an attachment type nothing was created of", format!("attachmentType={VPC}")),
        ("a state nothing is in", format!("state={AVAILABLE}")),
        ("an edge location nothing sits at", format!("edgeLocation={WEST}")),
    ] {
        let body = list(&emulator, case, &query).await;
        assert_eq!(
            reported(&body),
            Vec::<String>::new(),
            "case {case:?}: expected no entry, got {body}"
        );
        assert!(
            body["NextToken"].is_null(),
            "case {case:?}: an empty listing owes no token, got {body}"
        );
    }
}

#[tokio::test]
async fn list_attachments_reports_the_stored_attachments_each_filter_selects() {
    let emulator = Emulator::new();
    let fixture = seed(&emulator).await;
    // The attachments the clock owes a transition are past it, so what every row below
    // reports is stable while the table is walked.
    tokio::time::sleep(PAST_SETTLE).await;

    for case in filter_cases(&fixture) {
        let name = case.name;
        let body = list(&emulator, name, &case.query).await;
        let wanted = fixture.ids(&case.expected);
        let got = reported(&body);
        assert_eq!(
            got, wanted,
            "case {name:?}: expected the attachments {:?} in id order, got {got:?} from {body}",
            case.expected
        );
        assert!(
            body["NextToken"].is_null(),
            "case {name:?}: a page naming no size reports everything, so no token is owed, got {body}"
        );
    }
}

#[tokio::test]
async fn the_envelope_a_listing_reports_is_the_one_each_kind_s_reader_answers() {
    let emulator = Emulator::new();
    let fixture = seed(&emulator).await;
    tokio::time::sleep(PAST_SETTLE).await;

    // Each row: an attachment of the fixture, the reader of its own kind, and the wrapper
    // member that reader answers the envelope inside.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("west vpc", "/vpc-attachments", "VpcAttachment"),
        ("west connect", "/connect-attachments", "ConnectAttachment"),
        (
            "central vpn",
            "/site-to-site-vpn-attachments",
            "SiteToSiteVpnAttachment",
        ),
        (
            "spanning direct connect gateway",
            "/direct-connect-gateway-attachments",
            "DirectConnectGatewayAttachment",
        ),
        (
            "east route table",
            "/transit-gateway-route-table-attachments",
            "TransitGatewayRouteTableAttachment",
        ),
        (
            "northeast vpc awaiting acceptance",
            "/vpc-attachments",
            "VpcAttachment",
        ),
    ];

    for (label, route, wrapper) in rows {
        let id = fixture.ids(&[label]).remove(0);
        // The whole listing, so the entry compared against the reader of this kind is the
        // one an unfiltered caller sees.
        let body = list(&emulator, label, "").await;
        let listed = body["Attachments"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: expected an Attachments array in {body}"))
            .iter()
            .find(|entry| entry["AttachmentId"] == json!(id))
            .unwrap_or_else(|| panic!("case {label:?}: {id} must be listed, got {body}"))
            .clone();

        let (status, read) = emulator.get(&format!("{route}/{id}")).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the reader of this kind expected 200, got {status} with {read}"
        );
        assert_eq!(
            listed, read[wrapper]["Attachment"],
            "case {label:?}: the listed envelope must be the one {route} answers, got {listed}"
        );
    }

    // A read changes nothing a later caller sees: the same listing twice is the same answer.
    let first = list(&emulator, "the first read", "").await;
    let second = list(&emulator, "the second read", "").await;
    assert_eq!(
        first, second,
        "listing twice must answer the same body, got {first} then {second}"
    );
}

#[tokio::test]
async fn list_attachments_selects_on_the_state_each_attachment_is_in() {
    let emulator = Emulator::new();
    // A small fixture, so the states the clock decides are asserted well inside the span it
    // owes the transition over: one ordinary attachment, which is CREATING, and one on a core
    // network whose policy asks for acceptance, which is not.
    let ordinary = core_network_at(&emulator, "state listing network", &[WEST])
        .await
        .1;
    let (status, created) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": ordinary,
            "VpcArn": vpc_arn(WEST, 1),
            "SubnetArns": [subnet_arn(WEST, 1)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the ordinary attachment must be created, got {status} with {created}"
    );
    let creating = attachment_id("the ordinary attachment", &created["VpcAttachment"]);

    let awaiting_network = core_network_with(
        &emulator,
        "state acceptance network",
        NORTHEAST,
        &json!([{
            "rule-number": 100,
            "conditions": [{ "type": "any" }],
            "action": { "require-attachment-acceptance": true },
        }]),
    )
    .await;
    let (status, created) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": awaiting_network,
            "VpcArn": vpc_arn(NORTHEAST, 2),
            "SubnetArns": [subnet_arn(NORTHEAST, 2)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the attachment awaiting acceptance must be created, got {status} with {created}"
    );
    let awaiting = attachment_id("the attachment awaiting acceptance", &created["VpcAttachment"]);

    // Before the transition the clock owes falls due: each attachment is selected by the
    // state it is in and by no other, and a state nothing is in selects nothing.
    for (case, state, wanted) in [
        ("the state the clock owes a transition from", CREATING, vec![creating.clone()]),
        (
            "the state only an acceptance moves",
            PENDING_ATTACHMENT_ACCEPTANCE,
            vec![awaiting.clone()],
        ),
        ("the state nothing has reached yet", AVAILABLE, Vec::new()),
        ("a state nothing was put in", REJECTED, Vec::new()),
        ("the state of nothing being deleted", DELETING, Vec::new()),
    ] {
        let body = list(&emulator, case, &format!("state={state}")).await;
        assert_eq!(
            reported(&body),
            wanted,
            "case {case:?}: expected {wanted:?} in state {state}, got {body}"
        );
    }

    tokio::time::sleep(PAST_SETTLE).await;

    // After it: the ordinary attachment has moved, the one waiting for its acceptance has
    // not — no span of the clock moves that one.
    for (case, state, wanted) in [
        ("the state the clock moved it to", AVAILABLE, vec![creating.clone()]),
        ("the state it left", CREATING, Vec::new()),
        (
            "the state the clock never moves",
            PENDING_ATTACHMENT_ACCEPTANCE,
            vec![awaiting.clone()],
        ),
    ] {
        let body = list(&emulator, case, &format!("state={state}")).await;
        assert_eq!(
            reported(&body),
            wanted,
            "case {case:?}: expected {wanted:?} in state {state}, got {body}"
        );
    }

    // A state filter combines with the others the same way: the attachment is reported only
    // where every selector holds.
    let body = list(
        &emulator,
        "state and core network together",
        &format!("state={AVAILABLE}&coreNetworkId={awaiting_network}"),
    )
    .await;
    assert_eq!(
        reported(&body),
        Vec::<String>::new(),
        "an available attachment of another core network is not reported, got {body}"
    );
}

#[tokio::test]
async fn list_attachments_pages_the_listing_with_the_tokens_it_mints() {
    let emulator = Emulator::new();
    let fixture = seed(&emulator).await;
    tokio::time::sleep(PAST_SETTLE).await;

    let wanted = fixture.ids(&fixture.all_labels());
    assert!(
        wanted.len() > 2,
        "the fixture must hold enough attachments to page over, got {wanted:?}"
    );

    // Each row: a page size. 1 is the modelled lower bound, 500 the upper one, and the sizes
    // between them straddle the number of attachments there are to report.
    for size in [1, 2, 3, wanted.len(), wanted.len() + 1, MAX_PAGE] {
        let case = format!("maxResults={size}");
        let mut walked: Vec<String> = Vec::new();
        let mut query = format!("maxResults={size}");
        let mut pages = 0usize;
        loop {
            let body = list(&emulator, &case, &query).await;
            let page = reported(&body);
            assert!(
                !page.is_empty() && page.len() <= size,
                "case {case:?}: a page holds at least one and at most {size} entries, got {body}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= wanted.len(),
                "case {case:?}: paging must terminate within {} pages, got {body}",
                wanted.len()
            );
            match body["NextToken"].as_str() {
                Some(token) => query = format!("maxResults={size}&nextToken={}", escape(token)),
                None => break,
            }
        }
        assert_eq!(
            walked, wanted,
            "case {case:?}: following the tokens must report every attachment exactly once"
        );
        let expected_pages = wanted.len().div_ceil(size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {} attachments at {size} per page is {expected_pages} pages, walked {pages}",
            wanted.len()
        );
    }

    // A page size narrowed by a filter pages only what the filter selects.
    let vpcs = fixture.ids(&[
        "northeast vpc awaiting acceptance",
        "southeast vpc",
        "west vpc",
    ]);
    let mut walked: Vec<String> = Vec::new();
    let mut query = format!("attachmentType={VPC}&maxResults=1");
    loop {
        let body = list(&emulator, "one VPC attachment per page", &query).await;
        walked.extend(reported(&body));
        match body["NextToken"].as_str() {
            Some(token) => {
                query = format!("attachmentType={VPC}&maxResults=1&nextToken={}", escape(token));
            }
            None => break,
        }
    }
    assert_eq!(
        walked, vpcs,
        "paging a filtered listing reports exactly what the filter selects, got {walked:?}"
    );
}

#[tokio::test]
async fn the_listing_refuses_a_token_it_never_issued_and_a_page_size_outside_the_range() {
    let emulator = Emulator::new();
    let fixture = seed(&emulator).await;

    // A token minted for this listing, so the rows below differ from it only in the way they
    // fail to be one this service issued.
    let body = list(&emulator, "the first page", "maxResults=1").await;
    let issued = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a first page of one must owe a token, got {body}"))
        .to_string();
    assert!(
        issued.len() > 4,
        "the issued token must be long enough to corrupt, got {issued:?}"
    );

    // Each row: a `NextToken` this service did not mint. All three are refused on the member
    // rather than answered with a page: HTTP 400 `ValidationException`, which this operation
    // models (`smithy_query query="operation" operation="ListAttachments"`).
    for (case, token) in [
        ("a token that is not hex at all", "not-a-token".to_string()),
        // `6162636465` is the hex of "abcde", which carries none of the prefix this service's
        // own tokens do.
        ("a token that is hex but names no listing of this service", "6162636465".to_string()),
        ("a token of this service with its head cut off", issued[2..].to_string()),
    ] {
        let (status, refused) = emulator
            .get(&list_uri(&format!("nextToken={}", escape(&token))))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
    }

    // The token this service DID issue is served, which is what makes the rows above a
    // refusal of the token and not of the request.
    let resumed = list(
        &emulator,
        "the issued token",
        &format!("maxResults=1&nextToken={}", escape(&issued)),
    )
    .await;
    assert_eq!(
        reported(&resumed).len(),
        1,
        "the issued token resumes the listing, got {resumed}"
    );

    // `MaxResults` is modelled `range 1..500`, so a size past either published bound is
    // refused by the constraint walk ahead of the handler, with the same modelled error.
    for (case, size) in [("one below the lower bound", 0), ("one past the upper bound", 501)] {
        let (status, refused) = emulator.get(&list_uri(&format!("maxResults={size}"))).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
    }

    // Each published bound itself is served.
    for (case, size) in [("the lower bound", 1), ("the upper bound", MAX_PAGE)] {
        let body = list(&emulator, case, &format!("maxResults={size}")).await;
        let page = reported(&body);
        assert!(
            !page.is_empty() && page.len() <= size.min(fixture.attachments.len()),
            "case {case:?}: a page at this bound holds between one and {size} entries, got {body}"
        );
    }
}
