// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListPeerings`, driven over HTTP through the real router (the
//! harness lives in `crate::global_network_lifecycle`, the core-network-with-edges helper in
//! `crate::attachment_lifecycle` and the peering helpers in
//! `crate::transit_gateway_peering`).
//!
//! The rule under test: `ListPeerings` reports the stored `Peering` envelope of every
//! peering the emulator holds, in peering-id order; each of `CoreNetworkId`, `PeeringType`,
//! `EdgeLocation` and `State` NARROWS that listing, so a value naming nothing stored selects
//! no peering rather than failing the read; the envelope reported is the one
//! `GetTransitGatewayPeering` answers for the same record, the state being the one the
//! peering is in as of the call; the listing is paged by `MaxResults` and resumed by a
//! `NextToken` this service minted, and a token it did not mint is refused as a 400
//! `ValidationException`; and the read leaves the store exactly as the next caller would have
//! found it.
//!
//! The tables below span that class: peerings differing in core network (three of them), in
//! edge location (five), in ARN partition, in whose account the peered gateway is in, in the
//! width of its id, and in whether the create carried tags or a client token; filters
//! differing in which member is named, in how many are combined, and in whether the value
//! names anything stored; states differing in which side of the clock-owed transition the
//! call is on, plus the two states nothing here is in; page sizes at the modelled bounds and
//! either side of the number of peerings there are; and tokens differing in how they fail to
//! be one this service issued. Nothing reaches into `crate::state`: every peering is created
//! and read back through an operation.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::attachment_lifecycle::core_network_at;
use crate::global_network_lifecycle::{error_code, ids_of, Emulator, PAST_SETTLE};
use crate::resource_tagging::{escape, tag};
use crate::transit_gateway_peering::{
    create_peering, get_peering_ok, own_account, peering_id, transit_gateway_arn,
};

/// The one value `PeeringType` enumerates (`smithy_query shape=PeeringType`), which is
/// therefore the only type a `peeringType` filter can select on.
const TRANSIT_GATEWAY: &str = "TRANSIT_GATEWAY";

/// The `PeeringState` values this suite selects on (`smithy_query shape=PeeringState`): the
/// two the clock decides, and the two nothing in the fixture is ever in.
const CREATING: &str = "CREATING";
/// See [`CREATING`].
const AVAILABLE: &str = "AVAILABLE";
/// See [`CREATING`].
const DELETING: &str = "DELETING";
/// See [`CREATING`].
const FAILED: &str = "FAILED";

/// The widest page size the model admits: `MaxResults` carries `range 1..500`
/// (`smithy_query query="operation" operation="ListPeerings"`).
const MAX_PAGE: usize = 500;

/// The `ListPeerings` route (`GET /peerings`).
const LIST_URI: &str = "/peerings";

/// The edge locations the fixture's core networks configure.
const WEST: &str = "us-west-2";
/// See [`WEST`].
const CENTRAL: &str = "eu-central-1";
/// See [`WEST`].
const SOUTHEAST: &str = "ap-southeast-4";
/// See [`WEST`].
const EAST: &str = "sa-east-1";
/// See [`WEST`].
const NORTHEAST: &str = "ap-northeast-1";

/// The `ListPeerings` URI carrying `query` (already percent-encoded).
fn list_uri(query: &str) -> String {
    if query.is_empty() {
        LIST_URI.to_string()
    } else {
        format!("{LIST_URI}?{query}")
    }
}

/// `ListPeerings` with `query`, asserting it is served, and the body it answered.
async fn list(emulator: &Emulator, case: &str, query: &str) -> Value {
    let (status, body) = emulator.get(&list_uri(query)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPeerings expected 200, got {status} with {body}"
    );
    body
}

/// The peering ids one page reports, in the order reported.
fn reported(body: &Value) -> Vec<String> {
    ids_of(body, "Peerings", "PeeringId")
}

/// One peering of the fixture: which core network it is on and what its create carries.
struct PeeringSeed {
    /// The label every assertion names this peering by.
    label: &'static str,
    /// Which of the fixture's core networks the peering is on (index into
    /// [`Fixture::core_networks`]).
    core_network: usize,
    /// The Region of the peered transit gateway, which is therefore the edge location the
    /// peering sits at. Every one of these is an edge of that core network.
    region: &'static str,
    /// The ARN partition the gateway's ARN carries.
    partition: &'static str,
    /// The account the gateway belongs to, or `None` for the emulator's own account.
    account: Option<&'static str>,
    /// How many hex digits the gateway id carries (the modelled `tgw-[0-9a-f]{8,17}`).
    digits: usize,
    /// The tags the create carries.
    tags: &'static [(&'static str, &'static str)],
    /// The `ClientToken` the create carries, or `None` for a create naming none.
    client_token: Option<&'static str>,
}

/// The peerings the fixture creates: rows differing along everything the rule generalises
/// over — the core network, the edge location, the partition, whose account the peered
/// gateway is in, the width of its id, the tags and the client token.
const SEEDS: [PeeringSeed; 6] = [
    PeeringSeed {
        label: "west, this account, the shortest gateway id, no tags",
        core_network: 0,
        region: WEST,
        partition: "aws",
        account: None,
        digits: 8,
        tags: &[],
        client_token: None,
    },
    PeeringSeed {
        label: "west, another partition, a tag and a token",
        core_network: 0,
        region: WEST,
        partition: "aws-us-gov",
        account: None,
        digits: 17,
        tags: &[("Name", "oregon-gov")],
        client_token: Some("token-west-gov"),
    },
    PeeringSeed {
        label: "central, a gateway of another account, two tags",
        core_network: 0,
        region: CENTRAL,
        partition: "aws",
        account: Some("210987654321"),
        digits: 12,
        tags: &[("Name", "frankfurt"), ("Tier", "edge")],
        client_token: None,
    },
    PeeringSeed {
        label: "southeast, the longest gateway id",
        core_network: 1,
        region: SOUTHEAST,
        partition: "aws",
        account: None,
        digits: 17,
        tags: &[],
        client_token: Some("token-southeast"),
    },
    PeeringSeed {
        label: "east, one tag",
        core_network: 1,
        region: EAST,
        partition: "aws",
        account: None,
        digits: 9,
        tags: &[("Name", "sao-paulo")],
        client_token: None,
    },
    PeeringSeed {
        label: "northeast, the only peering of its core network",
        core_network: 2,
        region: NORTHEAST,
        partition: "aws",
        account: None,
        digits: 10,
        tags: &[],
        client_token: None,
    },
];

/// The `CreateTransitGatewayPeering` body the row `seed` asks for. `index` keeps the
/// gateways of two rows apart, since one gateway is peered once.
fn request_of(seed: &PeeringSeed, core_network: &str, index: usize) -> Value {
    let mut body = json!({
        "CoreNetworkId": core_network,
        "TransitGatewayArn": transit_gateway_arn(
            seed.partition,
            seed.region,
            seed.account.unwrap_or(&own_account()),
            index,
            seed.digits,
        ),
    });
    let object = body
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {:?}: the request body must be an object", seed.label));
    if !seed.tags.is_empty() {
        let tags: Vec<Value> = seed
            .tags
            .iter()
            .map(|(key, value)| tag(key, value))
            .collect();
        object.insert("Tags".to_string(), json!(tags));
    }
    if let Some(token) = seed.client_token {
        object.insert("ClientToken".to_string(), json!(token));
    }
    body
}

/// The fixture: the peerings created, each under the label every assertion names it by, and
/// the core networks they sit on.
struct Fixture {
    /// Label to minted peering id.
    peerings: BTreeMap<&'static str, String>,
    /// The three core networks, in the order [`seed`] creates them.
    core_networks: [String; 3],
}

impl Fixture {
    /// The ids of `labels`, in the order a listing reports them (ascending `PeeringId`, the
    /// order the store keys peerings in).
    fn ids(&self, labels: &[&str]) -> Vec<String> {
        let mut ids: Vec<String> = labels
            .iter()
            .map(|label| {
                self.peerings
                    .get(label)
                    .unwrap_or_else(|| panic!("the fixture must hold the peering {label:?}"))
                    .clone()
            })
            .collect();
        ids.sort();
        ids
    }

    /// Every label the fixture holds.
    fn all_labels(&self) -> Vec<&'static str> {
        self.peerings.keys().copied().collect()
    }
}

/// Create the fixture: three core networks — one with edges in [`WEST`] and [`CENTRAL`], one
/// in [`SOUTHEAST`] and [`EAST`], one in [`NORTHEAST`] — and the peerings [`SEEDS`] names on
/// them.
async fn seed(emulator: &Emulator) -> Fixture {
    let first = core_network_at(emulator, "first peering network", &[WEST, CENTRAL])
        .await
        .1;
    let second = core_network_at(emulator, "second peering network", &[SOUTHEAST, EAST])
        .await
        .1;
    let third = core_network_at(emulator, "third peering network", &[NORTHEAST])
        .await
        .1;
    let core_networks = [first, second, third];

    let mut peerings: BTreeMap<&'static str, String> = BTreeMap::new();
    for (index, row) in SEEDS.iter().enumerate() {
        let core_network = &core_networks[row.core_network];
        let (status, created) = create_peering(emulator, request_of(row, core_network, index)).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateTransitGatewayPeering expected 200, got {status} with {created}",
            row.label
        );
        peerings.insert(
            row.label,
            peering_id(row.label, &created["TransitGatewayPeering"]),
        );
    }

    Fixture {
        peerings,
        core_networks,
    }
}

/// One row of the filter table: the query a caller sends and the labels it must select.
struct FilterCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The query string, built from the fixture's minted ids.
    query: String,
    /// The labels the listing must report (the assertion sorts them by id, which is the
    /// order the listing uses).
    expected: Vec<&'static str>,
}

/// The table: one row per member a filter names, per combination of members, and per value
/// that names nothing stored.
fn filter_cases(fixture: &Fixture) -> Vec<FilterCase> {
    let [first, second, third] = &fixture.core_networks;
    vec![
        FilterCase {
            name: "no filter at all reports every stored peering",
            query: String::new(),
            expected: fixture.all_labels(),
        },
        FilterCase {
            name: "the first core network, which holds three peerings",
            query: format!("coreNetworkId={first}"),
            expected: vec![
                "central, a gateway of another account, two tags",
                "west, another partition, a tag and a token",
                "west, this account, the shortest gateway id, no tags",
            ],
        },
        FilterCase {
            name: "the second core network",
            query: format!("coreNetworkId={second}"),
            expected: vec!["east, one tag", "southeast, the longest gateway id"],
        },
        FilterCase {
            name: "the third core network, which holds one peering",
            query: format!("coreNetworkId={third}"),
            expected: vec!["northeast, the only peering of its core network"],
        },
        FilterCase {
            name: "the one modelled peering type, across core networks",
            query: format!("peeringType={TRANSIT_GATEWAY}"),
            expected: fixture.all_labels(),
        },
        FilterCase {
            name: "the west edge, where two peerings sit",
            query: format!("edgeLocation={WEST}"),
            expected: vec![
                "west, another partition, a tag and a token",
                "west, this account, the shortest gateway id, no tags",
            ],
        },
        FilterCase {
            name: "the central edge",
            query: format!("edgeLocation={CENTRAL}"),
            expected: vec!["central, a gateway of another account, two tags"],
        },
        FilterCase {
            name: "the southeast edge",
            query: format!("edgeLocation={SOUTHEAST}"),
            expected: vec!["southeast, the longest gateway id"],
        },
        FilterCase {
            name: "the east edge",
            query: format!("edgeLocation={EAST}"),
            expected: vec!["east, one tag"],
        },
        FilterCase {
            name: "the northeast edge",
            query: format!("edgeLocation={NORTHEAST}"),
            expected: vec!["northeast, the only peering of its core network"],
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
            name: "the first core network's peerings at the west edge",
            query: format!("coreNetworkId={first}&edgeLocation={WEST}"),
            expected: vec![
                "west, another partition, a tag and a token",
                "west, this account, the shortest gateway id, no tags",
            ],
        },
        FilterCase {
            name: "all three value selectors at once",
            query: format!(
                "coreNetworkId={first}&peeringType={TRANSIT_GATEWAY}&edgeLocation={CENTRAL}"
            ),
            expected: vec!["central, a gateway of another account, two tags"],
        },
        FilterCase {
            name: "a core network holding peerings only at other edges",
            query: format!("coreNetworkId={second}&edgeLocation={WEST}"),
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
async fn an_emulator_holding_no_peering_reports_the_empty_listing() {
    let emulator = Emulator::new();

    // Nothing has been created, so the listing is the empty one the store really holds —
    // not a refusal and not a placeholder entry.
    for (case, query) in [
        ("no filter at all", String::new()),
        (
            "a core network id nothing is stored for",
            "coreNetworkId=core-network-0000000000000ffff".to_string(),
        ),
        (
            "the peering type nothing was created of",
            format!("peeringType={TRANSIT_GATEWAY}"),
        ),
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
async fn list_peerings_reports_the_stored_peerings_each_filter_selects() {
    let emulator = Emulator::new();
    let fixture = seed(&emulator).await;
    // The peerings the clock owes a transition are past it, so what every row below reports
    // is stable while the table is walked.
    tokio::time::sleep(PAST_SETTLE).await;

    for case in filter_cases(&fixture) {
        let name = case.name;
        let body = list(&emulator, name, &case.query).await;
        let wanted = fixture.ids(&case.expected);
        let got = reported(&body);
        assert_eq!(
            got, wanted,
            "case {name:?}: expected the peerings {:?} in id order, got {got:?} from {body}",
            case.expected
        );
        assert!(
            body["NextToken"].is_null(),
            "case {name:?}: a page naming no size reports everything, so no token is owed, got {body}"
        );
    }
}

#[tokio::test]
async fn the_envelope_a_listing_reports_is_the_one_the_peering_s_own_reader_answers() {
    let emulator = Emulator::new();
    let fixture = seed(&emulator).await;
    tokio::time::sleep(PAST_SETTLE).await;

    for label in fixture.all_labels() {
        let id = fixture.ids(&[label]).remove(0);
        // The whole listing, so the entry compared against the reader is the one an
        // unfiltered caller sees.
        let body = list(&emulator, label, "").await;
        let listed = body["Peerings"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: expected a Peerings array in {body}"))
            .iter()
            .find(|entry| entry["PeeringId"] == json!(id))
            .unwrap_or_else(|| panic!("case {label:?}: {id} must be listed, got {body}"))
            .clone();

        let read = get_peering_ok(&emulator, label, &id).await;
        assert_eq!(
            listed, read["Peering"],
            "case {label:?}: the listed envelope must be the one GetTransitGatewayPeering \
             answers, got {listed}"
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
async fn list_peerings_selects_on_the_state_each_peering_is_in() {
    let emulator = Emulator::new();
    // A small fixture, so the states the clock decides are asserted well inside the span it
    // owes the transition over.
    let core_network = core_network_at(&emulator, "state peering network", &[WEST, CENTRAL])
        .await
        .1;
    let mut ids = Vec::new();
    for (index, region) in [WEST, CENTRAL].iter().enumerate() {
        let (status, created) = create_peering(
            &emulator,
            json!({
                "CoreNetworkId": core_network,
                "TransitGatewayArn": transit_gateway_arn(
                    "aws",
                    region,
                    &own_account(),
                    100 + index,
                    8,
                ),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: CreateTransitGatewayPeering expected 200, got {status} with {created}"
        );
        ids.push(peering_id(region, &created["TransitGatewayPeering"]));
    }
    ids.sort();

    // Before the transition the clock owes falls due: both peerings are selected by the
    // state they are in and by no other, and a state nothing is in selects nothing.
    for (case, state, wanted) in [
        (
            "the state the clock owes a transition from",
            CREATING,
            ids.clone(),
        ),
        ("the state nothing has reached yet", AVAILABLE, Vec::new()),
        ("the state of nothing being deleted", DELETING, Vec::new()),
        ("a state nothing was put in", FAILED, Vec::new()),
    ] {
        let body = list(&emulator, case, &format!("state={state}")).await;
        assert_eq!(
            reported(&body),
            wanted,
            "case {case:?}: expected {wanted:?} in state {state}, got {body}"
        );
    }

    tokio::time::sleep(PAST_SETTLE).await;

    // After it: both have moved, and the state they left selects nothing.
    for (case, state, wanted) in [
        ("the state the clock moved them to", AVAILABLE, ids.clone()),
        ("the state they left", CREATING, Vec::new()),
    ] {
        let body = list(&emulator, case, &format!("state={state}")).await;
        assert_eq!(
            reported(&body),
            wanted,
            "case {case:?}: expected {wanted:?} in state {state}, got {body}"
        );
    }

    // A state filter combines with the others the same way: the peering is reported only
    // where every selector holds.
    let body = list(
        &emulator,
        "state and edge location together",
        &format!("state={AVAILABLE}&edgeLocation={SOUTHEAST}"),
    )
    .await;
    assert_eq!(
        reported(&body),
        Vec::<String>::new(),
        "an available peering at another edge is not reported, got {body}"
    );

    // A deleted peering is no longer stored, so no state selects it any more.
    let gone = ids.remove(0);
    let (status, deleted) = emulator.delete(&format!("/peerings/{gone}")).await;
    assert_eq!(
        status, 200,
        "DeletePeering expected 200, got {status} with {deleted}"
    );
    for (case, query) in [
        ("the whole listing", String::new()),
        ("the state it was in", format!("state={AVAILABLE}")),
        ("the state a delete reports", format!("state={DELETING}")),
    ] {
        let body = list(&emulator, case, &query).await;
        assert!(
            !reported(&body).contains(&gone),
            "case {case:?}: the deleted peering {gone} must not be listed, got {body}"
        );
    }
}

#[tokio::test]
async fn list_peerings_pages_the_listing_with_the_tokens_it_mints() {
    let emulator = Emulator::new();
    let fixture = seed(&emulator).await;
    tokio::time::sleep(PAST_SETTLE).await;

    let wanted = fixture.ids(&fixture.all_labels());
    assert!(
        wanted.len() > 2,
        "the fixture must hold enough peerings to page over, got {wanted:?}"
    );

    // Each row: a page size. 1 is the modelled lower bound, 500 the upper one, and the sizes
    // between them straddle the number of peerings there are to report.
    for size in [1, 2, 4, wanted.len(), wanted.len() + 1, MAX_PAGE] {
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
            "case {case:?}: following the tokens must report every peering exactly once"
        );
        let expected_pages = wanted.len().div_ceil(size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {} peerings at {size} per page is {expected_pages} pages, walked {pages}",
            wanted.len()
        );
    }

    // A page size narrowed by a filter pages only what the filter selects.
    let at_west = fixture.ids(&[
        "west, another partition, a tag and a token",
        "west, this account, the shortest gateway id, no tags",
    ]);
    let mut walked: Vec<String> = Vec::new();
    let mut query = format!("edgeLocation={WEST}&maxResults=1");
    loop {
        let body = list(&emulator, "one west peering per page", &query).await;
        walked.extend(reported(&body));
        match body["NextToken"].as_str() {
            Some(token) => {
                query = format!("edgeLocation={WEST}&maxResults=1&nextToken={}", escape(token));
            }
            None => break,
        }
    }
    assert_eq!(
        walked, at_west,
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

    // Each row: a `NextToken` this service did not mint. All of them are refused on the
    // member rather than answered with a page: HTTP 400 `ValidationException`, which this
    // operation models (`smithy_query query="operation" operation="ListPeerings"`).
    for (case, token) in [
        ("a token that is not hex at all", "not-a-token".to_string()),
        // `6162636465` is the hex of "abcde", which carries none of the prefix this
        // service's own tokens do.
        (
            "a token that is hex but names no listing of this service",
            "6162636465".to_string(),
        ),
        (
            "a token of this service with its head cut off",
            issued[2..].to_string(),
        ),
        ("the issued token re-cased", issued.to_uppercase()),
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
    // refused by the constraint walk ahead of the handler.
    for (case, size) in [
        ("one below the lower bound", 0),
        ("one past the upper bound", 501),
    ] {
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
            !page.is_empty() && page.len() <= size.min(fixture.peerings.len()),
            "case {case:?}: a page at this bound holds between one and {size} entries, got {body}"
        );
    }
}
