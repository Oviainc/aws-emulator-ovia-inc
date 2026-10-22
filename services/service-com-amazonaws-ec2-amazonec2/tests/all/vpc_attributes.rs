// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyVpcAttribute` and `DescribeVpcAttribute`, driven over the
//! wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with
//! XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that EVERY `VpcAttributeName` attribute of a VPC is an
//! independently stored boolean, which `ModifyVpcAttribute` sets and
//! `DescribeVpcAttribute` reports back for the one attribute a request names, starting at
//! the value AWS gives the VPC when it is created.
//!
//! So the tables below span that class rather than one recorded request: every member of
//! the model's own `VpcAttributeName` catalog, both boolean values of each, both kinds of
//! VPC (the account's default one and one `CreateVpc` made), and sequences of
//! modifications in different orders — plus the refusals the two members' own
//! documentation states ("you cannot modify the DNS resolution and DNS hostnames
//! attributes in the same request", "you can only enable DNS hostnames if you've enabled
//! DNS support").

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, filters};
use super::wire::{member, param, Param};

/// The Region every call here is signed for: the one the default network is seeded for.
const REGION: &str = "us-east-1";

/// `enableDnsSupport`, the attribute the other two are read against.
const DNS_SUPPORT: &str = ec2::state::VPC_ATTRIBUTE_ENABLE_DNS_SUPPORT;

/// `enableDnsHostnames`.
const DNS_HOSTNAMES: &str = ec2::state::VPC_ATTRIBUTE_ENABLE_DNS_HOSTNAMES;

/// `enableNetworkAddressUsageMetrics`.
const NAU_METRICS: &str = ec2::state::VPC_ATTRIBUTE_ENABLE_NETWORK_ADDRESS_USAGE_METRICS;

/// Every attribute a VPC has. Pinned against the model's own `VpcAttributeName` catalog
/// by [`the_attribute_catalog_is_the_models_own`], so a member the model gains cannot be
/// left untested here.
const ATTRIBUTES: [&str; 3] = [DNS_SUPPORT, DNS_HOSTNAMES, NAU_METRICS];

/// The attributes of one VPC as this suite expects them, one field per [`ATTRIBUTES`]
/// entry, so a table row can state the WHOLE expected state after a modification rather
/// than only the attribute it set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Expected {
    dns_support: bool,
    dns_hostnames: bool,
    nau_metrics: bool,
}

impl Expected {
    /// The attributes AWS creates a VPC with: DNS resolution on for either kind of VPC,
    /// DNS hostnames on only for the account's default VPC, Network Address Usage metrics
    /// off until something turns them on (Amazon VPC User Guide, "DNS attributes in your
    /// VPC").
    fn at_creation(is_default: bool) -> Self {
        Self {
            dns_support: true,
            dns_hostnames: is_default,
            nau_metrics: false,
        }
    }

    /// The same values keyed by attribute name, so a test can walk [`ATTRIBUTES`].
    fn value_of(self, attribute: &str) -> bool {
        match attribute {
            DNS_SUPPORT => self.dns_support,
            DNS_HOSTNAMES => self.dns_hostnames,
            NAU_METRICS => self.nau_metrics,
            other => panic!("{other:?} is not an attribute of a VPC"),
        }
    }

    /// These attributes with the one `attribute` names set to `value`.
    fn with(mut self, attribute: &str, value: bool) -> Self {
        match attribute {
            DNS_SUPPORT => self.dns_support = value,
            DNS_HOSTNAMES => self.dns_hostnames = value,
            NAU_METRICS => self.nau_metrics = value,
            other => panic!("{other:?} is not an attribute of a VPC"),
        }
        self
    }
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The `ModifyVpcAttribute` request member that sets the attribute `attribute` names:
/// the same name with its first letter capitalised, which is how the model spells the
/// member (`enableDnsSupport` -> `EnableDnsSupport`).
fn request_member(attribute: &str) -> String {
    let mut characters = attribute.chars();
    match characters.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
        None => String::new(),
    }
}

/// The form fields of a `ModifyVpcAttribute` that sets `attribute` to `value` on `vpc_id`.
fn modify_params(vpc_id: &str, attribute: &str, value: bool) -> Vec<Param> {
    vec![
        param("VpcId", vpc_id),
        param(
            &format!("{}.Value", request_member(attribute)),
            if value { "true" } else { "false" },
        ),
    ]
}

/// One `ModifyVpcAttribute` call, as its HTTP status and XML body.
async fn modify(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "ModifyVpcAttribute", params).await
}

/// One `DescribeVpcAttribute` call, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeVpcAttribute", params).await
}

/// A VPC `CreateVpc` makes, as its `VpcId`.
async fn create_vpc(router: &MultiServiceRouter, case: &str, cidr: &str) -> String {
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateVpc", &[param("CidrBlock", cidr)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVpc, got {status} with body {body}"
    );
    member(case, &body, "vpcId")
}

/// The account's default VPC, as its `VpcId`.
async fn default_vpc(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeVpcs",
        &filters(&[("is-default", &["true"])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpcs, got {status} with body {body}"
    );
    element(&body, "vpcId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a default VPC in {body}"))
}

/// The text of the first element named `name` anywhere in `body`, through `roxmltree` —
/// the same parser [`super::wire`] reads a VPC with. A string search cannot be used here:
/// the serializer writes the nested attribute element with its own `xmlns`
/// (`<enableDnsSupport xmlns="…">`), which `<enableDnsSupport>` does not match.
fn first_element<'a>(document: &'a roxmltree::Document<'_>, name: &str) -> Option<roxmltree::Node<'a, 'a>> {
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == name)
}

/// The value `DescribeVpcAttribute` reports for `attribute`, requiring that the response
/// carries that attribute and ONLY that attribute ("you can specify only one attribute at
/// a time").
async fn read_attribute(
    router: &MultiServiceRouter,
    case: &str,
    vpc_id: &str,
    attribute: &str,
) -> bool {
    let (status, body) = describe(
        router,
        &[param("VpcId", vpc_id), param("Attribute", attribute)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpcAttribute for {attribute}, \
         got {status} with body {body}"
    );
    let document = roxmltree::Document::parse(&body).unwrap_or_else(|error| {
        panic!("case {case:?}: the body must be XML ({error}): {body}")
    });
    let reported_vpc = first_element(&document, "vpcId").and_then(|node| node.text());
    assert_eq!(
        reported_vpc,
        Some(vpc_id),
        "case {case:?}: DescribeVpcAttribute must name the VPC it describes, body {body}"
    );
    for other in ATTRIBUTES.iter().filter(|other| **other != attribute) {
        assert!(
            first_element(&document, other).is_none(),
            "case {case:?}: a DescribeVpcAttribute for {attribute} must not report \
             {other}, body {body}"
        );
    }
    let reported = first_element(&document, attribute).unwrap_or_else(|| {
        panic!("case {case:?}: no <{attribute}> in the DescribeVpcAttribute body {body}")
    });
    let value = reported
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "value")
        .and_then(|node| node.text())
        .unwrap_or_else(|| panic!("case {case:?}: no <value> inside <{attribute}> in {body}"));
    match value {
        "true" => true,
        "false" => false,
        other => panic!("case {case:?}: <{attribute}> carried {other:?}, not a boolean"),
    }
}

/// Every attribute of `vpc_id`, asserted against `expected`.
async fn assert_attributes(
    router: &MultiServiceRouter,
    case: &str,
    vpc_id: &str,
    expected: Expected,
) {
    for attribute in ATTRIBUTES {
        let actual = read_attribute(router, case, vpc_id, attribute).await;
        assert_eq!(
            actual,
            expected.value_of(attribute),
            "case {case:?}: expected {attribute} of {vpc_id} to be {}, got {actual:?}",
            expected.value_of(attribute)
        );
    }
}

/// `ModifyVpcAttribute` setting one attribute, required to succeed.
async fn set_attribute(
    router: &MultiServiceRouter,
    case: &str,
    vpc_id: &str,
    attribute: &str,
    value: bool,
) {
    let params = modify_params(vpc_id, attribute, value);
    let (status, body) = modify(router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ModifyVpcAttribute setting {attribute} to \
         {value}, got {status} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// The catalog of attributes
// ---------------------------------------------------------------------------

/// The attribute names this suite and the handlers use are the model's own
/// `VpcAttributeName` members — no more and no fewer.
#[tokio::test]
async fn the_attribute_catalog_is_the_models_own() {
    let mut used = ATTRIBUTES.to_vec();
    used.sort_unstable();
    let mut modelled = ec2::ops::types::VPC_ATTRIBUTE_NAME_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        used, modelled,
        "case \"VpcAttributeName catalog\": the attributes under test must be the \
         model's own members, got {used:?} against {modelled:?}"
    );
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// The rule: `ModifyVpcAttribute` sets the attribute the request names to the value it
/// carries, `DescribeVpcAttribute` reports that value back, and the VPC's other
/// attributes are untouched.
///
/// The rows are every attribute of a VPC crossed with both boolean values, so each row
/// differs along what the rule generalises over — which attribute, and which value.
#[tokio::test]
async fn modify_vpc_attribute_sets_each_attribute_and_describe_reads_it_back() {
    let router = emulator();
    // (case, the attribute to set, the value to set it to, the CIDR of the VPC it is set
    // on — one fresh VPC per row, so a row starts from the creation defaults)
    let cases = [
        ("DNS support off", DNS_SUPPORT, false, "10.1.0.0/16"),
        ("DNS support on", DNS_SUPPORT, true, "10.2.0.0/16"),
        ("DNS hostnames on", DNS_HOSTNAMES, true, "10.3.0.0/16"),
        ("DNS hostnames off", DNS_HOSTNAMES, false, "10.4.0.0/16"),
        ("NAU metrics on", NAU_METRICS, true, "10.5.0.0/16"),
        ("NAU metrics off", NAU_METRICS, false, "10.6.0.0/16"),
    ];

    for (case, attribute, value, cidr) in cases {
        let vpc_id = create_vpc(&router, case, cidr).await;
        assert_attributes(&router, case, &vpc_id, Expected::at_creation(false)).await;
        set_attribute(&router, case, &vpc_id, attribute, value).await;
        let expected = Expected::at_creation(false).with(attribute, value);
        assert_attributes(&router, case, &vpc_id, expected).await;
    }
}

/// The creation defaults are a property of the VPC, not of the caller: the account's
/// default VPC has DNS hostnames enabled and a VPC `CreateVpc` makes does not, while both
/// have DNS resolution enabled and Network Address Usage metrics disabled.
///
/// The rows are both kinds of VPC crossed with every attribute.
#[tokio::test]
async fn an_unmodified_vpc_reports_the_attributes_it_was_created_with() {
    let router = emulator();
    let created = create_vpc(&router, "a VPC CreateVpc made", "192.168.0.0/24").await;
    let seeded = default_vpc(&router, "the account's default VPC").await;
    // (case, the VPC, whether it is the account's default VPC)
    let cases = [
        ("the account's default VPC", seeded, true),
        ("a VPC CreateVpc made", created, false),
    ];

    for (case, vpc_id, is_default) in cases {
        assert_attributes(&router, case, &vpc_id, Expected::at_creation(is_default)).await;
    }
}

/// The attributes are INDEPENDENT and they PERSIST: a sequence of single-attribute
/// modifications leaves each attribute at the value its own last modification set, and no
/// other.
///
/// The rows are the sequence's steps, applied in order to one VPC; the expectation after
/// each step is computed from the steps so far rather than written out, so the sequence
/// can be reordered or extended without restating it.
#[tokio::test]
async fn each_modification_moves_only_the_attribute_it_names() {
    let router = emulator();
    let vpc_id = create_vpc(&router, "the sequence's VPC", "172.31.0.0/16").await;
    // (case, the attribute to set, the value to set it to)
    let steps = [
        ("NAU metrics first", NAU_METRICS, true),
        ("then DNS hostnames", DNS_HOSTNAMES, true),
        ("then DNS support, unchanged", DNS_SUPPORT, true),
        ("NAU metrics back off", NAU_METRICS, false),
        ("DNS hostnames back off", DNS_HOSTNAMES, false),
        ("DNS support off last", DNS_SUPPORT, false),
        ("NAU metrics on again", NAU_METRICS, true),
    ];

    let mut expected = Expected::at_creation(false);
    for (case, attribute, value) in steps {
        set_attribute(&router, case, &vpc_id, attribute, value).await;
        expected = expected.with(attribute, value);
        assert_attributes(&router, case, &vpc_id, expected).await;
    }
}

// ---------------------------------------------------------------------------
// The documented refusals
// ---------------------------------------------------------------------------

/// The rule: DNS hostnames may be enabled only while DNS resolution is enabled for the
/// VPC ("you can only enable DNS hostnames if you've enabled DNS support"), and a refused
/// modification changes nothing.
///
/// The rows are the two states DNS support can be in when the hostnames request arrives.
#[tokio::test]
async fn dns_hostnames_can_be_enabled_only_while_dns_support_is_enabled() {
    let router = emulator();
    // (case, whether DNS support is enabled first, the HTTP status the hostnames request
    // then answers, the error code it carries)
    let cases = [
        ("DNS support disabled", false, 400, ec2::state::INVALID_PARAMETER_COMBINATION),
        ("DNS support enabled", true, 200, ""),
    ];

    for (index, (case, support, status_expected, code_expected)) in cases.iter().enumerate() {
        let cidr = format!("10.2{index}.0.0/16");
        let vpc_id = create_vpc(&router, case, &cidr).await;
        set_attribute(&router, case, &vpc_id, DNS_SUPPORT, *support).await;

        let params = modify_params(&vpc_id, DNS_HOSTNAMES, true);
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            status, *status_expected,
            "case {case:?}: expected HTTP {status_expected} enabling DNS hostnames, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, *code_expected,
            "case {case:?}: expected error code {code_expected:?} enabling DNS \
             hostnames, got {code:?} with body {body}"
        );
        // Refused or applied, the VPC's attributes are exactly what the outcome implies:
        // a refusal leaves them as they were.
        let expected = Expected::at_creation(false)
            .with(DNS_SUPPORT, *support)
            .with(DNS_HOSTNAMES, *support);
        assert_attributes(&router, case, &vpc_id, expected).await;
    }
}

/// Every refusal these two operations make, each with the EC2 error code and HTTP status
/// it answers.
///
/// The rows differ along what makes the request bad: the two DNS attributes in one
/// request ("use separate requests for each attribute"), a request that names no
/// attribute at all, a VPC id no VPC of the account has — for the write and for the read
/// — and a dry run.
#[tokio::test]
async fn a_bad_request_is_refused_with_the_ec2_error_code_for_it() {
    let router = emulator();
    let vpc_id = create_vpc(&router, "the refusal table's VPC", "10.99.0.0/16").await;
    /// A VPC id of the right shape that no VPC of the account has.
    const ABSENT_VPC: &str = "vpc-0000000000000beef";
    // (case, the action, its form fields, the HTTP status, the EC2 error code)
    let cases: [(&str, &str, Vec<Param>, u16, &str); 5] = [
        (
            "both DNS attributes in one request",
            "ModifyVpcAttribute",
            vec![
                param("VpcId", &vpc_id),
                param("EnableDnsSupport.Value", "true"),
                param("EnableDnsHostnames.Value", "true"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "no attribute named at all",
            "ModifyVpcAttribute",
            vec![param("VpcId", &vpc_id)],
            400,
            ec2::state::MISSING_PARAMETER,
        ),
        (
            "a modification of a VPC that does not exist",
            "ModifyVpcAttribute",
            modify_params(ABSENT_VPC, DNS_SUPPORT, true),
            400,
            ec2::state::INVALID_VPC_ID_NOT_FOUND,
        ),
        (
            "a describe of a VPC that does not exist",
            "DescribeVpcAttribute",
            vec![param("VpcId", ABSENT_VPC), param("Attribute", DNS_SUPPORT)],
            400,
            ec2::state::INVALID_VPC_ID_NOT_FOUND,
        ),
        (
            "a dry run of a describe that would have succeeded",
            "DescribeVpcAttribute",
            vec![
                param("VpcId", &vpc_id),
                param("Attribute", DNS_SUPPORT),
                param("DryRun", "true"),
            ],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
    ];

    for (case, action, params, status_expected, code_expected) in cases {
        let (status, body) = super::ec2_query::call(&router, REGION, action, &params).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected} from {action}, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?} from {action}, got \
             {code:?} with body {body}"
        );
    }

    // None of those refusals wrote anything: the VPC they named still reports the
    // attributes it was created with.
    assert_attributes(
        &router,
        "after every refusal",
        &vpc_id,
        Expected::at_creation(false),
    )
    .await;
}
