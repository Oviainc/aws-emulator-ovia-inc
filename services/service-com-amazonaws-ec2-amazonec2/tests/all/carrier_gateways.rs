// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateCarrierGateway` and `DescribeCarrierGateways`, driven over
//! the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by
//! the `ec2` signing name in the request's SigV4 credential scope, so every call here
//! carries one.
//!
//! Two rules are under test:
//!
//! * `DescribeCarrierGateways` REPORTS THE STORE: the set it answers with is the account's
//!   carrier gateways narrowed by the `CarrierGatewayIds` and `Filters` the request names
//!   and cut into `MaxResults`-sized pages. Every table row is a distinct input along what
//!   that generalises over — which filter, how many values, how many filters, which ids,
//!   which page size.
//! * `CreateCarrierGateway` STORES ONE GATEWAY in the VPC the request names, answers it in
//!   the `pending` state the model publishes, and the clock owes it `available` one
//!   [`ec2::state::TRANSITION_DELAY`] later. The rows differ in the VPC, the number of tags
//!   and the client token.

use std::time::Duration;

use ec2::ops::types::Tag;
use ec2::state::{
    CARRIER_GATEWAY_ID_PREFIX, CARRIER_GATEWAY_STATE_AVAILABLE, CARRIER_GATEWAY_STATE_PENDING,
    CarrierGatewaySpec, ComAmazonawsEc2Amazonec2State as Ec2State, MAX_CLIENT_TOKEN_CHARS,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. The carrier gateway store holds the account's
/// gateways rather than one Region's, and the seeded default VPC is the one of the
/// emulator's default Region, so calls are signed for that Region.
const REGION: &str = "us-east-1";

/// Long enough past [`TRANSITION_DELAY`] that a transition the clock owes is due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `DescribeCarrierGateways` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeCarrierGateways", params).await
}

/// One `CreateCarrierGateway` call carrying `params`, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateCarrierGateway", params).await
}

/// The carrier gateway ids a successful describe reports, in the order it reported them.
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
    elements(&body, "carrierGatewayId")
}

/// The XML of the one carrier gateway `carrier_gateway_id` names.
async fn describe_one(router: &MultiServiceRouter, case: &str, carrier_gateway_id: &str) -> String {
    let params = indexed("CarrierGatewayId", &[carrier_gateway_id]);
    let (status, body) = describe(router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// A tag, as the fixtures carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// What one seeded carrier gateway is created with, beyond the id and the owning account
/// the service mints itself.
struct GatewayFixture {
    /// Which VPC of [`Seeded::vpc_ids`] the gateway is associated with.
    vpc_index: usize,
    /// The `CarrierGatewayState` the gateway is stored in.
    state: &'static str,
    tags: Vec<Tag>,
}

/// The gateways the filter and paging tables are written against. They differ along every
/// attribute the documented filters select on: the VPC, the lifecycle state, and the tags.
fn seeded_fixtures() -> Vec<GatewayFixture> {
    vec![
        GatewayFixture {
            vpc_index: 0,
            state: "available",
            tags: vec![tag("Name", "wavelength"), tag("Owner", "TeamA")],
        },
        GatewayFixture {
            vpc_index: 0,
            state: "pending",
            tags: vec![tag("Name", "edge")],
        },
        GatewayFixture {
            vpc_index: 0,
            state: "deleting",
            tags: Vec::new(),
        },
        GatewayFixture {
            vpc_index: 1,
            state: "available",
            tags: vec![tag("Owner", "TeamB")],
        },
        GatewayFixture {
            vpc_index: 1,
            state: "available",
            tags: vec![tag("Name", "wavelength"), tag("Tier", "prod")],
        },
        GatewayFixture {
            vpc_index: 1,
            state: "deleted",
            tags: vec![tag("Name", "edge")],
        },
    ]
}

/// What [`seed`] wrote: the VPCs the gateways are in, and the gateway ids in store order.
struct Seeded {
    vpc_ids: Vec<String>,
    ids: Vec<String>,
}

/// Write the seeded gateways into the engine's store, the way `CreateCarrierGateway`
/// would, and report the ids they were given. Seeding through the store rather than the
/// wire is what lets the filter table cover every `CarrierGatewayState` — a create only
/// ever mints `pending`.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let default_vpc_id = guard
        .vpcs
        .values()
        .find(|vpc| vpc.is_default == Some(true))
        .and_then(|vpc| vpc.vpc_id.clone())
        .expect("a fresh account has its default VPC");
    let second_vpc_id =
        guard.insert_vpc("10.0.0.0/16", false, "default", "available", REGION, Vec::new());
    let vpc_ids = vec![default_vpc_id, second_vpc_id];

    let ids = seeded_fixtures()
        .into_iter()
        .map(|fixture| {
            guard.insert_carrier_gateway(CarrierGatewaySpec {
                vpc_id: vpc_ids[fixture.vpc_index].clone(),
                state: fixture.state.to_string(),
                tags: fixture.tags,
                client_token: None,
            })
        })
        .collect();
    Seeded { vpc_ids, ids }
}

/// The ids of `seeded` at `indices`, which is what a row of the filter table names.
fn ids_at(seeded: &Seeded, indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .map(|index| seeded.ids[*index].clone())
        .collect()
}

/// A VPC created over the wire, by id — so a create can name a VPC the account really has
/// without reaching into the store.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let params = [param("CidrBlock", cidr_block)];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateVpc", &params).await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"))
}

/// The account's default VPC id, read back through `DescribeVpcs`.
async fn default_vpc_id(router: &MultiServiceRouter) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeVpcs",
        &filters(&[("is-default", &["true"])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeVpcs: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("DescribeVpcs answered no vpcId: {body}"))
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account has created no carrier gateway, so the operation reports an EMPTY set —
/// not a fabricated one — and describing is a READ: two calls report the same thing and
/// the store is untouched.
#[tokio::test]
async fn a_fresh_account_reports_no_carrier_gateway_and_reading_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, first) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {first}"
    );
    let first_ids = elements(&first, "carrierGatewayId");
    assert!(
        first_ids.is_empty(),
        "fresh account: expected no carrier gateway, got {first_ids:?}"
    );
    assert!(
        first.contains("carrierGatewaySet"),
        "fresh account: expected a carrierGatewaySet element, got {first}"
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
            .carrier_gateways
            .is_empty(),
        "describing carrier gateways must not write to the store"
    );
}

/// Every gateway the store holds is reported, in store order, with the members it was
/// stored with; a gateway written after a call shows up in the next one — the answer is
/// computed from the store, not fixed.
#[tokio::test]
async fn every_stored_carrier_gateway_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all gateways", &[]).await;
    assert_eq!(
        reported, seeded.ids,
        "all gateways: expected every stored gateway in store order, got {reported:?}"
    );

    let late_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_carrier_gateway(CarrierGatewaySpec {
            vpc_id: seeded.vpc_ids[1].clone(),
            state: CARRIER_GATEWAY_STATE_AVAILABLE.to_string(),
            tags: vec![tag("Name", "late")],
            client_token: None,
        });
    let reported = described_ids(&router, "after a later write", &[]).await;
    assert!(
        reported.contains(&late_id),
        "after a later write: expected {late_id} among {reported:?}"
    );

    let body = describe_one(&router, "the late gateway", &late_id).await;
    for (member, value) in [
        ("carrierGatewayId", late_id.as_str()),
        ("vpcId", seeded.vpc_ids[1].as_str()),
        ("state", CARRIER_GATEWAY_STATE_AVAILABLE),
        ("ownerId", engine::emulator_account_id()),
        ("key", "Name"),
        ("value", "late"),
    ] {
        assert!(
            body.contains(&format!("<{member}>{value}</{member}>")),
            "the late gateway: expected <{member}>{value}</{member}>, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// CreateCarrierGateway, and the round trip through the describe
// ---------------------------------------------------------------------------

/// One row of the create table: the VPC to create in, the tags to ask for, and a client
/// token or none.
struct CreateCase {
    name: &'static str,
    /// Which VPC of the two this test makes the gateway in.
    vpc_index: usize,
    tags: Vec<(&'static str, &'static str)>,
    client_token: Option<&'static str>,
}

/// Each create stores exactly one gateway, in the VPC it named, with the tags it asked
/// for, and `DescribeCarrierGateways` reads every one of them back. The rows differ in the
/// VPC, in how many tags they carry, and in whether they carry a client token.
#[tokio::test]
async fn a_created_carrier_gateway_round_trips_through_the_describe() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_ids = [
        default_vpc_id(&router).await,
        create_vpc(&router, "10.1.0.0/16").await,
    ];

    let cases = vec![
        CreateCase {
            name: "no tags, default VPC",
            vpc_index: 0,
            tags: Vec::new(),
            client_token: None,
        },
        CreateCase {
            name: "one tag",
            vpc_index: 0,
            tags: vec![("Name", "wavelength")],
            client_token: None,
        },
        CreateCase {
            name: "three tags, second VPC",
            vpc_index: 1,
            tags: vec![("Name", "edge"), ("Owner", "TeamA"), ("Tier", "prod")],
            client_token: Some("token-three-tags"),
        },
        CreateCase {
            name: "a tag value with spaces and punctuation",
            vpc_index: 1,
            tags: vec![("Note", "zone A/B, primary")],
            client_token: Some("token-note"),
        },
    ];

    let mut created: Vec<String> = Vec::new();
    for case in &cases {
        let vpc_id = &vpc_ids[case.vpc_index];
        let mut params = vec![param("VpcId", vpc_id)];
        if !case.tags.is_empty() {
            params.push(param("TagSpecification.1.ResourceType", "carrier-gateway"));
            for (index, (key, value)) in case.tags.iter().enumerate() {
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
        }
        if let Some(token) = case.client_token {
            params.push(param("ClientToken", token));
        }

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with body {body}",
            name = case.name
        );
        let id = element(&body, "carrierGatewayId")
            .unwrap_or_else(|| panic!("case {:?}: no carrierGatewayId in {body}", case.name));
        let suffix = id
            .strip_prefix(&format!("{CARRIER_GATEWAY_ID_PREFIX}-"))
            .unwrap_or_else(|| {
                panic!(
                    "case {name:?}: id {id} carries no carrier gateway prefix",
                    name = case.name
                )
            });
        assert!(
            suffix.len() == 17 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "case {name:?}: expected 17 hex digits after the prefix, got {id}",
            name = case.name
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(CARRIER_GATEWAY_STATE_PENDING),
            "case {name:?}: a create answers the pending state, got {body}",
            name = case.name
        );

        // The round trip: the gateway the create answered is the gateway the describe
        // reports, member for member.
        let read_back = describe_one(&router, case.name, &id).await;
        for (member, value) in [
            ("carrierGatewayId", id.as_str()),
            ("vpcId", vpc_id.as_str()),
            ("ownerId", engine::emulator_account_id()),
        ] {
            assert!(
                read_back.contains(&format!("<{member}>{value}</{member}>")),
                "case {name:?}: expected <{member}>{value}</{member}>, got {read_back}",
                name = case.name
            );
        }
        let keys = elements(&read_back, "key");
        let values = elements(&read_back, "value");
        let expected_keys: Vec<String> = case.tags.iter().map(|(key, _)| key.to_string()).collect();
        let expected_values: Vec<String> = case
            .tags
            .iter()
            .map(|(_, value)| value.to_string())
            .collect();
        assert_eq!(
            (keys, values),
            (expected_keys, expected_values),
            "case {name:?}: expected the tags the create asked for, got {read_back}",
            name = case.name
        );
        created.push(id);
    }

    let reported = described_ids(&router, "every created gateway", &[]).await;
    assert_eq!(
        reported, created,
        "every created gateway: expected the ids the creates minted, got {reported:?}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().carrier_gateways.len(),
        cases.len(),
        "each create stores exactly one gateway"
    );
}

/// A create answers `pending` and the clock — not a later call — moves the gateway to
/// `available`: a caller reading straight after the create still sees `pending`.
#[tokio::test]
async fn a_created_gateway_is_pending_and_the_clock_makes_it_available() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = default_vpc_id(&router).await;

    let (status, body) = create(&router, &[param("VpcId", &vpc_id)]).await;
    assert_eq!(
        status, 200,
        "create: expected HTTP 200, got {status} with body {body}"
    );
    let id = element(&body, "carrierGatewayId").expect("a created gateway has an id");

    let before = describe_one(&router, "straight after the create", &id).await;
    assert_eq!(
        element(&before, "state").as_deref(),
        Some(CARRIER_GATEWAY_STATE_PENDING),
        "straight after the create: expected the pending state, got {before}"
    );

    tokio::time::sleep(past_the_transition()).await;

    let after = describe_one(&router, "past the transition", &id).await;
    assert_eq!(
        element(&after, "state").as_deref(),
        Some(CARRIER_GATEWAY_STATE_AVAILABLE),
        "past the transition: expected the available state, got {after}"
    );
}

/// One client token names the one gateway it minted: a repeat of the same request reports
/// that gateway and creates nothing.
#[tokio::test]
async fn a_client_token_makes_the_create_idempotent() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = default_vpc_id(&router).await;
    let params = vec![param("VpcId", &vpc_id), param("ClientToken", "once-only")];

    let (status, first) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "first create: expected HTTP 200, got {status} with body {first}"
    );
    let first_id = element(&first, "carrierGatewayId").expect("a created gateway has an id");

    let (status, second) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "replay: expected HTTP 200, got {status} with body {second}"
    );
    assert_eq!(
        element(&second, "carrierGatewayId").as_deref(),
        Some(first_id.as_str()),
        "replay: expected the gateway the token minted, got {second}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().carrier_gateways.len(),
        1,
        "a replayed client token must not create a second gateway"
    );

    // A DIFFERENT token is a different request, so it mints a second gateway.
    let (status, other) = create(
        &router,
        &[param("VpcId", &vpc_id), param("ClientToken", "another")],
    )
    .await;
    assert_eq!(
        status, 200,
        "a second token: expected HTTP 200, got {status} with body {other}"
    );
    assert_ne!(
        element(&other, "carrierGatewayId").as_deref(),
        Some(first_id.as_str()),
        "a second token: expected a new gateway, got {other}"
    );
}

/// One row of the refusal table: the case name, the form fields to send, and the EC2 error
/// code the refusal must carry.
type RefusalCase = (&'static str, Vec<(String, String)>, &'static str);

/// A create this emulator cannot honour is refused with the EC2 code for what is wrong,
/// and stores nothing: an unknown VPC, a tag specification naming another resource type, a
/// reserved tag key, and a client token past the bound this crate holds one to.
#[tokio::test]
async fn a_create_that_cannot_be_honoured_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = default_vpc_id(&router).await;

    let long_token = "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1);
    let cases: Vec<RefusalCase> = vec![
        (
            "a VPC the account does not have",
            vec![param("VpcId", "vpc-0ffffffffffffffff")],
            "InvalidVpcID.NotFound",
        ),
        (
            "a malformed VPC id",
            vec![param("VpcId", "not-a-vpc")],
            "InvalidVpcID.NotFound",
        ),
        (
            "a tag specification for another resource type",
            vec![
                param("VpcId", &vpc_id),
                param("TagSpecification.1.ResourceType", "subnet"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "nope"),
            ],
            "InvalidParameterValue",
        ),
        (
            "a tag specification with no resource type",
            vec![
                param("VpcId", &vpc_id),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "nope"),
            ],
            "MissingParameter",
        ),
        (
            "a reserved tag key",
            vec![
                param("VpcId", &vpc_id),
                param("TagSpecification.1.ResourceType", "carrier-gateway"),
                param("TagSpecification.1.Tag.1.Key", "aws:created-by"),
                param("TagSpecification.1.Tag.1.Value", "me"),
            ],
            "InvalidParameterValue",
        ),
        (
            "a client token past the maximum",
            vec![param("VpcId", &vpc_id), param("ClientToken", &long_token)],
            "InvalidParameterValue",
        ),
    ];

    for (case, params, expected_code) in cases {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .carrier_gateways
            .is_empty(),
        "a refused create must store nothing"
    );
}

/// A carrier gateway is a resource associated with its VPC, so the VPC cannot be deleted
/// while it is there: "you must detach or delete all gateways and resources that are
/// associated with the VPC before you can delete it" (`DeleteVpc`).
#[tokio::test]
async fn a_carrier_gateway_blocks_deleting_its_vpc() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.2.0.0/16").await;

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "DeleteVpc",
        &[param("VpcId", &vpc_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "an empty VPC: expected HTTP 200, got {status} with body {body}"
    );

    let vpc_id = create_vpc(&router, "10.3.0.0/16").await;
    let (status, body) = create(&router, &[param("VpcId", &vpc_id)]).await;
    assert_eq!(
        status, 200,
        "create: expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "DeleteVpc",
        &[param("VpcId", &vpc_id)],
    )
    .await;
    assert_eq!(
        status, 400,
        "a VPC with a carrier gateway: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DependencyViolation",
        "a VPC with a carrier gateway: expected DependencyViolation, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute; values within one filter are OR'd,
/// the comparison is exact and case-sensitive, and EC2's `*` wildcard applies.
#[tokio::test]
async fn each_documented_filter_selects_the_gateways_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = (0..seeded.ids.len()).collect();

    // One row per filter name `DescribeCarrierGatewaysRequest$Filters` documents, each
    // asserted on gateways the issue this suite was written for never mentioned, plus the
    // OR, the case-sensitivity and the wildcard rows.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        (
            "carrier-gateway-id",
            vec![seeded.ids[2].clone()],
            vec![2],
        ),
        (
            "carrier-gateway-id",
            vec![seeded.ids[0].clone(), seeded.ids[3].clone()],
            vec![0, 3],
        ),
        (
            "carrier-gateway-id",
            vec![format!("{CARRIER_GATEWAY_ID_PREFIX}-*")],
            all.clone(),
        ),
        ("owner-id", vec![account.clone()], all.clone()),
        ("owner-id", vec!["000000000001".to_string()], Vec::new()),
        ("state", vec!["available".to_string()], vec![0, 3, 4]),
        (
            "state",
            vec!["pending".to_string(), "deleting".to_string()],
            vec![1, 2],
        ),
        ("state", vec!["deleted".to_string()], vec![5]),
        ("state", vec!["Available".to_string()], Vec::new()),
        ("vpc-id", vec![seeded.vpc_ids[0].clone()], vec![0, 1, 2]),
        ("vpc-id", vec![seeded.vpc_ids[1].clone()], vec![3, 4, 5]),
        ("tag-key", vec!["Name".to_string()], vec![0, 1, 4, 5]),
        ("tag-key", vec!["Owner".to_string()], vec![0, 3]),
        ("tag-key", vec!["Absent".to_string()], Vec::new()),
        ("tag:Name", vec!["wavelength".to_string()], vec![0, 4]),
        (
            "tag:Name",
            vec!["edge".to_string(), "wavelength".to_string()],
            vec![0, 1, 4, 5],
        ),
        ("tag:Name", vec!["wave*".to_string()], vec![0, 4]),
        ("tag:Owner", vec!["TeamB".to_string()], vec![3]),
        ("tag:Tier", vec!["prod".to_string()], vec![4]),
        ("tag:Name", vec!["WAVELENGTH".to_string()], Vec::new()),
    ];

    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the indices of the gateways that
/// must come back.
type FilterCombination<'a> = (Vec<(&'a str, Vec<String>)>, Vec<usize>);

/// Several filters are joined with an AND: only the gateways matching every one are
/// reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("state", vec!["available".to_string()]),
                ("vpc-id", vec![seeded.vpc_ids[1].clone()]),
            ],
            vec![3, 4],
        ),
        (
            vec![
                ("tag-key", vec!["Name".to_string()]),
                ("state", vec!["available".to_string()]),
            ],
            vec![0, 4],
        ),
        (
            vec![
                ("vpc-id", vec![seeded.vpc_ids[0].clone()]),
                ("tag:Name", vec!["edge".to_string()]),
                ("state", vec!["pending".to_string()]),
            ],
            vec![1],
        ),
        (
            vec![
                ("state", vec!["available".to_string()]),
                ("tag:Owner", vec!["TeamA".to_string()]),
                ("vpc-id", vec![seeded.vpc_ids[1].clone()]),
            ],
            Vec::new(),
        ),
    ];

    for (entries, expected_indices) in cases {
        let case = format!("{entries:?}");
        let borrowed: Vec<(&str, Vec<&str>)> = entries
            .iter()
            .map(|(name, values)| (*name, values.iter().map(String::as_str).collect()))
            .collect();
        let as_slices: Vec<(&str, &[&str])> = borrowed
            .iter()
            .map(|(name, values)| (*name, values.as_slice()))
            .collect();
        let reported = described_ids(&router, &case, &filters(&as_slices)).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name the operation does not define is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: a near-miss of a defined name, a filter of a different describe, the
    // underscore spelling, a defined name with the wrong case, the tag filter without its
    // key separator, the tag filter with a separator but no key, and an empty name.
    let cases = [
        "carrier-gateway-ids",
        "cidr-block",
        "carrier_gateway_id",
        "State",
        "tagName",
        "tag:",
        "",
    ];
    for case in cases {
        let (status, body) = describe(&router, &filters(&[(case, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// CarrierGatewayIds
// ---------------------------------------------------------------------------

/// `CarrierGatewayIds` narrows the report to the ids named, in store order whatever order
/// they were asked for.
#[tokio::test]
async fn carrier_gateway_ids_narrow_the_report_to_the_ids_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows differ in count and in the order the ids are asked for.
    let cases: Vec<Vec<usize>> = vec![
        vec![0],
        vec![5],
        vec![2, 4],
        vec![4, 2],
        vec![0, 1, 2, 3, 4, 5],
        vec![3, 3],
    ];
    for indices in cases {
        let case = format!("{indices:?}");
        let asked: Vec<String> = ids_at(&seeded, &indices);
        let borrowed: Vec<&str> = asked.iter().map(String::as_str).collect();
        let reported = described_ids(&router, &case, &indexed("CarrierGatewayId", &borrowed)).await;
        let mut expected = indices.clone();
        expected.sort_unstable();
        expected.dedup();
        assert_eq!(
            reported,
            ids_at(&seeded, &expected),
            "case {case:?}: expected the ids named in store order, got {reported:?}"
        );
    }
}

/// A `CarrierGatewayIds` entry that names no gateway of the account is refused with
/// `InvalidCarrierGatewayID.NotFound`, naming the id the request sent — even alongside ids
/// that do exist.
#[tokio::test]
async fn a_carrier_gateway_id_that_names_no_gateway_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let known = seeded.ids[0].clone();

    // Rows: a well-formed id nothing has, the short id form, an id of another resource
    // kind, free text, and an unknown id carried beside a known one. Each row names the
    // unknown id the refusal has to report.
    let cases: Vec<(Vec<String>, String)> = vec![
        (
            vec![format!("{CARRIER_GATEWAY_ID_PREFIX}-0ffffffffffffffff")],
            format!("{CARRIER_GATEWAY_ID_PREFIX}-0ffffffffffffffff"),
        ),
        (
            vec![format!("{CARRIER_GATEWAY_ID_PREFIX}-0fffffff")],
            format!("{CARRIER_GATEWAY_ID_PREFIX}-0fffffff"),
        ),
        (
            vec!["vpc-00000000000000001".to_string()],
            "vpc-00000000000000001".to_string(),
        ),
        (vec!["not-an-id".to_string()], "not-an-id".to_string()),
        (
            vec![
                known.clone(),
                format!("{CARRIER_GATEWAY_ID_PREFIX}-0ffffffffffffffff"),
            ],
            format!("{CARRIER_GATEWAY_ID_PREFIX}-0ffffffffffffffff"),
        ),
    ];
    for (asked, unknown) in cases {
        let case = format!("{asked:?}");
        let borrowed: Vec<&str> = asked.iter().map(String::as_str).collect();
        let (status, body) = describe(&router, &indexed("CarrierGatewayId", &borrowed)).await;
        // The message is built from the request: it names the id that could not be
        // resolved, whichever entry of the list that was.
        assert_eq!(
            unescaped(&element(&body, "Message").unwrap_or_default()),
            format!("The CarrierGateway ID '{unknown}' does not exist"),
            "case {case:?}: expected the refusal to name {unknown}, got {body}"
        );
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidCarrierGatewayID.NotFound",
            "case {case:?}: expected InvalidCarrierGatewayID.NotFound, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every gateway exactly once, in store
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_gateway_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `CarrierGatewayMaxResults` is modelled `range 5..1000`, so the rows are its published
    // lower bound, a size that divides the store exactly, one that does not, and the size
    // that holds everything.
    for max_results in [5, 6, 7, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One page per gateway is the most a correct walk can take, so a token that never
        // clears fails the case instead of hanging the suite.
        let page_budget = seeded.ids.len() + 1;
        for page in 0..=page_budget {
            assert!(
                page < page_budget,
                "case {case:?}: the walk took more than {page_budget} pages, so NextToken \
                 never cleared; walked {walked:?}"
            );
            let mut params = vec![param("MaxResults", &max_results.to_string())];
            if let Some(cursor) = token.as_deref() {
                params.push(param("NextToken", cursor));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            walked.extend(elements(&body, "carrierGatewayId"));
            token = elements(&body, "nextToken").first().cloned();
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, seeded.ids,
            "case {case:?}: expected every gateway exactly once in store order, got {walked:?}"
        );
    }
}

/// A token is issued exactly when a gateway follows the page, and resuming from it starts
/// at that gateway — and the page is a cut of the FILTERED set, so `MaxResults` and a
/// filter compose.
#[tokio::test]
async fn a_token_is_issued_only_when_a_gateway_follows_the_page() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[param("MaxResults", "5")]).await;
    assert_eq!(status, 200, "first page: got {status} {body}");
    assert_eq!(
        elements(&body, "carrierGatewayId"),
        ids_at(&seeded, &[0, 1, 2, 3, 4]),
        "first page: expected the first five gateways, got {body}"
    );
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("first page: expected a token, got {body}"));

    let second = described_ids(&router, "second page", &[param("NextToken", &token)]).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[5]),
        "second page: expected the gateway after the first five, got {second:?}"
    );

    // The whole store in one page: no gateway follows it, so no token.
    let (status, body) = describe(&router, &[param("MaxResults", "1000")]).await;
    assert_eq!(status, 200, "one page: got {status} {body}");
    assert!(
        elements(&body, "nextToken").is_empty(),
        "one page: expected no token, got {body}"
    );

    // A page cut from a filtered set, not from the whole store.
    let mut params = filters(&[("vpc-id", &[seeded.vpc_ids[1].as_str()])]);
    params.push(param("MaxResults", "5"));
    let filtered = described_ids(&router, "filtered page", &params).await;
    assert_eq!(
        filtered,
        ids_at(&seeded, &[3, 4, 5]),
        "filtered page: expected one VPC's gateways, got {filtered:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`range 5..1000`) and one past each.
    let cases = [(4, 400), (5, 200), (1000, 200), (1001, 400)];
    for (max_results, expected_status) in cases {
        let params = [param("MaxResults", &max_results.to_string())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`,
/// rather than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, and that id hex-encoded with a wrong
    // checksum (the shape the token takes, without the checksum a minted token carries).
    let forged_marker = ::hex::encode(format!("ec2-1:0000000000000000:{}", seeded.ids[0]));
    let cases = [
        "nonsense".to_string(),
        "!!".to_string(),
        "0a0b0c".to_string(),
        seeded.ids[0].clone(),
        ::hex::encode(seeded.ids[0].as_bytes()),
        forged_marker,
    ];
    for case in cases {
        let (status, body) = describe(&router, &[param("NextToken", &case)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and neither
/// describes nor creates; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_doing_the_work() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (describe_status, describe_body) = describe(&router, &[param("DryRun", "true")]).await;
    let (create_status, create_body) = create(
        &router,
        &[param("VpcId", &seeded.vpc_ids[0]), param("DryRun", "true")],
    )
    .await;
    for (case, status, body) in [
        ("describe", describe_status, describe_body),
        ("create", create_status, create_body),
    ] {
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
    }

    assert_eq!(
        engine.state::<Ec2State>().lock().carrier_gateways.len(),
        seeded.ids.len(),
        "a dry run must not create a gateway"
    );
    let reported = described_ids(&router, "DryRun=false", &[param("DryRun", "false")]).await;
    assert_eq!(
        reported, seeded.ids,
        "DryRun=false: expected an ordinary call, got {reported:?}"
    );
}
