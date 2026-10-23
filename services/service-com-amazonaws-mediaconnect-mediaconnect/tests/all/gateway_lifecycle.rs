// SPDX-License-Identifier: CC0-1.0

//! The gateway lifecycle, end to end over HTTP: CreateGateway -> DescribeGateway ->
//! ListGateways -> DeleteGateway.
//!
//! The subject is ListGateways: it must report the gateways the store HOLDS — every one
//! of them, each exactly once, in one stable order — a page at a time, rather than a
//! value fixed in the handler. The cases below therefore vary what the rule generalises
//! over: how many gateways are stored, the page size (across the modelled `MaxResults`
//! range and its default), whether a gateway was created or deleted, and what shape the
//! pagination token has.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches: routing,
//! restJson1 deserialization, constraint validation, handler, serialization.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::GatewayNetwork;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};

/// Every ARN CreateGateway mints must satisfy the modelled `GatewayArn` constraint
/// (`smithy_query(query="shape",shape="GatewayArn")`: `^arn:.+:mediaconnect.+:gateway:.+$`),
/// because DescribeGateway and DeleteGateway take it as an `@httpLabel` and the framework
/// validates the member against that pattern.
///
/// The check runs through the crate's own encoding of the form it mints — the same
/// function ListGateways validates a pagination token with — and then re-asserts, field
/// by field, the parts that form fixes, so a bug in that function cannot hide here.
fn assert_minted_gateway_arn(case: &str, arn: &str, name: &str) {
    assert!(
        mc::is_mintable_gateway_arn(arn),
        "case {case:?}: minted GatewayArn must be one this service could mint, got {arn:?}"
    );
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        8,
        "case {case:?}: a gateway ARN has 8 colon-separated segments, got {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[2], segments[3], segments[5]),
        ("arn", "mediaconnect", REGION, "gateway"),
        "case {case:?}: ARN prefix, service, Region and resource type, got {arn:?}"
    );
    assert!(
        segments[1].starts_with("aws"),
        "case {case:?}: the partition is an aws partition, got {arn:?}"
    );
    assert!(
        segments[4].len() == 12 && segments[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: the account id is 12 digits, got {arn:?}"
    );
    assert_eq!(
        segments[7], name,
        "case {case:?}: the last segment is the gateway name, got {arn:?}"
    );
}

/// Put `count` gateways straight into the store, the way CreateGateway stores one.
///
/// The store ceiling is a bound on what is STORED, so the suite that asserts it needs a
/// full store rather than a thousand round trips: the calls that matter (the one that
/// fills the last slot, the one past the bound) still go over the wire.
fn seed_gateways(emulator: &Emulator, count: usize) {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    for _ in 0..count {
        let suffix = store.next_suffix();
        let name = format!("seeded-gateway-{suffix}");
        let gateway_arn = engine::emulator_arn(
            mc::SERVICE_NAMESPACE,
            REGION,
            &mc::gateway_resource_fragment(suffix, &name),
        );
        store.gateways.insert(
            gateway_arn.clone(),
            mc::StoredGateway {
                gateway_arn,
                name,
                egress_cidr_blocks: vec!["10.0.0.0/16".to_string()],
                // One network, because that is the smallest a gateway CreateGateway
                // would accept carries: a seeded record is shaped like a created one.
                networks: vec![GatewayNetwork {
                    name: "net-1".to_string(),
                    cidr_block: "10.0.0.0/24".to_string(),
                }],
                gateway_state: mc::GATEWAY_STATE_ACTIVE.to_string(),
                pending: None,
                // A seeded gateway is live, like a created one: only DeleteGateway
                // retires a record.
                deleted_at: None,
            },
        );
    }
}

async fn create_gateway(emulator: &Emulator, body: Value) -> (u16, Option<String>, Value) {
    emulator.send("POST", "/v1/gateways", Some(body)).await
}

async fn describe_gateway(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/gateways/{arn}"), None)
        .await
}

async fn delete_gateway(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/gateways/{arn}"), None)
        .await
}

async fn list_gateways(emulator: &Emulator, query: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/gateways{query}"), None)
        .await
}

/// The `Gateways` member of a ListGateways response, as `(GatewayArn, Name, GatewayState)`.
fn listed(body: &Value) -> Vec<(String, String, String)> {
    body["gateways"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|gateway| {
            (
                gateway["gatewayArn"].as_str().unwrap_or_default().to_string(),
                gateway["name"].as_str().unwrap_or_default().to_string(),
                gateway["gatewayState"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// One CreateGateway request: the name, the egress CIDR blocks and the `(name, cidrBlock)`
/// of each network.
struct CreateCase {
    case: &'static str,
    name: &'static str,
    egress_cidr_blocks: Vec<&'static str>,
    networks: Vec<(&'static str, &'static str)>,
}

impl CreateCase {
    fn body(&self) -> Value {
        json!({
            "name": self.name,
            "egressCidrBlocks": self.egress_cidr_blocks,
            "networks": self
                .networks
                .iter()
                .map(|(name, cidr_block)| json!({ "name": name, "cidrBlock": cidr_block }))
                .collect::<Vec<Value>>(),
        })
    }
}

/// The create requests the rules below are asserted over. The rows differ in the count of
/// networks (one, and the documented maximum of four), the count of egress CIDR blocks,
/// and the form of the gateway name.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "one network, one egress block",
            name: "gw-single",
            egress_cidr_blocks: vec!["10.0.0.0/16"],
            networks: vec![("net-1", "10.0.0.0/16")],
        },
        CreateCase {
            case: "the documented maximum of four networks",
            name: "gw_max_networks",
            egress_cidr_blocks: vec!["10.1.0.0/16", "192.168.0.0/24"],
            networks: vec![
                ("net-1", "10.1.0.0/24"),
                ("net-2", "10.1.1.0/24"),
                ("net-3", "10.1.2.0/24"),
                ("net-4", "10.1.3.0/24"),
            ],
        },
        CreateCase {
            case: "three egress blocks and a dotted name",
            name: "gw.dotted.name",
            egress_cidr_blocks: vec!["10.2.0.0/16", "10.3.0.0/16", "203.0.113.0/24"],
            networks: vec![("net-a", "10.2.0.0/24"), ("net-b", "10.2.1.0/24")],
        },
    ]
}

/// CreateGateway stores the gateway it returns: DescribeGateway reports the same record
/// and ListGateways summarises it. Every member of the response comes from the store, so
/// what the create answered is what the later reads report.
#[tokio::test]
async fn create_persists_the_gateway_describe_and_list_report() {
    let emulator = Emulator::new();

    for case in create_cases() {
        let CreateCase {
            case: name_of_case,
            name,
            ref egress_cidr_blocks,
            ref networks,
        } = case;
        let (status, code, created) = create_gateway(&emulator, case.body()).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {name_of_case:?}: create expected 201, got {status} {code:?}: {created}"
        );

        let gateway = &created["gateway"];
        let gateway_arn = gateway["gatewayArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name_of_case:?}: the create answers an ARN: {created}"))
            .to_string();
        assert_minted_gateway_arn(name_of_case, &gateway_arn, name);
        assert_eq!(
            gateway["name"].as_str(),
            Some(name),
            "case {name_of_case:?}: the stored gateway keeps its name, got {gateway}"
        );
        assert_eq!(
            gateway["gatewayState"].as_str(),
            Some(mc::GATEWAY_STATE_CREATING),
            "case {name_of_case:?}: a new gateway is reported in its transient state, got \
             {gateway}"
        );
        assert_eq!(
            gateway["egressCidrBlocks"],
            json!(egress_cidr_blocks),
            "case {name_of_case:?}: the egress CIDR blocks are stored as requested, got {gateway}"
        );
        assert_eq!(
            gateway["networks"],
            json!(networks
                .iter()
                .map(|(name, cidr_block)| json!({ "name": name, "cidrBlock": cidr_block }))
                .collect::<Vec<Value>>()),
            "case {name_of_case:?}: every requested network is stored, got {gateway}"
        );

        // The round trip: DescribeGateway reports the record the create stored. The state
        // is the one member the CLOCK may have moved on in between (see the transition
        // test below), so it is asserted to be one of the two states this lifecycle
        // reaches rather than compared byte for byte.
        let (status, code, described) = describe_gateway(&emulator, &gateway_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {name_of_case:?}: describe expected 200, got {status} {code:?}: {described}"
        );
        for member in ["gatewayArn", "name", "egressCidrBlocks", "networks"] {
            assert_eq!(
                described["gateway"][member], gateway[member],
                "case {name_of_case:?}: DescribeGateway must report the created {member}, got {:?}",
                described["gateway"][member]
            );
        }
        let described_state = described["gateway"]["gatewayState"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            [mc::GATEWAY_STATE_CREATING, mc::GATEWAY_STATE_ACTIVE].contains(&described_state.as_str()),
            "case {name_of_case:?}: a described gateway is CREATING or ACTIVE, got \
             {described_state:?}"
        );

        // And ListGateways summarises the same record.
        let (status, _, list) = list_gateways(&emulator, "?maxResults=1000").await;
        assert_eq!(
            status, 200,
            "case {name_of_case:?}: list expected 200, got {status}: {list}"
        );
        let summary = listed(&list)
            .into_iter()
            .find(|(arn, ..)| arn == &gateway_arn)
            .unwrap_or_else(|| {
                panic!("case {name_of_case:?}: the new gateway must be listed, got {list}")
            });
        assert_eq!(
            (summary.0.as_str(), summary.1.as_str()),
            (gateway_arn.as_str(), name),
            "case {name_of_case:?}: the listed summary must name the stored gateway"
        );
        assert!(
            [mc::GATEWAY_STATE_CREATING, mc::GATEWAY_STATE_ACTIVE].contains(&summary.2.as_str()),
            "case {name_of_case:?}: a listed gateway is CREATING or ACTIVE, got {:?}",
            summary.2
        );
    }
}

/// A new gateway is answered in the non-terminal `CREATING` state and the CLOCK settles it
/// into `ACTIVE`; both reads report the state the store holds, so the listed state moves
/// with the described one. DeleteGateway then RETIRES the record — `GatewayState` carries a
/// terminal `DELETED` value, so the gateway stays readable — and the deletion lifecycle is
/// observable the same way: `DELETING`, then the clock settles `DELETED`.
#[tokio::test]
async fn the_clock_settles_a_new_gateway_and_delete_retires_it() {
    let emulator = Emulator::new();
    let (status, _, created) = create_gateway(
        &emulator,
        json!({
            "name": "gw-lifecycle",
            "egressCidrBlocks": ["10.0.0.0/16"],
            "networks": [{ "name": "net-1", "cidrBlock": "10.0.0.0/24" }]
        }),
    )
    .await;
    assert_eq!(status, 201, "create expected 201, got {status}: {created}");
    let gateway_arn = created["gateway"]["gatewayArn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    // A caller acting straight after the create still sees the transient state.
    let (_, _, described) = describe_gateway(&emulator, &gateway_arn).await;
    assert_eq!(
        described["gateway"]["gatewayState"].as_str(),
        Some(mc::GATEWAY_STATE_CREATING),
        "a gateway read straight after the create is still CREATING, got {described}"
    );

    // Wait out the transition the clock owes, then the settled state is reported — by
    // both readers, because both compute it from the same record.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let (_, _, described) = describe_gateway(&emulator, &gateway_arn).await;
    assert_eq!(
        described["gateway"]["gatewayState"].as_str(),
        Some(mc::GATEWAY_STATE_ACTIVE),
        "the clock settles the gateway into ACTIVE, got {described}"
    );
    let (_, _, list) = list_gateways(&emulator, "").await;
    assert!(
        listed(&list)
            .iter()
            .any(|(arn, _, state)| arn == &gateway_arn && state == mc::GATEWAY_STATE_ACTIVE),
        "ListGateways reports the settled state, got {list}"
    );

    let (status, code, deleted) = delete_gateway(&emulator, &gateway_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "delete expected 200, got {status} {code:?}: {deleted}"
    );
    assert_eq!(
        deleted["gatewayArn"].as_str(),
        Some(gateway_arn.as_str()),
        "delete reports the gateway it retired, got {deleted}"
    );

    // A caller reading straight after the delete still sees the transient state, and the
    // record is still there: a deleted gateway is readable, never a 404.
    let (status, code, described) = describe_gateway(&emulator, &gateway_arn).await;
    assert_eq!(
        (status, code.as_deref(), described["gateway"]["gatewayState"].as_str()),
        (200, None, Some(mc::GATEWAY_STATE_DELETING)),
        "a gateway read straight after the delete is 200 DELETING, got {status} {code:?}: \
         {described}"
    );

    tokio::time::sleep(Duration::from_millis(900)).await;
    let (status, code, described) = describe_gateway(&emulator, &gateway_arn).await;
    assert_eq!(
        (status, code.as_deref(), described["gateway"]["gatewayState"].as_str()),
        (200, None, Some(mc::GATEWAY_STATE_DELETED)),
        "the clock settles the gateway into the terminal DELETED, got {status} {code:?}: \
         {described}"
    );
    // The whole record is still projected, not a husk.
    assert_eq!(
        described["gateway"]["name"].as_str(),
        Some("gw-lifecycle"),
        "a retired gateway still reports its name, got {described}"
    );
    let (_, _, list) = list_gateways(&emulator, "").await;
    assert!(
        listed(&list)
            .iter()
            .any(|(arn, _, state)| arn == &gateway_arn && state == mc::GATEWAY_STATE_DELETED),
        "ListGateways reports the record the store holds, DELETED state and all, got {list}"
    );

    // DeleteGateway is `@idempotent`: repeating it against the already-retired gateway
    // succeeds again, with the same ARN, and leaves the terminal state where it was.
    let (status, code, deleted_again) = delete_gateway(&emulator, &gateway_arn).await;
    assert_eq!(
        (status, code.as_deref(), deleted_again["gatewayArn"].as_str()),
        (200, None, Some(gateway_arn.as_str())),
        "a repeated delete succeeds again, got {status} {code:?}: {deleted_again}"
    );
    let (_, _, described) = describe_gateway(&emulator, &gateway_arn).await;
    assert_eq!(
        described["gateway"]["gatewayState"].as_str(),
        Some(mc::GATEWAY_STATE_DELETED),
        "a repeated delete does not restart the deletion, got {described}"
    );
}

/// Two gateways may share one name: the model puts no uniqueness rule on a gateway's
/// name, and the gateway id in the ARN keeps the records apart. Both are stored, both are
/// listed, and each answers DescribeGateway on its own ARN.
#[tokio::test]
async fn two_gateways_may_share_one_name() {
    let emulator = Emulator::new();
    let shared_name = "gw-shared-name";
    let mut arns: Vec<String> = Vec::new();
    for attempt in 0..2 {
        let (status, code, created) = create_gateway(
            &emulator,
            json!({
                "name": shared_name,
                "egressCidrBlocks": ["10.0.0.0/16"],
                "networks": [{ "name": "net-1", "cidrBlock": "10.0.0.0/24" }]
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "attempt {attempt}: a repeated gateway name is accepted, got {status} {code:?}: \
             {created}"
        );
        let arn = created["gateway"]["gatewayArn"]
            .as_str()
            .unwrap_or_else(|| panic!("attempt {attempt}: the create answers an ARN: {created}"))
            .to_string();
        assert_minted_gateway_arn("shared name", &arn, shared_name);
        arns.push(arn);
    }
    assert_ne!(
        arns[0], arns[1],
        "two gateways sharing a name get their own ARNs, got {arns:?}"
    );

    for arn in &arns {
        let (status, code, described) = describe_gateway(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "each same-named gateway answers on its own ARN, got {status} {code:?}: {described}"
        );
        assert_eq!(
            described["gateway"]["gatewayArn"].as_str(),
            Some(arn.as_str()),
            "describe reports the gateway the ARN names, got {described}"
        );
    }
    let (_, _, list) = list_gateways(&emulator, "?maxResults=1000").await;
    let same_named: Vec<String> = listed(&list)
        .into_iter()
        .filter(|(_, name, _)| name == shared_name)
        .map(|(arn, ..)| arn)
        .collect();
    assert_eq!(
        same_named.len(),
        2,
        "both same-named gateways are listed, got {list}"
    );
}

/// ListGateways pages the persisted gateways: whatever `MaxResults` is, the pages
/// partition every stored gateway exactly once, in one stable order, and only the last
/// page omits `NextToken`. With no `MaxResults` the page holds the documented default of
/// ten.
#[tokio::test]
async fn list_gateways_pages_every_stored_gateway_exactly_once() {
    let emulator = Emulator::new();
    // More than the documented default page size, so the default is observable.
    let total = 12;
    for index in 0..total {
        let (status, _, body) = create_gateway(
            &emulator,
            json!({
                "name": format!("gw-page-{index:02}"),
                "egressCidrBlocks": ["10.0.0.0/16"],
                "networks": [{ "name": "net-1", "cidrBlock": "10.0.0.0/24" }]
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "gateway {index} expected 201, got {status}: {body}"
        );
    }

    let (_, _, all) = list_gateways(&emulator, "?maxResults=1000").await;
    let expected: Vec<String> = listed(&all).into_iter().map(|(arn, ..)| arn).collect();
    assert_eq!(
        expected.len(),
        total,
        "every created gateway is listed, got {expected:?}"
    );

    // "If MaxResults is not included in the request, the service defaults to pagination
    // with a maximum of 10 results per page"
    // (`smithy_query(query="doc",shape="ListGatewaysRequest",member="MaxResults")`).
    let (status, _, default_page) = list_gateways(&emulator, "").await;
    assert_eq!(status, 200, "the default page expected 200, got {status}");
    let default_arns: Vec<String> = listed(&default_page)
        .into_iter()
        .map(|(arn, ..)| arn)
        .collect();
    assert_eq!(
        default_arns,
        expected[..10].to_vec(),
        "a request with no MaxResults holds the first ten stored gateways, got {default_page}"
    );
    assert_eq!(
        default_page["nextToken"].as_str(),
        Some(expected[10].as_str()),
        "the default page carries the ARN the next page resumes at, got {default_page}"
    );

    // The page size spans the modelled `MaxResults` range (1..1000): a page smaller than
    // the set, ones that divide it, one that equals it, and the maximum.
    for page_size in [1_usize, 2, 5, 12, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let (status, code, body) = list_gateways(&emulator, &query).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "page size {page_size}: expected 200, got {status} {code:?}: {body}"
            );
            let page: Vec<String> = listed(&body).into_iter().map(|(arn, ..)| arn).collect();
            token = body["nextToken"].as_str().map(ToString::to_string);
            if token.is_some() {
                assert_eq!(
                    page.len(),
                    page_size.min(total),
                    "page size {page_size}: a page that carries a token is full, got {body}"
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= total + 1,
                "page size {page_size}: paging must terminate, saw {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "page size {page_size}: the pages must partition every stored gateway once, in order"
        );
    }

    // A token stays usable when the gateway it names is deleted between pages: it is the
    // ARN the listing RESUMES AT. A gateway delete is a soft delete, so the retired record
    // is still listed — at the same key, in the same order — and the resumed page starts
    // with it.
    let (_, _, first_page) = list_gateways(&emulator, "?maxResults=3").await;
    let token = first_page["nextToken"]
        .as_str()
        .expect("a page smaller than the set carries a token")
        .to_string();
    let (status, _, deleted) = delete_gateway(&emulator, &token).await;
    assert_eq!(
        status, 200,
        "deleting the gateway the token names expected 200, got {status}: {deleted}"
    );
    let (status, code, resumed) =
        list_gateways(&emulator, &format!("?maxResults=1000&nextToken={token}")).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "resuming at a deleted gateway's ARN expected 200, got {status} {code:?}: {resumed}"
    );
    let resumed_arns: Vec<String> = listed(&resumed).into_iter().map(|(arn, ..)| arn).collect();
    assert_eq!(
        resumed_arns,
        expected[3..].to_vec(),
        "the listing resumes at the key the token names, got {resumed}"
    );
}

/// One refusal case: the request, and the status and error code it must answer with.
struct RefusalCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
}

/// Every gateway operation refuses an unknown gateway with `NotFoundException` (404) and
/// an ARN that fails the modelled pattern with `BadRequestException` (400) — never with a
/// 501 and never with a success. CreateGateway refuses a request that breaks a documented
/// rule of the class, and ListGateways refuses a page size outside the modelled range and
/// a pagination token it could not have issued.
#[tokio::test]
async fn refusals_name_the_modelled_error() {
    let emulator = Emulator::new();

    // Well-formed per the modelled pattern, but no such gateway.
    let missing = "arn:aws:mediaconnect:us-east-1:000000000000:gateway:1-deadbeef:missing";
    // Fails the modelled `GatewayArn` pattern.
    let malformed = "not-an-arn";
    let networks = json!([{ "name": "net-1", "cidrBlock": "10.0.0.0/24" }]);

    let cases = vec![
        RefusalCase {
            case: "describe an unknown gateway",
            method: "GET",
            uri: format!("/v1/gateways/{missing}"),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "delete an unknown gateway",
            method: "DELETE",
            uri: format!("/v1/gateways/{missing}"),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "describe an ARN that fails the modelled pattern",
            method: "GET",
            uri: format!("/v1/gateways/{malformed}"),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "delete an ARN that fails the modelled pattern",
            method: "DELETE",
            uri: format!("/v1/gateways/{malformed}"),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        // "The request must include at least one network (up to four)."
        RefusalCase {
            case: "create a gateway with no network",
            method: "POST",
            uri: "/v1/gateways".to_string(),
            body: Some(json!({
                "name": "gw-no-network",
                "egressCidrBlocks": ["10.0.0.0/16"],
                "networks": []
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "create a gateway with one network past the documented maximum",
            method: "POST",
            uri: "/v1/gateways".to_string(),
            body: Some(json!({
                "name": "gw-five-networks",
                "egressCidrBlocks": ["10.0.0.0/16"],
                "networks": [
                    { "name": "net-1", "cidrBlock": "10.0.0.0/24" },
                    { "name": "net-2", "cidrBlock": "10.0.1.0/24" },
                    { "name": "net-3", "cidrBlock": "10.0.2.0/24" },
                    { "name": "net-4", "cidrBlock": "10.0.3.0/24" },
                    { "name": "net-5", "cidrBlock": "10.0.4.0/24" }
                ]
            })),
            status: 400,
            code: "BadRequestException",
        },
        // `GatewayNetwork$Name` "must be unique among networks in this gateway".
        RefusalCase {
            case: "create a gateway whose networks share a name",
            method: "POST",
            uri: "/v1/gateways".to_string(),
            body: Some(json!({
                "name": "gw-duplicate-network",
                "egressCidrBlocks": ["10.0.0.0/16"],
                "networks": [
                    { "name": "net-1", "cidrBlock": "10.0.0.0/24" },
                    { "name": "net-1", "cidrBlock": "10.0.1.0/24" }
                ]
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "create a gateway with no egress CIDR block",
            method: "POST",
            uri: "/v1/gateways".to_string(),
            body: Some(json!({
                "name": "gw-no-egress",
                "egressCidrBlocks": [],
                "networks": networks.clone()
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "create a gateway with an empty name",
            method: "POST",
            uri: "/v1/gateways".to_string(),
            body: Some(json!({
                "name": "",
                "egressCidrBlocks": ["10.0.0.0/16"],
                "networks": networks.clone()
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "create a gateway whose name cannot be an ARN segment",
            method: "POST",
            uri: "/v1/gateways".to_string(),
            body: Some(json!({
                "name": "bad:name",
                "egressCidrBlocks": ["10.0.0.0/16"],
                "networks": networks.clone()
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "create a gateway whose name cannot be addressed in a URI",
            method: "POST",
            uri: "/v1/gateways".to_string(),
            body: Some(json!({
                "name": "bad/name",
                "egressCidrBlocks": ["10.0.0.0/16"],
                "networks": networks.clone()
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "create a gateway with no name at all",
            method: "POST",
            uri: "/v1/gateways".to_string(),
            body: Some(json!({
                "egressCidrBlocks": ["10.0.0.0/16"],
                "networks": networks.clone()
            })),
            status: 400,
            code: "BadRequestException",
        },
        // The modelled `MaxResults` range is 1..1000, so each bound is usable (the paging
        // suite above lists with 1 and with 1000) and each value past it is refused.
        RefusalCase {
            case: "list with MaxResults below the modelled range",
            method: "GET",
            uri: "/v1/gateways?maxResults=0".to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "list with MaxResults above the modelled range",
            method: "GET",
            uri: "/v1/gateways?maxResults=1001".to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        // A `NextToken` is the gateway ARN the listing resumes at, so a token the service
        // could not have minted is refused. The rows below are the ways a token can fail
        // that form: not an ARN at all, another service's ARN, an account id that is not
        // 12 digits, the wrong resource type, and an empty gateway name.
        RefusalCase {
            case: "list with a nextToken that is not an ARN",
            method: "GET",
            uri: "/v1/gateways?nextToken=not-a-token".to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "list with a nextToken naming another service",
            method: "GET",
            uri: "/v1/gateways?nextToken=arn:aws:medialive:us-east-1:000000000000:gateway:1-abcdef12:g"
                .to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "list with a nextToken whose account id is too short",
            method: "GET",
            uri: "/v1/gateways?nextToken=arn:aws:mediaconnect:us-east-1:00000000000:gateway:1-abcdef12:g"
                .to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "list with a nextToken naming another resource type",
            method: "GET",
            uri: "/v1/gateways?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:flow:1-abcdef12:f"
                .to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "list with a nextToken whose gateway name is empty",
            method: "GET",
            uri: "/v1/gateways?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:gateway:1-abcdef12:"
                .to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
    ];

    for RefusalCase {
        case,
        method,
        uri,
        body,
        status: expected_status,
        code: expected_code,
    } in cases
    {
        let (status, code, response) = emulator.send(method, &uri, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {response}"
        );
        assert_eq!(
            response["__type"].as_str(),
            Some(expected_code),
            "case {case:?}: the error envelope names the shape, got {response}"
        );
        assert!(
            response["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: the refusal carries a message, got {response}"
        );
    }
}

/// Every collection ONE CreateGateway request can fill is bounded, so a single request
/// cannot store a record of caller-chosen size: a request at the bound is stored and
/// echoed whole by DescribeGateway, one entry past it is refused with the modelled
/// `BadRequestException` (400) and stores nothing. The rows are the two caller-sized lists
/// of a gateway record — its networks (bounded by the operation's own documentation) and
/// its egress CIDR blocks (bounded by this crate's request-collection bound).
#[tokio::test]
async fn create_gateway_bounds_every_request_sized_collection() {
    /// A request with `egress_blocks` egress CIDR blocks and `networks` networks.
    fn body(name: &str, egress_blocks: usize, networks: usize) -> Value {
        json!({
            "name": name,
            "egressCidrBlocks": (0..egress_blocks)
                .map(|index| format!("10.{}.{}.0/24", index / 256, index % 256))
                .collect::<Vec<String>>(),
            "networks": (0..networks)
                .map(|index| json!({
                    "name": format!("net-{index}"),
                    "cidrBlock": format!("10.{}.{}.0/24", index / 256, index % 256),
                }))
                .collect::<Vec<Value>>(),
        })
    }

    let emulator = Emulator::new();
    let cases = [
        (
            "EgressCidrBlocks",
            mc::MAX_GATEWAY_EGRESS_CIDR_BLOCKS,
            body("gw-egress-at-bound", mc::MAX_GATEWAY_EGRESS_CIDR_BLOCKS, 1),
            body(
                "gw-egress-past-bound",
                mc::MAX_GATEWAY_EGRESS_CIDR_BLOCKS + 1,
                1,
            ),
        ),
        (
            "Networks",
            mc::MAX_GATEWAY_NETWORKS,
            body("gw-networks-at-bound", 1, mc::MAX_GATEWAY_NETWORKS),
            body("gw-networks-past-bound", 1, mc::MAX_GATEWAY_NETWORKS + 1),
        ),
    ];

    for (member, limit, at_bound, past_bound) in cases {
        let (status, code, created) = create_gateway(&emulator, at_bound.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {member:?} at the bound ({limit}): expected 201, got {status} {code:?}: \
             {created}"
        );
        let gateway_arn = created["gateway"]["gatewayArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {member:?}: the create answers an ARN: {created}"))
            .to_string();
        let (_, _, described) = describe_gateway(&emulator, &gateway_arn).await;
        assert_eq!(
            described["gateway"]["egressCidrBlocks"], at_bound["egressCidrBlocks"],
            "case {member:?} at the bound: the stored egress blocks are reported whole, got {:?}",
            described["gateway"]["egressCidrBlocks"]
        );
        assert_eq!(
            described["gateway"]["networks"], at_bound["networks"],
            "case {member:?} at the bound: the stored networks are reported whole, got {:?}",
            described["gateway"]["networks"]
        );

        let past_bound_name = past_bound["name"].as_str().unwrap_or_default().to_string();
        let (status, code, refused) = create_gateway(&emulator, past_bound).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {member:?} one past the bound ({}): expected 400 BadRequestException, got \
             {status} {code:?}: {refused}",
            limit + 1
        );
        let (_, _, all) = list_gateways(&emulator, "?maxResults=1000").await;
        assert!(
            !listed(&all).iter().any(|(_, name, _)| name == &past_bound_name),
            "case {member:?}: a refused request stores nothing, got {all}"
        );
    }
}

/// The gateway store is BOUNDED: CreateGateway refuses once the store already holds
/// [`mc::MAX_GATEWAYS`] gateways, with the modelled `CreateGateway420Exception` (HTTP 420)
/// and without storing anything, while the gateways that were created are still readable.
/// Deleting one makes room for exactly one more, so the bound is a ceiling on what is
/// stored, not a counter of calls.
#[tokio::test]
async fn create_gateway_refuses_past_the_store_ceiling() {
    const CEILING: usize = mc::MAX_GATEWAYS;

    let emulator = Emulator::new();
    // One slot short of the ceiling, set up directly; the calls at the boundary below go
    // through the real request path.
    seed_gateways(&emulator, CEILING - 1);
    let body = |name: &str| {
        json!({
            "name": name,
            "egressCidrBlocks": ["10.0.0.0/16"],
            "networks": [{ "name": "net-1", "cidrBlock": "10.0.0.0/24" }]
        })
    };

    let (status, code, created) = create_gateway(&emulator, body("gw-bound-last")).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the call that fills the last slot expected 201, got {status} {code:?}: {created}"
    );
    let last_arn = created["gateway"]["gatewayArn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    let (status, code, refused) = create_gateway(&emulator, body("gw-past-bound")).await;
    assert_eq!(
        (status, code.as_deref()),
        (420, Some("CreateGateway420Exception")),
        "one gateway past the ceiling expected 420 CreateGateway420Exception, got {status} \
         {code:?}: {refused}"
    );

    // Nothing was stored by the refused call, and what was created is still readable.
    let (status, code, described) = describe_gateway(&emulator, &last_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "after the refusal the stored gateways are still readable, got {status} {code:?}: \
         {described}"
    );
    let (status, _, page) = list_gateways(&emulator, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "after the refusal ListGateways still answers, got {status}: {page}"
    );

    // Deleting one gateway makes room for exactly one more: the bound counts what is
    // stored, not how many calls were made.
    let (status, _, deleted) = delete_gateway(&emulator, &last_arn).await;
    assert_eq!(status, 200, "delete expected 200, got {status}: {deleted}");
    let (status, code, body_after) = create_gateway(&emulator, body("gw-after-delete")).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "a freed slot takes exactly one more gateway, got {status} {code:?}: {body_after}"
    );
    let (status, code, refused_again) = create_gateway(&emulator, body("gw-past-bound-again")).await;
    assert_eq!(
        (status, code.as_deref()),
        (420, Some("CreateGateway420Exception")),
        "the store is full again, got {status} {code:?}: {refused_again}"
    );
}
