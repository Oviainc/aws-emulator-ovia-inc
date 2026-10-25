// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the PEERING slice — `CreateTransitGatewayPeering` and
//! `GetTransitGatewayPeering` — driven over HTTP through the real router (the harness lives
//! in `crate::global_network_lifecycle`).
//!
//! The rule under test: a transit gateway peering is a stored resource of ONE core network,
//! sitting at the edge of that core network in the Region of the transit gateway it peers
//! with — no request member carries an edge location — so it reports that core network and
//! its ARN, the account that owns it, the one peering type, that edge location, the gateway
//! ARN as both its `TransitGatewayArn` and the peer's `ResourceArn`, and the tags its create
//! carried; it waits in `CREATING` until the transition the clock owes falls due and is
//! `AVAILABLE` after it, and it reports the id of the peering attachment it leaves on that
//! gateway from the stored record, so two reads never disagree about it.
//! `GetTransitGatewayPeering` answers that one stored record and changes nothing a later
//! caller sees; a `PeeringId` naming no stored peering is the modelled
//! `ResourceNotFoundException`.
//!
//! The table below spans that class: rows differing in the Region (and so the edge location)
//! the gateway sits in, in the ARN partition, in whether the gateway belongs to another
//! account, in the width of the gateway id, in how many tags the create carries and in
//! whether it carries a client token. Nothing reaches into `crate::state`: every peering is
//! created and read back through an operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::core_network_at;
use crate::global_network_lifecycle::{
    error_code, expected_arn, is_minted_id, Emulator, PAST_SETTLE,
};
use crate::resource_tagging::{escape, list_tags, tag, tags_uri};

/// The id prefix and ARN resource type of a peering: the model's own
/// `^peering-([0-9a-f]{8,17})$` (`smithy_query shape=PeeringId`).
pub(crate) const PEERING: &str = "peering";

/// The id prefix of the EC2 transit gateway peering attachment a peering establishes:
/// `TransitGatewayPeeringAttachmentId` carries `^tgw-attach-([0-9a-f]{8,17})$`
/// (`smithy_query shape=TransitGatewayPeering`).
const PEERING_ATTACHMENT: &str = "tgw-attach";

/// The one value `PeeringType` carries (`smithy_query shape=PeeringType`).
const TRANSIT_GATEWAY: &str = "TRANSIT_GATEWAY";

/// The edges the core network under test configures, one per Region.
const EDGES: [&str; 3] = ["us-west-2", "eu-central-1", "ap-southeast-4"];

/// A transit gateway ARN. The skeleton is the one every EC2 ARN this model types spells —
/// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$` for `VpcArn`
/// (`smithy_query shape=VpcArn`) — with the `transit-gateway` resource type and a `tgw-` id;
/// `digits` is how many hex digits that id carries, which that pattern writes as 8 to 17.
pub(crate) fn transit_gateway_arn(
    partition: &str,
    region: &str,
    account: &str,
    index: usize,
    digits: usize,
) -> String {
    format!(
        "arn:{partition}:ec2:{region}:{account}:transit-gateway/tgw-{index:0width$x}",
        width = digits
    )
}

/// The account the emulator runs as, which is the account every peering it mints belongs to.
pub(crate) fn own_account() -> String {
    engine::emulator_account_id().to_string()
}

/// One row of the table: the peering one `CreateTransitGatewayPeering` asks for.
struct PeeringCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The Region the transit gateway is in, which is therefore the edge location the
    /// peering sits at. Every one of these is an edge of the core network the test builds.
    region: &'static str,
    /// The ARN partition the gateway's ARN carries.
    partition: &'static str,
    /// The account the gateway belongs to, or `None` for the emulator's own account: a
    /// peering may peer with a gateway of another account.
    account: Option<&'static str>,
    /// How many hex digits the gateway id carries.
    digits: usize,
    /// The tags the create carries.
    tags: &'static [(&'static str, &'static str)],
    /// The `ClientToken` the create carries, or `None` for a create naming none.
    client_token: Option<&'static str>,
}

/// The table: rows differing along everything the rule generalises over — the Region and so
/// the edge location, the partition, whose account the gateway is in, the width of its id,
/// the tags and the client token.
const PEERING_CASES: [PeeringCase; 5] = [
    PeeringCase {
        name: "a gateway of this account, the shortest id the pattern allows, no tags",
        region: "us-west-2",
        partition: "aws",
        account: None,
        digits: 8,
        tags: &[],
        client_token: None,
    },
    PeeringCase {
        name: "a gateway at another edge, the longest id the pattern allows, one tag",
        region: "eu-central-1",
        partition: "aws",
        account: None,
        digits: 17,
        tags: &[("Name", "frankfurt")],
        client_token: Some("token-eu"),
    },
    PeeringCase {
        name: "a gateway of another account, two tags",
        region: "ap-southeast-4",
        partition: "aws",
        account: Some("210987654321"),
        digits: 12,
        tags: &[("Name", "melbourne"), ("Tier", "edge")],
        client_token: None,
    },
    PeeringCase {
        name: "a gateway in another partition",
        region: "us-west-2",
        partition: "aws-us-gov",
        account: None,
        digits: 9,
        tags: &[],
        client_token: Some("token-gov"),
    },
    PeeringCase {
        name: "a second gateway at the first edge, carrying a tag and a token",
        region: "us-west-2",
        partition: "aws",
        account: None,
        digits: 17,
        tags: &[("Name", "oregon-second")],
        client_token: Some("token-second"),
    },
];

/// The gateway ARN the row `case` peers with. `index` keeps the gateways of two rows (and of
/// two subjects) apart, since one gateway is peered once.
fn case_gateway(case: &PeeringCase, index: usize) -> String {
    transit_gateway_arn(
        case.partition,
        case.region,
        case.account.unwrap_or(&own_account()),
        index,
        case.digits,
    )
}

/// The `Tags` the row `case` carries, as the wire reports them.
fn case_tags(case: &PeeringCase) -> Value {
    let tags: Vec<Value> = case
        .tags
        .iter()
        .map(|(key, value)| tag(key, value))
        .collect();
    json!(tags)
}

/// The `CreateTransitGatewayPeering` request body the row `case` asks for.
fn request_of(case: &PeeringCase, core_network: &str, index: usize) -> Value {
    let mut body = json!({
        "CoreNetworkId": core_network,
        "TransitGatewayArn": case_gateway(case, index),
    });
    let object = body
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {:?}: the request body must be an object", case.name));
    if !case.tags.is_empty() {
        object.insert("Tags".to_string(), case_tags(case));
    }
    if let Some(token) = case.client_token {
        object.insert("ClientToken".to_string(), json!(token));
    }
    body
}

/// `CreateTransitGatewayPeering`.
pub(crate) async fn create_peering(emulator: &Emulator, body: Value) -> (u16, Value) {
    emulator.post("/transit-gateway-peerings", body).await
}

/// `GetTransitGatewayPeering`.
async fn get_peering(emulator: &Emulator, id: &str) -> (u16, Value) {
    emulator
        .get(&format!("/transit-gateway-peerings/{id}"))
        .await
}

/// `GetTransitGatewayPeering`, asserting the call succeeded.
pub(crate) async fn get_peering_ok(emulator: &Emulator, case: &str, id: &str) -> Value {
    let (status, body) = get_peering(emulator, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetTransitGatewayPeering expected 200, got {status} with {body}"
    );
    body["TransitGatewayPeering"].clone()
}

/// The string member `member` of `value`, or `None` when the member is absent.
fn member(value: &Value, member: &str) -> Option<String> {
    value[member].as_str().map(str::to_string)
}

/// The `PeeringId` of a `TransitGatewayPeering`.
pub(crate) fn peering_id(case: &str, wrapper: &Value) -> String {
    member(&wrapper["Peering"], "PeeringId")
        .unwrap_or_else(|| panic!("case {case:?}: no Peering.PeeringId in {wrapper}"))
}

/// Assert `peering` is the `TransitGatewayPeering` the row `case` asks for; `state`, when
/// given, is the lifecycle state it must report.
///
/// Everything asserted here is computed from stored state: the core network and its ARN come
/// from the core network the create named, the edge location from the Region inside the
/// gateway's ARN (no request member carries it), the owning account from the emulator's one
/// account, and the rest is what the create carried. The state is asserted only where the
/// caller knows which side of the clock-owed transition it is on, so no assertion here is
/// coupled to how long a round trip took.
fn assert_peering(
    case: &PeeringCase,
    index: usize,
    core_network: &str,
    peering: &Value,
    state: Option<&str>,
) {
    let name = case.name;
    let gateway = case_gateway(case, index);
    let envelope = &peering["Peering"];
    assert_eq!(
        member(envelope, "CoreNetworkId").as_deref(),
        Some(core_network),
        "case {name:?}: expected the core network the create named, got {peering}"
    );
    assert_eq!(
        member(envelope, "CoreNetworkArn"),
        Some(expected_arn("core-network", core_network)),
        "case {name:?}: expected that core network's ARN, got {peering}"
    );
    assert_eq!(
        member(envelope, "OwnerAccountId"),
        Some(own_account()),
        "case {name:?}: expected the account the peering belongs to, got {peering}"
    );
    assert_eq!(
        member(envelope, "PeeringType").as_deref(),
        Some(TRANSIT_GATEWAY),
        "case {name:?}: expected the one modelled peering type, got {peering}"
    );
    if let Some(state) = state {
        assert_eq!(
            member(envelope, "State").as_deref(),
            Some(state),
            "case {name:?}: expected state {state}, got {peering}"
        );
    }
    assert_eq!(
        member(envelope, "EdgeLocation").as_deref(),
        Some(case.region),
        "case {name:?}: expected the Region of the gateway as the edge location, got {peering}"
    );
    assert_eq!(
        member(envelope, "ResourceArn").as_deref(),
        Some(gateway.as_str()),
        "case {name:?}: expected the peered gateway as the peer's resource ARN, got {peering}"
    );
    assert_eq!(
        envelope["Tags"],
        case_tags(case),
        "case {name:?}: expected the tags the create carried, got {peering}"
    );
    assert!(
        envelope["CreatedAt"].as_f64().is_some_and(|at| at > 0.0),
        "case {name:?}: expected a minted CreatedAt timestamp, got {peering}"
    );
    assert_eq!(
        envelope["LastModificationErrors"],
        json!([]),
        "case {name:?}: a peering this service accepted has no modification error, got {peering}"
    );
    assert_eq!(
        member(peering, "TransitGatewayArn").as_deref(),
        Some(gateway.as_str()),
        "case {name:?}: expected the gateway the create named, got {peering}"
    );

    // The peering attachment the gateway side holds is a minted id of the stored record, not
    // a value any request carried.
    let attachment = member(peering, "TransitGatewayPeeringAttachmentId").unwrap_or_else(|| {
        panic!("case {name:?}: expected a TransitGatewayPeeringAttachmentId in {peering}")
    });
    assert!(
        is_minted_id(&attachment, PEERING_ATTACHMENT),
        "case {name:?}: expected a minted peering attachment id, got {attachment:?}"
    );
}

/// Create every row of the table against `core_network`, asserting what each create
/// answered, and answer the minted peering ids in creation order.
async fn create_all(emulator: &Emulator, core_network: &str, base: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for (index, case) in PEERING_CASES.iter().enumerate() {
        let position = base + index;
        let (status, created) =
            create_peering(emulator, request_of(case, core_network, position)).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateTransitGatewayPeering expected 200, got {status} with {created}",
            case.name
        );
        let peering = created["TransitGatewayPeering"].clone();
        let id = peering_id(case.name, &peering);
        assert!(
            is_minted_id(&id, PEERING),
            "case {:?}: expected a minted peering id, got {id:?}",
            case.name
        );
        assert_peering(case, position, core_network, &peering, Some("CREATING"));
        ids.push(id);
    }
    ids
}

#[tokio::test]
async fn a_created_transit_gateway_peering_reads_back_through_get() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "peerings", &EDGES).await;
    let ids = create_all(&emulator, &core_network, 0).await;

    // Each row reads back from the one stored record: nothing the create answered was
    // invented for that response.
    for (index, (case, id)) in PEERING_CASES.iter().zip(&ids).enumerate() {
        let peering = get_peering_ok(&emulator, case.name, id).await;
        assert_eq!(
            peering_id(case.name, &peering),
            *id,
            "case {:?}: the reader answered another peering, got {peering}",
            case.name
        );
        assert_peering(case, index, &core_network, &peering, None);
    }

    // Reading is not a mutation: the same read repeated answers the same peering, and every
    // row is still there afterwards.
    for (index, (case, id)) in PEERING_CASES.iter().zip(&ids).enumerate() {
        let first = get_peering_ok(&emulator, case.name, id).await;
        let again = get_peering_ok(&emulator, case.name, id).await;
        assert_eq!(
            first, again,
            "case {:?}: reading a peering changed what the next caller sees",
            case.name
        );
        assert_peering(case, index, &core_network, &again, None);
    }
}

#[tokio::test]
async fn a_peering_waits_in_creating_until_the_transition_the_clock_owes_falls_due() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "settling", &EDGES).await;
    let ids = create_all(&emulator, &core_network, 100).await;

    tokio::time::sleep(PAST_SETTLE).await;

    for (index, (case, id)) in PEERING_CASES.iter().zip(&ids).enumerate() {
        let peering = get_peering_ok(&emulator, case.name, id).await;
        assert_peering(case, 100 + index, &core_network, &peering, Some("AVAILABLE"));
        let attachment = member(&peering, "TransitGatewayPeeringAttachmentId")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: an available peering reports its peering attachment, got {peering}",
                    case.name
                )
            });
        // The attachment id is stored, not minted per read, so it is stable.
        let again = get_peering_ok(&emulator, case.name, id).await;
        assert_eq!(
            member(&again, "TransitGatewayPeeringAttachmentId"),
            Some(attachment),
            "case {:?}: the peering attachment id changed between reads, got {again}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_peering_id_naming_no_stored_peering_is_refused_as_not_found() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "missing", &EDGES).await;
    let ids = create_all(&emulator, &core_network, 200).await;

    // Each row is a well-formed `PeeringId` (the modelled label pattern) that names no
    // stored peering: one past the ids this run minted, and one drawn from elsewhere in the
    // hex id space.
    for (case, id) in [
        ("never created", "peering-00000000000000099"),
        ("an id of the right shape but the wrong value", "peering-abcdef0123456789a"),
    ] {
        let (status, body) = get_peering(&emulator, id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {body}"
        );
    }

    // The refusals left the stored peerings exactly as they were.
    for (case, id) in PEERING_CASES.iter().zip(&ids) {
        let peering = get_peering_ok(&emulator, case.name, id).await;
        assert_eq!(
            peering_id(case.name, &peering),
            *id,
            "case {:?}: a not-found refusal disturbed a stored peering, got {peering}",
            case.name
        );
    }
}

/// One refused create: the body it sends and what it must be answered with.
struct RefusalCase {
    name: &'static str,
    gateway: String,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn a_create_outside_the_rule_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "refusals", &EDGES).await;
    let ids = create_all(&emulator, &core_network, 300).await;
    let account = own_account();

    // Each row is a different WAY of being outside the rule: a string that is not an ARN, an
    // ARN of another service, an ARN of another EC2 resource type, an ARN carrying no Region,
    // one carrying no partition, one naming the type but no gateway, one with no resource
    // segment at all, a Region the core network configures no edge in, and a gateway already
    // peered.
    let refusals = vec![
        RefusalCase {
            name: "a gateway id instead of an ARN",
            gateway: "tgw-0123456789abcdef1".to_string(),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "an ARN of this service instead of a gateway",
            gateway: expected_arn("core-network", &core_network),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "an ARN of another EC2 resource type",
            gateway: format!("arn:aws:ec2:us-west-2:{account}:vpc/vpc-0123456789abcdef1"),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "an ARN naming no Region",
            gateway: format!("arn:aws:ec2::{account}:transit-gateway/tgw-0123456789abcdef1"),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "an ARN naming no partition",
            gateway: format!("arn::ec2:us-west-2:{account}:transit-gateway/tgw-0123456789abcdef1"),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "an ARN naming the type but no gateway",
            gateway: format!("arn:aws:ec2:us-west-2:{account}:transit-gateway/"),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "an ARN with no resource segment at all",
            gateway: format!("arn:aws:ec2:us-west-2:{account}:transit-gateway"),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "a gateway in a Region the core network configures no edge in",
            gateway: transit_gateway_arn("aws", "sa-east-1", &account, 777, 17),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "a gateway this core network is already peered with",
            gateway: case_gateway(&PEERING_CASES[0], 300),
            status: 409,
            code: "ConflictException",
        },
    ];

    for refusal in &refusals {
        let (status, body) = create_peering(
            &emulator,
            json!({
                "CoreNetworkId": core_network,
                "TransitGatewayArn": refusal.gateway,
            }),
        )
        .await;
        assert_eq!(
            status, refusal.status,
            "case {:?}: expected {}, got {status} with {body}",
            refusal.name, refusal.status
        );
        assert_eq!(
            error_code(&body),
            refusal.code,
            "case {:?}: wrong error code in {body}",
            refusal.name
        );
    }

    // A core network that is not stored is a field failure too, not a peering with a
    // dangling back-reference.
    let (status, body) = create_peering(
        &emulator,
        json!({
            "CoreNetworkId": "core-network-00000000000000099",
            "TransitGatewayArn": transit_gateway_arn("aws", "us-west-2", &account, 888, 17),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an unstored core network\": expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"an unstored core network\": wrong error code in {body}"
    );

    // Nothing was stored by any refusal: the inventory is exactly the rows the table
    // created, each still readable and still peering the gateway it was created with. The
    // lifecycle state is deliberately not asserted here — the settle span the clock owes may
    // have fallen due during the refusals above, and the create and read tests are where the
    // state belongs.
    for (index, (case, id)) in PEERING_CASES.iter().zip(&ids).enumerate() {
        let peering = get_peering_ok(&emulator, case.name, id).await;
        let gateway = case_gateway(case, 300 + index);
        assert_eq!(
            peering_id(case.name, &peering),
            *id,
            "case {:?}: the reader answered another peering, got {peering}",
            case.name
        );
        assert_eq!(
            member(&peering, "TransitGatewayArn").as_deref(),
            Some(gateway.as_str()),
            "case {:?}: a refused create disturbed a stored peering, got {peering}",
            case.name
        );
    }
    assert_eq!(
        peering_count(&emulator, &core_network).await,
        ids.len(),
        "a refused create stored a peering"
    );
}

/// How many peerings the store holds for the global network that owns `core_network`, as
/// `GetNetworkResourceCounts` reports them.
pub(crate) async fn peering_count(emulator: &Emulator, core_network: &str) -> usize {
    let network = global_network_of(emulator, core_network).await;
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/network-resource-count?resourceType={}",
            escape(PEERING)
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetNetworkResourceCounts expected 200, got {status} with {body}"
    );
    let count = body["NetworkResourceCounts"][0]["Count"]
        .as_u64()
        .unwrap_or_else(|| panic!("expected a peering count in {body}"));
    usize::try_from(count).unwrap_or(usize::MAX)
}

/// The global network that owns `core_network`, read back through `GetCoreNetwork`.
async fn global_network_of(emulator: &Emulator, core_network: &str) -> String {
    let (status, body) = emulator
        .get(&format!("/core-networks/{core_network}"))
        .await;
    assert_eq!(
        status, 200,
        "GetCoreNetwork expected 200, got {status} with {body}"
    );
    body["CoreNetwork"]["GlobalNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a GlobalNetworkId in {body}"))
        .to_string()
}

#[tokio::test]
async fn a_client_token_makes_the_create_idempotent() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "tokens", &EDGES).await;
    let case = &PEERING_CASES[1];
    let request = request_of(case, &core_network, 400);

    let (status, first) = create_peering(&emulator, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {:?}: the first create expected 200, got {status} with {first}",
        case.name
    );
    let id = peering_id(case.name, &first["TransitGatewayPeering"]);

    // The same token asking for the same thing answers the record the first call minted.
    for repeat in 1..=2 {
        let (status, again) = create_peering(&emulator, request.clone()).await;
        assert_eq!(
            status, 200,
            "case repeat {repeat}: a retry expected 200, got {status} with {again}"
        );
        assert_eq!(
            peering_id(case.name, &again["TransitGatewayPeering"]),
            id,
            "case repeat {repeat}: a retry minted a second peering, got {again}"
        );
    }

    // The same token asking for something else is a conflict, and stores nothing.
    let (status, body) = create_peering(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "TransitGatewayArn": transit_gateway_arn("aws", "us-west-2", &own_account(), 401, 17),
            "ClientToken": case.client_token,
        }),
    )
    .await;
    assert_eq!(
        status, 409,
        "case \"a reused token asking for another peering\": expected 409, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case \"a reused token asking for another peering\": wrong error code in {body}"
    );
    assert_eq!(
        peering_count(&emulator, &core_network).await,
        1,
        "a conflicted retry stored a second peering"
    );
}

#[tokio::test]
async fn a_stored_peering_is_a_network_resource_of_its_core_network_and_carries_tags() {
    let emulator = Emulator::new();
    let (network, core_network) = core_network_at(&emulator, "inventory", &EDGES).await;
    let case = &PEERING_CASES[2];
    let (status, created) = create_peering(&emulator, request_of(case, &core_network, 500)).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateTransitGatewayPeering expected 200, got {status} with {created}",
        case.name
    );
    let id = peering_id(case.name, &created["TransitGatewayPeering"]);
    let arn = expected_arn(PEERING, &id);

    // The inventory reports the peering under its own resource type, with the ARN of this
    // service's `peering` resource and the core network it belongs to.
    let (status, listed) = emulator
        .get(&format!(
            "/global-networks/{network}/network-resources?resourceType={}",
            escape(PEERING)
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetNetworkResources expected 200, got {status} with {listed}"
    );
    let resources = listed["NetworkResources"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a NetworkResources array in {listed}"));
    assert_eq!(
        resources.len(),
        1,
        "case {:?}: expected the one stored peering, got {listed}",
        case.name
    );
    let resource = &resources[0];
    assert_eq!(
        member(resource, "ResourceId").as_deref(),
        Some(id.as_str()),
        "case {:?}: expected the peering's id, got {resource}",
        case.name
    );
    assert_eq!(
        member(resource, "ResourceArn").as_deref(),
        Some(arn.as_str()),
        "case {:?}: expected the peering's ARN, got {resource}",
        case.name
    );
    assert_eq!(
        member(resource, "CoreNetworkId").as_deref(),
        Some(core_network.as_str()),
        "case {:?}: expected the core network the peering belongs to, got {resource}",
        case.name
    );
    assert_eq!(
        resource["Tags"],
        case_tags(case),
        "case {:?}: expected the tags the create carried, got {resource}",
        case.name
    );

    // A peering is a taggable resource of this service, and the tags a later `TagResource`
    // merges are the ones its reader then reports.
    let (status, body) = emulator
        .post(&tags_uri(&arn), json!({ "Tags": [tag("Stage", "prod")] }))
        .await;
    assert_eq!(
        status, 200,
        "TagResource expected 200, got {status} with {body}"
    );
    let (status, tags) = list_tags(&emulator, &arn).await;
    assert_eq!(
        status, 200,
        "ListTagsForResource expected 200, got {status} with {tags}"
    );
    let mut expected: Vec<Value> = case
        .tags
        .iter()
        .map(|(key, value)| tag(key, value))
        .collect();
    expected.push(tag("Stage", "prod"));
    assert_eq!(
        tags,
        json!(expected),
        "case {:?}: expected the merged tag set, got {tags}",
        case.name
    );
    let peering = get_peering_ok(&emulator, case.name, &id).await;
    assert_eq!(
        peering["Peering"]["Tags"],
        json!(expected),
        "case {:?}: expected the peering's reader to report the merged tags, got {peering}",
        case.name
    );
}
