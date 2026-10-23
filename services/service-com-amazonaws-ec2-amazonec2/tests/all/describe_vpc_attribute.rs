// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeVpcAttribute`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeVpcAttribute`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: for the VPC the request
//! names it answers the stored value of the ONE `VpcAttributeName` attribute asked about,
//! leaves the other two out, echoes the `VpcId`, and writes nothing. Every table row is a
//! distinct input along what that generalises over — which of the three attributes, which
//! stored value, which kind of VPC (the account's default VPC, one a caller created), and
//! which malformed or unknown id — so the one case a recording happens to contain is one
//! row among many rather than the specification.

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, VpcAttributes};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use super::ec2_query::error_code;

/// The Region every call here is signed for. A VPC's attributes are facts of the account's
/// VPC rather than of a Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The three members of the model's `VpcAttributeName` enum, which are both the names a
/// request may ask about and the element names the answer reports them under:
///
/// ```text
/// smithy_query(query="shape",shape="VpcAttributeName",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
const ATTRIBUTE_NAMES: [&str; 3] = [
    "enableDnsSupport",
    "enableDnsHostnames",
    "enableNetworkAddressUsageMetrics",
];

/// The three names the tables below are written against are exactly the members the model
/// declares, so an enum that gains, loses or renames one fails here rather than leaving an
/// attribute silently untested.
#[test]
fn the_tabled_attribute_names_are_the_models_enum_members() {
    let mut tabled = ATTRIBUTE_NAMES.to_vec();
    tabled.sort_unstable();
    let mut modelled = ec2::ops::types::VPC_ATTRIBUTE_NAME_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        tabled, modelled,
        "case \"VpcAttributeName members\": expected the model's {modelled:?}, \
         got {tabled:?}"
    );
}

/// The emulator the tests drive, with the engine whose store a suite seeds.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `DescribeVpcAttribute` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeVpcAttribute", params).await
}

/// The form fields of a `DescribeVpcAttribute` request.
fn request(vpc_id: &str, attribute: &str) -> Vec<(String, String)> {
    vec![
        ("VpcId".to_string(), vpc_id.to_string()),
        ("Attribute".to_string(), attribute.to_string()),
    ]
}

/// The stored value of `attribute` in `attributes`, so a table row can name the attribute
/// by its wire name and still say what the store holds for it.
fn held(attributes: VpcAttributes, attribute: &str) -> bool {
    match attribute {
        "enableDnsSupport" => attributes.enable_dns_support,
        "enableDnsHostnames" => attributes.enable_dns_hostnames,
        "enableNetworkAddressUsageMetrics" => attributes.enable_network_address_usage_metrics,
        other => panic!("{other:?} is not a VpcAttributeName member"),
    }
}

/// The text of the first element named `name` anywhere in a response, read through
/// `roxmltree` rather than by string surgery: an `AttributeBooleanValue` member reaches the
/// wire carrying its own `xmlns`, so `<enableDnsSupport xmlns="…">` is not found by looking
/// for the literal `<enableDnsSupport>`.
fn text(body: &str, name: &str) -> Option<String> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the body must be XML ({error}): {body}"));
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == name)
        .map(|node| node.text().unwrap_or_default().to_string())
}

/// The `<value>` of the `<attribute>` element of a response, or `None` when the response
/// carries no element for that attribute at all.
fn reported(body: &str, attribute: &str) -> Option<bool> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the body must be XML ({error}): {body}"));
    let member = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == attribute)?;
    let value = member
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "value")
        .map(|node| node.text().unwrap_or_default().to_string())
        .unwrap_or_else(|| panic!("<{attribute}> carries no <value> in {body}"));
    Some(
        value
            .parse()
            .unwrap_or_else(|_| panic!("<{attribute}> reported a non-boolean {value:?}")),
    )
}

/// `DescribeVpcAttribute` requiring success, and the body it answered.
async fn describe_ok(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    attribute: &str,
) -> String {
    let (status, body) = describe(router, &request(vpc_id, attribute)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpcAttribute, \
         got {status} with body {body}"
    );
    body
}

/// Every combination of the three booleans, so a table row differs from its neighbours in
/// exactly one stored value and no row's answer can be right by accident.
fn attribute_combinations() -> Vec<VpcAttributes> {
    let mut combinations = Vec::new();
    for support in [false, true] {
        for hostnames in [false, true] {
            for metrics in [false, true] {
                combinations.push(VpcAttributes {
                    enable_dns_support: support,
                    enable_dns_hostnames: hostnames,
                    enable_network_address_usage_metrics: metrics,
                });
            }
        }
    }
    combinations
}

/// Write one VPC per combination into the store, the way a creating operation would, with
/// that combination as its attributes — the rows the reporting tables are written against.
fn seed(engine: &engine::InMemoryEngine) -> Vec<(VpcAttributes, String)> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut seeded = Vec::new();
    for (index, attributes) in attribute_combinations().into_iter().enumerate() {
        let cidr_block = format!("10.{index}.0.0/16");
        let vpc_id = guard.insert_vpc(&cidr_block, false, "default", "available", Vec::new());
        guard.vpc_attributes.insert(vpc_id.clone(), attributes);
        seeded.push((attributes, vpc_id));
    }
    seeded
}

/// The id of the default VPC a fresh account already has: the one VPC of a fresh store.
fn default_vpc_id(engine: &engine::InMemoryEngine) -> String {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .vpcs
        .values()
        .find(|vpc| vpc.is_default == Some(true))
        .and_then(|vpc| vpc.vpc_id.clone())
        .expect("a fresh account has a default VPC")
}

// ---------------------------------------------------------------------------
// Reporting the store
// ---------------------------------------------------------------------------

/// The rule: the answer carries the stored value of the one attribute the request asked
/// about, and no element for either of the other two.
///
/// The rows are every combination of the three stored booleans crossed with every
/// `VpcAttributeName` member, so a handler that reported a constant, the wrong attribute's
/// value, or more than the attribute asked about fails on some row.
#[tokio::test]
async fn describe_vpc_attribute_reports_the_stored_value_of_the_attribute_asked_about() {
    let (router, engine) = emulator();
    for (attributes, vpc_id) in seed(&engine) {
        for attribute in ATTRIBUTE_NAMES {
            let case = format!("{attribute} of {vpc_id} stored as {attributes:?}");
            let body = describe_ok(&case, &router, &vpc_id, attribute).await;

            let actual = reported(&body, attribute);
            assert_eq!(
                actual,
                Some(held(attributes, attribute)),
                "case {case:?}: expected the stored value of {attribute}, \
                 got {actual:?} in {body}"
            );
            assert_eq!(
                text(&body, "vpcId").as_deref(),
                Some(vpc_id.as_str()),
                "case {case:?}: expected the request's VpcId echoed, got {body}"
            );
            for other in ATTRIBUTE_NAMES.iter().filter(|name| **name != attribute) {
                let unexpected = reported(&body, other);
                assert_eq!(
                    unexpected, None,
                    "case {case:?}: only {attribute} was asked about, \
                     but {other} was reported as {unexpected:?} in {body}"
                );
            }
        }
    }
}

/// The rule: a VPC nothing has modified reports the attributes its KIND is created with —
/// DNS support on for every VPC, DNS hostnames on only for the account's default VPC, and
/// Network Address Usage metrics off.
///
/// The rows are the two kinds of VPC: the default VPC a fresh account already has, and one
/// a caller creates with `CreateVpc` over the same wire.
#[tokio::test]
async fn an_unmodified_vpc_reports_the_attributes_its_kind_is_created_with() {
    let (router, engine) = emulator();

    let default_vpc = default_vpc_id(&engine);
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "CreateVpc",
        &[("CidrBlock".to_string(), "10.42.0.0/16".to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateVpc, got {status} with body {body}"
    );
    let created_vpc = text(&body, "vpcId").expect("CreateVpc answers the VPC's id");

    // (case, the VPC, the attributes it has before anything modifies it) — the values the
    // Amazon VPC User Guide's "DNS attributes in your VPC" table publishes, written out
    // here rather than taken from the production constructor so a wrong default in that
    // constructor fails this test.
    let cases = [
        (
            "the account's default VPC",
            default_vpc,
            VpcAttributes {
                enable_dns_support: true,
                enable_dns_hostnames: true,
                enable_network_address_usage_metrics: false,
            },
        ),
        (
            "a VPC CreateVpc created",
            created_vpc,
            VpcAttributes {
                enable_dns_support: true,
                enable_dns_hostnames: false,
                enable_network_address_usage_metrics: false,
            },
        ),
    ];

    for (case, vpc_id, expected) in cases {
        for attribute in ATTRIBUTE_NAMES {
            let row = format!("{case}: {attribute}");
            let body = describe_ok(&row, &router, &vpc_id, attribute).await;
            let actual = reported(&body, attribute);
            assert_eq!(
                actual,
                Some(held(expected, attribute)),
                "case {row:?}: expected the unmodified value of {attribute}, \
                 got {actual:?} in {body}"
            );
        }
    }
}

/// The rule holds for a VPC the store holds no attribute row for at all: it reads as the
/// attributes an unmodified VPC of its kind has, rather than as a refusal or an empty
/// answer — a read never has to write a row to be able to answer.
#[tokio::test]
async fn a_vpc_with_no_stored_attribute_row_reads_as_an_unmodified_vpc() {
    let (router, engine) = emulator();
    let vpc_id = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let vpc_id = guard.insert_vpc("10.7.0.0/16", false, "default", "available", Vec::new());
        guard.vpc_attributes.remove(&vpc_id);
        vpc_id
    };

    // The published values for a VPC a caller created, written out rather than taken from
    // the production constructor (see the table above).
    let expected = VpcAttributes {
        enable_dns_support: true,
        enable_dns_hostnames: false,
        enable_network_address_usage_metrics: false,
    };
    for attribute in ATTRIBUTE_NAMES {
        let case = format!("no stored row: {attribute}");
        let body = describe_ok(&case, &router, &vpc_id, attribute).await;
        let actual = reported(&body, attribute);
        assert_eq!(
            actual,
            Some(held(expected, attribute)),
            "case {case:?}: expected the unmodified value of {attribute}, \
             got {actual:?} in {body}"
        );
    }
}

/// The rule: reporting an attribute changes nothing a later caller sees — the same request
/// repeated answers the same value, and reading one attribute does not disturb another.
#[tokio::test]
async fn describe_vpc_attribute_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();
    let seeded = seed(&engine);
    // Read every attribute of every seeded VPC once...
    for (_, vpc_id) in &seeded {
        for attribute in ATTRIBUTE_NAMES {
            describe_ok("first pass", &router, vpc_id, attribute).await;
        }
    }
    // ...then again: a second pass must report exactly what the store was seeded with.
    for (attributes, vpc_id) in &seeded {
        for attribute in ATTRIBUTE_NAMES {
            let case = format!("second pass: {attribute} of {vpc_id}");
            let body = describe_ok(&case, &router, vpc_id, attribute).await;
            let actual = reported(&body, attribute);
            assert_eq!(
                actual,
                Some(held(*attributes, attribute)),
                "case {case:?}: a read must not change the stored value, \
                 got {actual:?} in {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// The rule: an id that names no VPC of the account is refused with EC2's
/// `InvalidVpcID.NotFound`, whichever attribute was asked about.
///
/// The rows differ in the FORM of the unknown id — a long id, the short 8-hex form, an id
/// of another resource type and a value that is not an id at all — so no row's refusal can
/// come from the id's shape alone.
#[tokio::test]
async fn describe_vpc_attribute_refuses_a_vpc_id_the_account_does_not_hold() {
    let (router, engine) = emulator();
    seed(&engine);
    let unknown_ids = [
        ("a long id no VPC has", "vpc-0123456789abcdef0"),
        ("the short 8-hex form", "vpc-1a2b3c4d"),
        ("another resource type", "subnet-0123456789abcdef0"),
        ("not an id at all", "not-a-vpc"),
    ];

    for (case, vpc_id) in unknown_ids {
        for attribute in ATTRIBUTE_NAMES {
            let row = format!("{case}: {attribute}");
            let (status, body) = describe(&router, &request(vpc_id, attribute)).await;
            assert_eq!(
                status, 400,
                "case {row:?}: expected HTTP 400 for an unknown VPC id, \
                 got {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "InvalidVpcID.NotFound",
                "case {row:?}: expected InvalidVpcID.NotFound, got {code:?} in {body}"
            );
        }
    }
}

/// The rule: a `DryRun` request that would have succeeded is answered with EC2's
/// `DryRunOperation`, whichever attribute it names; it reports no attribute and leaves the
/// stored values alone.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_without_reporting_the_attribute() {
    let (router, engine) = emulator();
    let seeded = seed(&engine);
    let (attributes, vpc_id) = seeded
        .first()
        .cloned()
        .expect("the seed writes at least one VPC");

    for attribute in ATTRIBUTE_NAMES {
        let case = format!("dry run: {attribute}");
        let mut params = request(&vpc_id, attribute);
        params.push(("DryRun".to_string(), "true".to_string()));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 for a dry run, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {code:?} in {body}"
        );
        let unexpected = reported(&body, attribute);
        assert_eq!(
            unexpected, None,
            "case {case:?}: a dry run reports no attribute, got {unexpected:?} in {body}"
        );
    }

    // A dry run performed nothing, so the stored values are untouched.
    for attribute in ATTRIBUTE_NAMES {
        let case = format!("after the dry runs: {attribute}");
        let body = describe_ok(&case, &router, &vpc_id, attribute).await;
        let actual = reported(&body, attribute);
        assert_eq!(
            actual,
            Some(held(attributes, attribute)),
            "case {case:?}: expected the stored value, got {actual:?} in {body}"
        );
    }
}

/// The rule: an `Attribute` outside the model's `VpcAttributeName` enum is a BAD REQUEST,
/// refused with HTTP 400 — never a 501, which would say the operation is unimplemented, and
/// never a 200 reporting some attribute the request did not ask about. The enum constraint
/// the model declares is what refuses it, before the handler runs, so the code is
/// `ValidationError`.
///
/// The rows differ in how the value misses the enum: a near-miss in casing, a name of a
/// SUBNET attribute, and an empty value.
#[tokio::test]
async fn describe_vpc_attribute_refuses_an_attribute_outside_the_enum() {
    let (router, engine) = emulator();
    let seeded = seed(&engine);
    let (_, vpc_id) = seeded
        .first()
        .cloned()
        .expect("the seed writes at least one VPC");
    let cases = [
        ("wrong casing", "EnableDnsSupport"),
        ("another resource's attribute", "enableDns64"),
        ("empty", ""),
    ];

    for (case, attribute) in cases {
        let (status, body) = describe(&router, &request(&vpc_id, attribute)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an attribute outside the enum, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationError",
            "case {case:?}: expected the modelled enum constraint to refuse the value \
             with ValidationError, got {code:?} in {body}"
        );
        assert!(
            code != "NotImplemented",
            "case {case:?}: a bad request is never answered as unimplemented, \
             got {code:?} in {body}"
        );
        for name in ATTRIBUTE_NAMES {
            let unexpected = reported(&body, name);
            assert_eq!(
                unexpected, None,
                "case {case:?}: a refusal reports no attribute, got {unexpected:?} in {body}"
            );
        }
    }
}
