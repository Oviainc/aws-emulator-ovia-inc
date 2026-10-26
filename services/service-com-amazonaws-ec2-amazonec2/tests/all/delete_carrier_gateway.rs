// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteCarrierGateway`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DeleteCarrierGateway`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rule under test is that the operation DELETES FROM THE STORE and reports what it
//! removed as the delete leaves it: the gateway `CarrierGatewayId` names leaves the
//! account's state, the response carries that gateway exactly as the describe reported it
//! until this call except for its `State`, which is the `CarrierGatewayState` a deletion
//! puts a gateway into (`deleting`), every other gateway is left alone, a
//! `CarrierGatewayId` the account holds no gateway for is refused rather than answered as a
//! delete that did nothing, and a VPC is released by its gateways only once the LAST of
//! them is deleted.
//!
//! Every table row is a distinct input along what that generalises over — which gateway of
//! the seeded set (so: which VPC, which `CarrierGatewayState` it was stored in, how many
//! tags it carries, whether a `ClientToken` minted it), in which order the gateways are
//! deleted, how many gateways one VPC holds, and each way a value can fail to name a
//! gateway — so no single recorded call is the specification.

use ec2::ops::types::Tag;
use ec2::state::{
    CARRIER_GATEWAY_ID_PREFIX, CARRIER_GATEWAY_STATE_AVAILABLE, CARRIER_GATEWAY_STATE_DELETING,
    CARRIER_GATEWAY_STATE_PENDING, CarrierGatewaySpec,
    ComAmazonawsEc2Amazonec2State as Ec2State, DEPENDENCY_VIOLATION, DRY_RUN_OPERATION,
    INVALID_CARRIER_GATEWAY_ID_NOT_FOUND,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, indexed};

/// The Region every call here is signed for: the carrier gateway store holds the account's
/// gateways, and the seeded default VPC is the one of the emulator's default Region.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because anything was wrong with it.
const DRY_RUN_STATUS: u16 = 412;

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `DeleteCarrierGateway` call carrying `params`, as its HTTP status and XML body.
async fn delete(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DeleteCarrierGateway", params).await
}

/// One `DeleteCarrierGateway` call naming `carrier_gateway_id`.
async fn delete_gateway(router: &MultiServiceRouter, carrier_gateway_id: &str) -> (u16, String) {
    delete(router, &[param("CarrierGatewayId", carrier_gateway_id)]).await
}

/// The gateway ids `DescribeCarrierGateways` reports for `params`, in the order it reported
/// them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = call(router, REGION, "DescribeCarrierGateways", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the describe, got {status} with body {body}"
    );
    elements(&body, "carrierGatewayId")
}

/// The XML `DescribeCarrierGateways` answers for the one gateway `carrier_gateway_id` names.
async fn describe_one(router: &MultiServiceRouter, case: &str, carrier_gateway_id: &str) -> String {
    let params = indexed("CarrierGatewayId", &[carrier_gateway_id]);
    let (status, body) = call(router, REGION, "DescribeCarrierGateways", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the describe of {carrier_gateway_id}, got \
         {status} with body {body}"
    );
    body
}

/// The `<Message>` of a refusal, read out of the PARSED XML envelope: the body has to be
/// well-formed XML and the element's own text is what is answered, so a value the service
/// wrote into the markup unescaped cannot read back as the message it belongs to.
fn refusal_message(case: &str, body: &str) -> String {
    let document = roxmltree::Document::parse(body).unwrap_or_else(|error| {
        panic!("case {case:?}: a refusal must be well-formed XML ({error}): {body}")
    });
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "Message")
        .and_then(|node| node.text())
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {body}"))
        .to_string()
}

/// How many carrier gateways the store holds.
fn stored_gateways(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().carrier_gateways.len()
}

/// Every member of the one modelled `CarrierGateway` an XML body carries
/// (`smithy_query(query="shape",shape="CarrierGateway")`), so what a delete reports can be
/// compared with what the describe reported for the same gateway without a member-by-member
/// assertion at each call site.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedGateway {
    /// `CarrierGateway$CarrierGatewayId`.
    carrier_gateway_id: Option<String>,
    /// `CarrierGateway$VpcId`.
    vpc_id: Option<String>,
    /// `CarrierGateway$State`.
    state: Option<String>,
    /// `CarrierGateway$OwnerId`.
    owner_id: Option<String>,
    /// `CarrierGateway$Tags`, as `(key, value)` pairs in the order the body wrote them.
    tags: Vec<(String, String)>,
}

/// The one gateway `xml` reports. The tags are read out of their own wrapper element
/// (`tagSet`), so a gateway that carries none reads as an empty list rather than borrowing
/// another element's items.
fn reported_gateway(xml: &str) -> ReportedGateway {
    let tag_set = element(xml, "tagSet").unwrap_or_default();
    let keys = elements(&tag_set, "key");
    let values = elements(&tag_set, "value");
    ReportedGateway {
        carrier_gateway_id: element(xml, "carrierGatewayId"),
        vpc_id: element(xml, "vpcId"),
        state: element(xml, "state"),
        owner_id: element(xml, "ownerId"),
        tags: keys.into_iter().zip(values).collect(),
    }
}

/// One seeded gateway: everything a create decides about it, beyond the id and the owning
/// account the service mints itself.
struct Fixture {
    case: &'static str,
    /// Which of the two seeded VPCs the gateway is associated with.
    vpc_index: usize,
    /// The `CarrierGatewayState` the gateway is stored in.
    state: &'static str,
    tags: Vec<(&'static str, &'static str)>,
    /// The `ClientToken` that minted it, if any.
    client_token: Option<&'static str>,
}

/// The gateways every table here is written against. They differ along everything a delete
/// could plausibly be sensitive to: the VPC, the `CarrierGatewayState` the gateway was
/// stored in (every member of the enum a stored gateway can hold), how many tags it carries,
/// and whether a `ClientToken` minted it.
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            case: "a pending gateway with no tags",
            vpc_index: 0,
            state: CARRIER_GATEWAY_STATE_PENDING,
            tags: Vec::new(),
            client_token: None,
        },
        Fixture {
            case: "an available gateway a client token minted",
            vpc_index: 0,
            state: CARRIER_GATEWAY_STATE_AVAILABLE,
            tags: vec![("Name", "wavelength")],
            client_token: Some("seeded-token"),
        },
        Fixture {
            case: "an available gateway in the second VPC with two tags",
            vpc_index: 1,
            state: CARRIER_GATEWAY_STATE_AVAILABLE,
            tags: vec![("Name", "edge"), ("Owner", "TeamB")],
            client_token: None,
        },
        Fixture {
            case: "a gateway already deleting",
            vpc_index: 1,
            state: CARRIER_GATEWAY_STATE_DELETING,
            tags: Vec::new(),
            client_token: Some("another-token"),
        },
        Fixture {
            case: "a deleted gateway with three tags",
            vpc_index: 1,
            state: "deleted",
            tags: vec![("Name", "retired"), ("Owner", "TeamA"), ("Tier", "prod")],
            client_token: None,
        },
    ]
}

/// Write the seeded gateways into the engine's store, the way `CreateCarrierGateway` would,
/// and answer the ids they were given in store order. Seeding through the store rather than
/// the wire is what lets the tables cover every `CarrierGatewayState` a stored gateway can
/// hold — a create only ever mints `pending`.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let default_vpc_id = guard
        .vpcs
        .values()
        .find(|vpc| vpc.is_default == Some(true))
        .and_then(|vpc| vpc.vpc_id.clone())
        .expect("a fresh account has its default VPC");
    let second_vpc_id = guard.insert_vpc(
        "10.80.0.0/16",
        false,
        "default",
        "available",
        REGION,
        Vec::new(),
    );
    let vpc_ids = [default_vpc_id, second_vpc_id];

    fixtures()
        .into_iter()
        .map(|fixture| {
            guard.insert_carrier_gateway(CarrierGatewaySpec {
                vpc_id: vpc_ids[fixture.vpc_index].clone(),
                state: fixture.state.to_string(),
                tags: fixture
                    .tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: Some((*key).to_string()),
                        value: Some((*value).to_string()),
                    })
                    .collect(),
                client_token: fixture.client_token.map(str::to_string),
            })
        })
        .collect()
}

/// A VPC created over the wire, by id.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let (status, body) = call(router, REGION, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"))
}

/// A carrier gateway created over the wire in `vpc_id`, by id.
async fn create_gateway(router: &MultiServiceRouter, case: &str, vpc_id: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "CreateCarrierGateway",
        &[param("VpcId", vpc_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the create, got {status} with body {body}"
    );
    element(&body, "carrierGatewayId")
        .unwrap_or_else(|| panic!("case {case:?}: the create answered no carrierGatewayId: {body}"))
}

// ---------------------------------------------------------------------------
// The store is what is deleted, and what is reported
// ---------------------------------------------------------------------------

/// Deleting a gateway removes exactly it from the store and answers with that gateway as
/// the describe reported it until this call — every member, tags included — except for the
/// state, which is the one a deletion puts a gateway into. One row per seeded gateway, so
/// the rows differ in the VPC, the stored state, the number of tags and whether a client
/// token minted the gateway.
#[tokio::test]
async fn deleting_a_gateway_removes_exactly_it_and_reports_it_deleting() {
    for (position, fixture) in fixtures().into_iter().enumerate() {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let carrier_gateway_id = seeded[position].clone();
        let case = fixture.case;

        let described = reported_gateway(&describe_one(&router, case, &carrier_gateway_id).await);
        // A seeded gateway is stored in the state its row names and the clock owes it no
        // move: the owed-transition index is written by the CREATE, so seeding through
        // `insert_carrier_gateway` leaves nothing due and this row is not timing-dependent.
        assert_eq!(
            described.state.as_deref(),
            Some(fixture.state),
            "case {case:?}: the fixture must be stored in the state it names, got {described:?}"
        );

        let (status, body) = delete_gateway(&router, &carrier_gateway_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let deleted = reported_gateway(&body);
        let expected = ReportedGateway {
            state: Some(CARRIER_GATEWAY_STATE_DELETING.to_string()),
            ..described
        };
        assert_eq!(
            deleted, expected,
            "case {case:?}: expected the delete to report the stored gateway as {expected:?}, \
             got {deleted:?}"
        );

        let remaining: Vec<String> = seeded
            .iter()
            .filter(|id| **id != carrier_gateway_id)
            .cloned()
            .collect();
        let reported = described_ids(&router, case, &[]).await;
        assert_eq!(
            reported, remaining,
            "case {case:?}: expected the other gateways {remaining:?} to be left alone, got \
             {reported:?}"
        );
        assert_eq!(
            stored_gateways(&engine),
            remaining.len(),
            "case {case:?}: expected {} gateways in the store, got {}",
            remaining.len(),
            stored_gateways(&engine)
        );
    }
}

/// Whatever ORDER the gateways are deleted in, the store is left holding exactly the ones
/// that were not deleted, in store order: a delete removes the gateway it names and nothing
/// else. The rows are distinct orders over the seeded positions, plus one that deletes every
/// gateway.
#[tokio::test]
async fn deleting_in_any_order_leaves_exactly_the_gateways_not_deleted() {
    let orders: [&[usize]; 4] = [&[0, 1, 2], &[4, 3, 2], &[3, 0, 4], &[0, 1, 2, 3, 4]];

    for order in orders {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let case = format!("order {order:?}");

        for position in order {
            let carrier_gateway_id = &seeded[*position];
            let (status, body) = delete_gateway(&router, carrier_gateway_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: deleting {carrier_gateway_id} expected HTTP 200, got {status} \
                 with body {body}"
            );
            let reported = element(&body, "carrierGatewayId").unwrap_or_default();
            assert_eq!(
                &reported, carrier_gateway_id,
                "case {case:?}: expected the response to name {carrier_gateway_id}, got \
                 {reported:?}"
            );
        }

        let expected: Vec<String> = seeded
            .iter()
            .enumerate()
            .filter(|(position, _)| !order.contains(position))
            .map(|(_, id)| id.clone())
            .collect();
        let reported = described_ids(&router, &case, &[]).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the gateways not deleted {expected:?}, got {reported:?}"
        );
        assert_eq!(
            stored_gateways(&engine),
            expected.len(),
            "case {case:?}: expected {} gateways in the store, got {}",
            expected.len(),
            stored_gateways(&engine)
        );
    }
}

// ---------------------------------------------------------------------------
// A CarrierGatewayId that names no gateway
// ---------------------------------------------------------------------------

/// A `CarrierGatewayId` the account holds no gateway for is refused — HTTP 400
/// `InvalidCarrierGatewayID.NotFound`, naming the value the request sent — and nothing is
/// removed. The rows are each way a value can fail to name a gateway of the account: both
/// published EC2 id suffix lengths with digits nothing was created for, a suffix of neither
/// length, an upper-case suffix, a non-hexadecimal suffix, another resource type's id, free
/// text, and a value carrying the characters that are significant in the XML envelope the
/// refusal is written into — which the message names as the caller sent them, so the
/// envelope stays well-formed and the value is not reflected into its markup.
#[tokio::test]
async fn a_carrier_gateway_id_that_names_no_gateway_is_refused() {
    let prefix = CARRIER_GATEWAY_ID_PREFIX;
    let cases = [
        ("the long form", format!("{prefix}-00000000000000000")),
        ("the short form", format!("{prefix}-00000000")),
        ("neither length", format!("{prefix}-0000000000000000")),
        ("an upper-case suffix", format!("{prefix}-0ABCDEF01")),
        ("a non-hex suffix", format!("{prefix}-zzzzzzzz")),
        ("another resource's id", "vpc-00000000000000000".to_string()),
        ("free text", "not-a-carrier-gateway".to_string()),
        (
            "XML metacharacters",
            "cagw-<tag>&'\"end".to_string(),
        ),
    ];

    for (case, carrier_gateway_id) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);

        let (status, body) = delete_gateway(&router, &carrier_gateway_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_CARRIER_GATEWAY_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_CARRIER_GATEWAY_ID_NOT_FOUND}, got {body}"
        );
        // Read the message out of the PARSED envelope rather than by substring: a value
        // whose metacharacters reached the markup verbatim would either stop the body from
        // parsing or cut the element's text short at that character, so this is what pins
        // that a caller-controlled id is written into the envelope escaped.
        let message = refusal_message(case, &body);
        assert_eq!(
            message,
            format!("The CarrierGateway ID '{carrier_gateway_id}' does not exist"),
            "case {case:?}: expected the surface's one sentence, got {message:?}"
        );
        assert_eq!(
            message, error_message(&body),
            "case {case:?}: the parsed message and the one on the wire must agree, got \
             {message:?} and {:?}",
            error_message(&body)
        );
        assert_eq!(
            stored_gateways(&engine),
            seeded.len(),
            "case {case:?}: a refused delete must remove nothing, got {} gateways",
            stored_gateways(&engine)
        );
        let reported = described_ids(&router, case, &[]).await;
        assert_eq!(
            reported, seeded,
            "case {case:?}: expected every gateway to be left alone, got {reported:?}"
        );
    }
}

/// A delete is not idempotent: the gateway is gone after the first call, so a second call
/// naming the same id names no gateway of the account and is refused — the same refusal any
/// other unknown id gets, naming that id.
#[tokio::test]
async fn deleting_the_same_gateway_twice_refuses_the_second_call() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let carrier_gateway_id = seeded[1].clone();

    let (status, body) = delete_gateway(&router, &carrier_gateway_id).await;
    assert_eq!(
        status, 200,
        "the first delete: expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = delete_gateway(&router, &carrier_gateway_id).await;
    assert_eq!(
        status, 400,
        "the second delete: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_CARRIER_GATEWAY_ID_NOT_FOUND,
        "the second delete: expected {INVALID_CARRIER_GATEWAY_ID_NOT_FOUND}, got {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("The CarrierGateway ID '{carrier_gateway_id}' does not exist"),
        "the second delete: expected the message to name {carrier_gateway_id}, got {body}"
    );
    assert_eq!(
        stored_gateways(&engine),
        seeded.len() - 1,
        "the second delete: the refused call must remove nothing, got {} gateways",
        stored_gateways(&engine)
    );
}

/// A request that names no `CarrierGatewayId` at all names no gateway to delete, so it is
/// refused rather than answered with an empty success, and nothing is removed.
#[tokio::test]
async fn a_request_that_names_no_gateway_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = delete(&router, &[]).await;
    assert_eq!(
        status, 400,
        "no CarrierGatewayId: expected HTTP 400, got {status} with body {body}"
    );
    // The member is `@required`, so the BOUNDARY validator refuses the request before this
    // operation's handler runs — with the framework's own `MalformedInput`, not with a code
    // the handler chose.
    assert_eq!(
        error_code(&body),
        "MalformedInput",
        "no CarrierGatewayId: expected the boundary validator's MalformedInput, got {body}"
    );
    assert_eq!(
        stored_gateways(&engine),
        seeded.len(),
        "no CarrierGatewayId: a refused delete must remove nothing, got {} gateways",
        stored_gateways(&engine)
    );
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and deletes
/// nothing, so the gateway is still readable afterwards; `DryRun=false` is an ordinary
/// delete. One row per seeded gateway, so the probe is asserted whatever state the gateway
/// was stored in.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_deleting() {
    for (position, fixture) in fixtures().into_iter().enumerate() {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let carrier_gateway_id = seeded[position].clone();
        let case = fixture.case;

        let (status, body) = delete(
            &router,
            &[
                param("CarrierGatewayId", &carrier_gateway_id),
                param("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION}, got {body}"
        );
        assert_eq!(
            stored_gateways(&engine),
            seeded.len(),
            "case {case:?}: a dry run must delete nothing, got {} gateways",
            stored_gateways(&engine)
        );
        let reported = described_ids(&router, case, &[]).await;
        assert_eq!(
            reported, seeded,
            "case {case:?}: a dry run must leave every gateway readable, got {reported:?}"
        );

        // `DryRun=false` is no dry run at all: the delete happens.
        let (status, body) = delete(
            &router,
            &[
                param("CarrierGatewayId", &carrier_gateway_id),
                param("DryRun", "false"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DryRun=false expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            stored_gateways(&engine),
            seeded.len() - 1,
            "case {case:?}: DryRun=false must delete the gateway, got {} gateways",
            stored_gateways(&engine)
        );
    }
}

// ---------------------------------------------------------------------------
// What the delete releases
// ---------------------------------------------------------------------------

/// A carrier gateway is a resource associated with its VPC, so the VPC is released only once
/// the LAST of its gateways is deleted: "you must detach or delete all gateways and
/// resources that are associated with the VPC before you can delete it" (`DeleteVpc`). The
/// rows differ in how many gateways the VPC holds, so the release is asserted against a
/// count rather than against one gateway. What bounds that count is the CREATE, which this
/// emulator applies no per-VPC bound in; the rule under test here is the release, so a row
/// asks the create for the gateways and asserts only what the deletes then free.
#[tokio::test]
async fn a_vpc_is_released_only_once_its_last_carrier_gateway_is_deleted() {
    for (case, gateway_count) in [("one gateway", 1usize), ("two gateways", 2), ("three", 3)] {
        let (router, engine) = super::ec2_query::emulator();
        let vpc_id = create_vpc(&router, "10.90.0.0/16").await;
        let mut gateways = Vec::new();
        for _ in 0..gateway_count {
            gateways.push(create_gateway(&router, case, &vpc_id).await);
        }

        for (deleted, carrier_gateway_id) in gateways.iter().enumerate() {
            // Every gateway still in the VPC holds it, so the VPC delete is refused until
            // the last one is gone.
            let (status, body) =
                call(&router, REGION, "DeleteVpc", &[param("VpcId", &vpc_id)]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: with {} gateway(s) left the VPC delete must be refused, got \
                 {status} with body {body}",
                gateway_count - deleted
            );
            assert_eq!(
                error_code(&body),
                DEPENDENCY_VIOLATION,
                "case {case:?}: expected {DEPENDENCY_VIOLATION}, got {body}"
            );

            let (status, body) = delete_gateway(&router, carrier_gateway_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: deleting {carrier_gateway_id} expected HTTP 200, got {status} \
                 with body {body}"
            );
        }

        assert_eq!(
            stored_gateways(&engine),
            0,
            "case {case:?}: every gateway of the VPC was deleted, got {} left",
            stored_gateways(&engine)
        );
        let (status, body) = call(&router, REGION, "DeleteVpc", &[param("VpcId", &vpc_id)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the last gateway is gone, so the VPC delete must succeed, got \
             {status} with body {body}"
        );
    }
}
