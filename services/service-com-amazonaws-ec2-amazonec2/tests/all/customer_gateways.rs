// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the customer gateway slice — `CreateCustomerGateway` and the
//! `DescribeCustomerGateways` it reads back through — driven over the wire the service
//! actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope,
//! so every call here carries one.
//!
//! The rules under test:
//!
//! 1. A describe REPORTS the customer gateways the account holds, each as the store holds it,
//!    narrowed by the ids the request names and by every documented filter joined with `AND`,
//!    in `CustomerGatewayId` order; a `CustomerGatewayId` the account holds no gateway for is
//!    refused with `InvalidCustomerGatewayID.NotFound` and nothing is reported, a filter name
//!    the operation does not document with `InvalidParameterValue`, and `DryRun` reports the
//!    permission outcome and reads nothing.
//! 2. A CREATE STORES THE GATEWAY THE REQUEST DESCRIBES — its type, outside address, BGP ASN,
//!    device name, certificate and tags — under a server-minted id, in service and owed no
//!    move, and every member of it reads back through the describe; a value outside what the
//!    operation publishes is refused rather than defaulted or stored, and an identical request
//!    answers the gateway it already made.
//! 3. A DELETE TAKES THE GATEWAY AN ID NAMES OUT OF SERVICE: it keeps being reported, in the
//!    terminal state of a gateway that is gone, whatever state it was in; an id the account
//!    holds no gateway for is refused as the describe refuses one, and a gateway a VPN
//!    connection still uses is refused with `DependencyViolation`. The gateway the id names is
//!    the ONLY thing that moves: every other member of it, every other gateway of the account
//!    and every other resource that names one are left exactly as they were.
//!
//! Each table row is a distinct input along what those rules generalise over: which member a
//! gateway carries a value for, which documented filter selects on it, how many values and
//! how many filters one request combines, which form the id takes, how many ids are named,
//! which state a gateway is in, and each published ASN bound and one past it. The recorded
//! case the issue was filed about — a well-formed long `cgw-` id nothing holds — is ONE row of
//! [`an_id_the_account_holds_no_gateway_for_is_refused_and_reports_nothing`].
//!
//! The reporting rules are asserted over gateways seeded through `crate::state`'s one write
//! path as well as over created ones, because a create answers a gateway in service and the
//! describe has to report the other three states the same way.

use std::collections::BTreeMap;
use std::time::Duration;

use ec2::ops::types::{CustomerGateway, Tag, VpnConnection};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, CUSTOMER_GATEWAY_ID_PREFIX, DEPENDENCY_VIOLATION,
    DRY_RUN_OPERATION, GATEWAY_TYPE_IPSEC_1, INVALID_CUSTOMER_GATEWAY_ID_NOT_FOUND,
    INVALID_PARAMETER_VALUE, MAX_CUSTOMER_GATEWAY_DEVICE_NAME_CHARS, MAX_FILTERS,
    MAX_FILTER_VALUES, MAX_FILTER_VALUE_BYTES, MAX_RESOURCE_IDS, MISSING_PARAMETER,
    RESOURCE_TYPE_CUSTOMER_GATEWAY, TRANSITION_DELAY, VPN_STATE_AVAILABLE, VPN_STATE_DELETED,
    VPN_STATE_DELETING, VPN_STATE_PENDING,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message, filters, indexed};
use super::wire::{child_text, document, is_long_id, param, set_items, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The operation under test, as ec2Query names the action.
const DESCRIBE: &str = "DescribeCustomerGateways";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `DescribeCustomerGateways` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, DESCRIBE, params).await
}

/// `DescribeCustomerGateways` requiring success, and the body it answered.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {DESCRIBE}, got {status} with body {body}"
    );
    body
}

/// A customer gateway as a create will report one: the `ipsec.1` type every customer gateway
/// has ("Currently, the only supported type is `ipsec.1`",
/// `DescribeCustomerGatewaysRequest$Filters`), in `state`, with no member set that the row
/// seeding it does not set.
fn gateway(state: &str) -> CustomerGateway {
    CustomerGateway {
        state: Some(state.to_string()),
        r#type: Some(GATEWAY_TYPE_IPSEC_1.to_string()),
        ..Default::default()
    }
}

/// Seed `view` in the store and answer the `CustomerGatewayId` it was minted with.
///
/// A create answers a gateway in service, so the other states the describe must report are
/// reached by writing the store directly, as the VPN connection suite does for a connection.
fn seed(engine: &engine::InMemoryEngine, view: CustomerGateway) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_customer_gateway(view)
        .expect("the store holds the seeded customer gateway")
}

/// How many gateways the store holds right now — what a refusal must have left untouched.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().customer_gateways.len()
}

/// One reported gateway: its scalar members by the element name ec2Query writes them under,
/// and the tags of its `tagSet` in the order the response reported them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Reported {
    /// Every scalar member the response carried for this gateway.
    members: BTreeMap<String, String>,
    /// `(key, value)` of each tag, in reported order.
    tags: Vec<(String, String)>,
}

impl Reported {
    /// The gateway's `customerGatewayId`, or the empty string when it reported none.
    fn id(&self) -> &str {
        self.members
            .get("customerGatewayId")
            .map_or("", String::as_str)
    }
}

/// The element the describe lists the gateways under
/// (`DescribeCustomerGatewaysResult$CustomerGateways`'s `customerGatewaySet`).
const GATEWAY_SET: &str = "customerGatewaySet";

/// The element a create answers the gateway it made under
/// (`CreateCustomerGatewayResult$CustomerGateway`'s `customerGateway`).
const CREATED_GATEWAY: &str = "customerGateway";

/// One gateway element as this suite compares them: every scalar member it carried, and the
/// tags of its `tagSet`.
fn read_gateway(element: roxmltree::Node<'_, '_>) -> Reported {
    let mut gateway = Reported::default();
    for member in element.children().filter(roxmltree::Node::is_element) {
        if member.tag_name().name() == "tagSet" {
            gateway.tags = set_items(element, "tagSet")
                .into_iter()
                .map(|tag| {
                    (
                        child_text(tag, "key").unwrap_or_default(),
                        child_text(tag, "value").unwrap_or_default(),
                    )
                })
                .collect();
            continue;
        }
        gateway.members.insert(
            member.tag_name().name().to_string(),
            member.text().unwrap_or_default().to_string(),
        );
    }
    gateway
}

/// Every gateway a `DescribeCustomerGateways` body reports, in the order it reported them.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    let Some(set) = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == GATEWAY_SET)
    else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(read_gateway)
        .collect()
}

/// The one gateway a `CreateCustomerGateway` body answers, or `None` when it answered none —
/// a create reports its gateway under its own element rather than in a set.
fn created(case: &str, body: &str) -> Option<Reported> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == CREATED_GATEWAY)
        .map(read_gateway)
}

/// The `customerGatewayId` of every gateway a body reports, in reported order.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    reported(case, body)
        .iter()
        .map(|gateway| gateway.id().to_string())
        .collect()
}

/// One row of the filter table: what makes it a different input, the `Filters` entries the
/// request carries, and the `CustomerGatewayId`s the report must then hold.
type FilterCase<'a> = (&'a str, Vec<(&'a str, &'a [&'a str])>, Vec<String>);

/// `CustomerGatewayId.N` form fields for the ids a request names.
fn id_params(ids: &[&str]) -> Vec<Param> {
    indexed("CustomerGatewayId", ids)
}

/// Rule 1: every stored gateway is reported, with every member the store holds for it and
/// nothing else, under a server-minted id.
///
/// The rows differ along WHICH members a gateway carries a value for — the device-and-tags
/// form, the certificate form, the extended-ASN form with an IPv6 outside address — so the
/// report is pinned for a gateway of each shape rather than for one example.
#[tokio::test]
async fn every_stored_gateway_is_reported_with_the_members_the_store_holds() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "three gateways of different member shapes";

    // Row 1: an IPv4 device with a 16-bit private ASN, a device name and two tags.
    let mut first = gateway(VPN_STATE_AVAILABLE);
    first.ip_address = Some("203.0.113.12".to_string());
    first.bgp_asn = Some("65000".to_string());
    first.device_name = Some("branch-office-router".to_string());
    first.tags = Some(vec![
        Tag {
            key: Some("Name".to_string()),
            value: Some("branch-office".to_string()),
        },
        Tag {
            key: Some("Owner".to_string()),
            value: Some("TeamA".to_string()),
        },
    ]);
    // Row 2: a device identified by a certificate, with the lowest ASN `BgpAsn` publishes.
    let mut second = gateway(VPN_STATE_AVAILABLE);
    second.ip_address = Some("198.51.100.7".to_string());
    second.bgp_asn = Some("1".to_string());
    second.certificate_arn = Some(
        "arn:aws:acm:us-east-1:123456789012:certificate/12345678-1234-1234-1234-123456789012"
            .to_string(),
    );
    // Row 3: an IPv6 outside address with an ASN above what `BgpAsn` carries, so the gateway
    // reports it in `BgpAsnExtended` instead ("Valid values: 2,147,483,648 to 4,294,967,295",
    // `CustomerGateway$BgpAsnExtended`).
    let mut third = gateway(VPN_STATE_AVAILABLE);
    third.ip_address = Some("2001:db8::1".to_string());
    third.bgp_asn_extended = Some("4200000000".to_string());

    let first_id = seed(&engine, first);
    let second_id = seed(&engine, second);
    let third_id = seed(&engine, third);

    let body = describe_ok(case, &router, &[]).await;
    let reported = reported(case, &body);

    let expected: Vec<Reported> = vec![
        Reported {
            members: BTreeMap::from([
                ("customerGatewayId".to_string(), first_id.clone()),
                ("state".to_string(), VPN_STATE_AVAILABLE.to_string()),
                ("type".to_string(), GATEWAY_TYPE_IPSEC_1.to_string()),
                ("ipAddress".to_string(), "203.0.113.12".to_string()),
                ("bgpAsn".to_string(), "65000".to_string()),
                ("deviceName".to_string(), "branch-office-router".to_string()),
            ]),
            tags: vec![
                ("Name".to_string(), "branch-office".to_string()),
                ("Owner".to_string(), "TeamA".to_string()),
            ],
        },
        Reported {
            members: BTreeMap::from([
                ("customerGatewayId".to_string(), second_id.clone()),
                ("state".to_string(), VPN_STATE_AVAILABLE.to_string()),
                ("type".to_string(), GATEWAY_TYPE_IPSEC_1.to_string()),
                ("ipAddress".to_string(), "198.51.100.7".to_string()),
                ("bgpAsn".to_string(), "1".to_string()),
                (
                    "certificateArn".to_string(),
                    "arn:aws:acm:us-east-1:123456789012:certificate/\
                     12345678-1234-1234-1234-123456789012"
                        .to_string(),
                ),
            ]),
            tags: Vec::new(),
        },
        Reported {
            members: BTreeMap::from([
                ("customerGatewayId".to_string(), third_id.clone()),
                ("state".to_string(), VPN_STATE_AVAILABLE.to_string()),
                ("type".to_string(), GATEWAY_TYPE_IPSEC_1.to_string()),
                ("ipAddress".to_string(), "2001:db8::1".to_string()),
                ("bgpAsnExtended".to_string(), "4200000000".to_string()),
            ]),
            tags: Vec::new(),
        },
    ];
    assert_eq!(
        reported, expected,
        "case {case:?}: expected every stored gateway reported as the store holds it, got \
         {reported:?} in {body}"
    );

    // The id is the service's to mint: the long EC2 resource id form under the customer
    // gateway's own prefix, and a distinct one per gateway.
    for id in [&first_id, &second_id, &third_id] {
        assert!(
            is_long_id(CUSTOMER_GATEWAY_ID_PREFIX, id),
            "case {case:?}: expected a long {CUSTOMER_GATEWAY_ID_PREFIX}- id, got {id:?}"
        );
    }
    // The store is keyed by that id, so the report comes back in id order whatever order the
    // gateways were written in.
    let ids = reported_ids(case, &body);
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        ids, sorted,
        "case {case:?}: expected the report in CustomerGatewayId order, got {ids:?}"
    );

    // A read changes nothing a later caller sees: the same request answers the same report.
    let again = describe_ok(case, &router, &[]).await;
    assert_eq!(
        reported_ids(case, &again),
        ids,
        "case {case:?}: a second read must report the same gateways, got {again}"
    );
}

/// Rule 2: `CustomerGatewayIds` narrows the report to the ids named, and naming none reports
/// all of them.
///
/// The rows differ along how many ids are named and whether one is repeated.
#[tokio::test]
async fn the_id_selector_narrows_the_report() {
    let (router, engine) = super::ec2_query::emulator();
    let first = seed(&engine, gateway(VPN_STATE_AVAILABLE));
    let second = seed(&engine, gateway(VPN_STATE_AVAILABLE));
    let third = seed(&engine, gateway(VPN_STATE_AVAILABLE));

    let cases: Vec<(&str, Vec<&str>, Vec<String>)> = vec![
        (
            "no id at all reports every gateway",
            Vec::new(),
            vec![first.clone(), second.clone(), third.clone()],
        ),
        ("one id reports that gateway", vec![&second], vec![second.clone()]),
        (
            "two ids report both, in id order",
            vec![&third, &first],
            vec![first.clone(), third.clone()],
        ),
        (
            "the same id twice reports it once",
            vec![&first, &first],
            vec![first.clone()],
        ),
    ];

    for (case, ids, expected) in cases {
        let body = describe_ok(case, &router, &id_params(&ids)).await;
        let got = reported_ids(case, &body);
        assert_eq!(
            got, expected,
            "case {case:?}: expected the gateways {expected:?}, got {got:?} in {body}"
        );
    }
}

/// Rule 3: a `CustomerGatewayId` the account holds no gateway for is refused with
/// `InvalidCustomerGatewayID.NotFound`, whatever the store holds and whatever else the
/// request names, and the report carries nothing.
///
/// The rows differ along the form the unresolvable id takes: the long id form (the recorded
/// case), the short one, the all-zero placeholder, another resource kind's id, and a value
/// that is no id at all — plus one row naming it beside an id the account DOES hold, where
/// the refusal still wins.
#[tokio::test]
async fn an_id_the_account_holds_no_gateway_for_is_refused_and_reports_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let held = seed(&engine, gateway(VPN_STATE_AVAILABLE));
    let before = stored_count(&engine);

    let cases: Vec<(&str, Vec<&str>, &str)> = vec![
        (
            "a long-form id nothing holds",
            vec!["cgw-00000000000000002"],
            "cgw-00000000000000002",
        ),
        ("a short-form id nothing holds", vec!["cgw-00000000"], "cgw-00000000"),
        (
            "the all-zero placeholder id",
            vec!["cgw-00000000000000000"],
            "cgw-00000000000000000",
        ),
        (
            "another resource kind's id",
            vec!["vgw-0123456789abcdef0"],
            "vgw-0123456789abcdef0",
        ),
        ("a value that is no id at all", vec!["not-a-gateway"], "not-a-gateway"),
        (
            "an unknown id beside one the account holds",
            vec![&held, "cgw-00000000000000003"],
            "cgw-00000000000000003",
        ),
    ];

    for (case, ids, unknown) in cases {
        let (status, body) = describe(&router, &id_params(&ids)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_CUSTOMER_GATEWAY_ID_NOT_FOUND,
            "case {case:?}: expected the code {INVALID_CUSTOMER_GATEWAY_ID_NOT_FOUND}, got \
             {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The customerGateway ID '{unknown}' does not exist"),
            "case {case:?}: expected the refusal to name {unknown:?}, got {body}"
        );
        assert!(
            reported(case, &body).is_empty(),
            "case {case:?}: a refusal reports no gateway, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            before,
            "case {case:?}: a refusal writes nothing to the store"
        );
    }
}

/// Rule 4: each documented filter selects on the attribute it names, several values of one
/// filter are joined with `OR` and several filters with `AND`; a filter whose attribute a
/// gateway does not carry selects that gateway out.
///
/// The rows cover every name `DescribeCustomerGatewaysRequest$Filters` documents — including
/// `bgp-asn` against a gateway whose ASN is reported in `BgpAsnExtended` — and the issue
/// cited none of them.
#[tokio::test]
async fn each_documented_filter_selects_on_its_own_attribute() {
    let (router, engine) = super::ec2_query::emulator();

    let mut ipv4 = gateway(VPN_STATE_AVAILABLE);
    ipv4.ip_address = Some("203.0.113.12".to_string());
    ipv4.bgp_asn = Some("65000".to_string());
    ipv4.tags = Some(vec![Tag {
        key: Some("Name".to_string()),
        value: Some("branch-office".to_string()),
    }]);
    let mut extended = gateway(VPN_STATE_PENDING);
    extended.ip_address = Some("198.51.100.7".to_string());
    extended.bgp_asn_extended = Some("4200000000".to_string());
    extended.tags = Some(vec![Tag {
        key: Some("Stage".to_string()),
        value: Some("test".to_string()),
    }]);

    let ipv4_id = seed(&engine, ipv4);
    let extended_id = seed(&engine, extended);
    // The id filter's value, bound so the table below can borrow it.
    let extended_id_value = [extended_id.as_str()];

    // The pending gateway's clock-owed move is left to its own test; here the filter on
    // `state` is asserted before it falls due.
    let cases: Vec<FilterCase> = vec![
        (
            "bgp-asn against the member BgpAsn carries",
            vec![("bgp-asn", &["65000"])],
            vec![ipv4_id.clone()],
        ),
        (
            "bgp-asn against the member BgpAsnExtended carries",
            vec![("bgp-asn", &["4200000000"])],
            vec![extended_id.clone()],
        ),
        (
            "customer-gateway-id",
            vec![("customer-gateway-id", &extended_id_value)],
            vec![extended_id.clone()],
        ),
        (
            "ip-address",
            vec![("ip-address", &["203.0.113.12"])],
            vec![ipv4_id.clone()],
        ),
        (
            "state",
            vec![("state", &[VPN_STATE_PENDING])],
            vec![extended_id.clone()],
        ),
        (
            "type, which both gateways share",
            vec![("type", &[GATEWAY_TYPE_IPSEC_1])],
            vec![ipv4_id.clone(), extended_id.clone()],
        ),
        (
            "tag:Name on the one gateway carrying that key",
            vec![("tag:Name", &["branch-office"])],
            vec![ipv4_id.clone()],
        ),
        (
            "tag-key, whatever the value",
            vec![("tag-key", &["Stage"])],
            vec![extended_id.clone()],
        ),
        (
            "two values of one filter are joined with OR",
            vec![("ip-address", &["203.0.113.12", "198.51.100.7"])],
            vec![ipv4_id.clone(), extended_id.clone()],
        ),
        (
            "two filters are joined with AND",
            vec![
                ("type", &[GATEWAY_TYPE_IPSEC_1]),
                ("state", &[VPN_STATE_AVAILABLE]),
            ],
            vec![ipv4_id.clone()],
        ),
        (
            "two filters no one gateway satisfies together",
            vec![
                ("state", &[VPN_STATE_AVAILABLE]),
                ("bgp-asn", &["4200000000"]),
            ],
            Vec::new(),
        ),
        (
            "a value no gateway holds",
            vec![("ip-address", &["192.0.2.1"])],
            Vec::new(),
        ),
        (
            "a filter on an attribute neither gateway carries",
            vec![("tag:Owner", &["TeamA"])],
            Vec::new(),
        ),
    ];

    for (case, entries, expected) in cases {
        let body = describe_ok(case, &router, &filters(&entries)).await;
        let got = reported_ids(case, &body);
        assert_eq!(
            got, expected,
            "case {case:?}: expected the gateways {expected:?}, got {got:?} in {body}"
        );
    }
}

/// Rule 4, refusals: a filter name this operation does not document is refused rather than
/// read as "no results".
///
/// The rows differ along how the name misses: a name of another describe's vocabulary, a
/// misspelling, a case difference ("Filter names are case-sensitive" — `Filter$Name`), and a
/// `tag:` prefix with no key.
#[tokio::test]
async fn a_filter_name_the_operation_does_not_document_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine, gateway(VPN_STATE_AVAILABLE));

    for (case, name) in [
        ("another describe's filter", "vpn-gateway-id"),
        ("a misspelling", "bgp-asn-number"),
        ("a case difference", "State"),
        ("a tag prefix with no key", "tag:"),
        ("the empty name", ""),
    ] {
        let (status, body) = describe(&router, &filters(&[(name, &[] as &[&str])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for the filter {name:?}, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected the code {INVALID_PARAMETER_VALUE}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The filter '{name}' is invalid"),
            "case {case:?}: expected the refusal to name the filter {name:?}, got {body}"
        );
    }
}

/// Rule 4, bounds: a request asking for more filtering than one call will do is refused
/// before any of it runs, at each ceiling this crate publishes.
#[tokio::test]
async fn a_request_past_a_published_ceiling_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine, gateway(VPN_STATE_AVAILABLE));

    // One filter past MAX_FILTERS, all of them documented, so the count is what is refused.
    let too_many_filters: Vec<Param> = (1..=MAX_FILTERS + 1)
        .flat_map(|index| {
            vec![
                param(&format!("Filter.{index}.Name"), "state"),
                param(&format!("Filter.{index}.Value.1"), VPN_STATE_AVAILABLE),
            ]
        })
        .collect();
    // One value past MAX_FILTER_VALUES on one documented filter.
    let too_many_values: Vec<Param> = std::iter::once(param("Filter.1.Name", "state"))
        .chain((1..=MAX_FILTER_VALUES + 1).map(|index| {
            param(&format!("Filter.1.Value.{index}"), VPN_STATE_AVAILABLE)
        }))
        .collect();
    // One byte past MAX_FILTER_VALUE_BYTES in a single value.
    let oversize_value = vec![
        param("Filter.1.Name", "state"),
        param("Filter.1.Value.1", &"a".repeat(MAX_FILTER_VALUE_BYTES + 1)),
    ];
    // One id past MAX_RESOURCE_IDS, refused on the count before any id is resolved.
    let too_many_ids: Vec<Param> = (1..=MAX_RESOURCE_IDS + 1)
        .map(|index| param(&format!("CustomerGatewayId.{index}"), "cgw-00000000000000002"))
        .collect();

    for (case, params, expected) in [
        (
            "one filter past the ceiling",
            too_many_filters,
            format!(
                "The request carries {supplied} filters, above the maximum of {MAX_FILTERS}",
                supplied = MAX_FILTERS + 1
            ),
        ),
        (
            "one value past the ceiling",
            too_many_values,
            format!(
                "The filter 'state' carries {supplied} values, above the maximum of \
                 {MAX_FILTER_VALUES}",
                supplied = MAX_FILTER_VALUES + 1
            ),
        ),
        (
            "one byte past the value ceiling",
            oversize_value,
            format!(
                "A value of the filter 'state' carries {supplied} bytes, above the maximum \
                 of {MAX_FILTER_VALUE_BYTES}",
                supplied = MAX_FILTER_VALUE_BYTES + 1
            ),
        ),
        (
            "one id past the ceiling",
            too_many_ids,
            format!(
                "CustomerGatewayIds carries {supplied} ids, above the maximum of \
                 {MAX_RESOURCE_IDS}",
                supplied = MAX_RESOURCE_IDS + 1
            ),
        ),
    ] {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected the code {INVALID_PARAMETER_VALUE}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected the refusal {expected:?}, got {body}"
        );
    }
}

/// Rule 5: `DryRun` answers the permission outcome and reads nothing — whatever else the
/// request would have selected, and even when it names an id nothing holds, which the
/// request's own form is checked for first.
#[tokio::test]
async fn a_dry_run_reports_the_permission_and_reports_no_gateway() {
    let (router, engine) = super::ec2_query::emulator();
    let held = seed(&engine, gateway(VPN_STATE_AVAILABLE));

    for (case, mut params) in [
        ("a dry run of the unfiltered describe", Vec::new()),
        ("a dry run naming a gateway the account holds", id_params(&[&held])),
        (
            "a dry run naming an id nothing holds",
            id_params(&["cgw-00000000000000002"]),
        ),
        (
            "a dry run carrying a documented filter",
            filters(&[("state", &[VPN_STATE_AVAILABLE])]),
        ),
    ] {
        params.push(param("DryRun", "true"));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected the code {DRY_RUN_OPERATION}, got {body}"
        );
        assert!(
            reported(case, &body).is_empty(),
            "case {case:?}: a dry run reports no gateway, got {body}"
        );
    }
}

/// Rule 6: a gateway stored in a non-terminal state is reported in it until the move the
/// clock owes falls due, and in its terminal state afterwards — the move happening on the
/// clock rather than at the next call.
///
/// The rows differ along which non-terminal state the gateway is in, and so along which
/// terminal state is owed; a gateway stored in a terminal state is owed nothing and stays as
/// it is.
#[tokio::test]
async fn a_gateway_in_a_non_terminal_state_reaches_its_terminal_one_on_the_clock() {
    let (router, engine) = super::ec2_query::emulator();

    let cases: Vec<(&str, &str, &str)> = vec![
        ("a gateway coming up", VPN_STATE_PENDING, VPN_STATE_AVAILABLE),
        ("a gateway going away", VPN_STATE_DELETING, VPN_STATE_DELETED),
        (
            "a gateway already in service",
            VPN_STATE_AVAILABLE,
            VPN_STATE_AVAILABLE,
        ),
        ("a gateway already gone", VPN_STATE_DELETED, VPN_STATE_DELETED),
    ];

    for (case, stored, owed) in cases {
        let id = seed(&engine, gateway(stored));
        let params = id_params(&[&id]);

        // Straight after the write the gateway is still in the state it was stored in, so a
        // caller acting at once sees the transient state rather than the one owed.
        let body = describe_ok(case, &router, &params).await;
        let before = reported(case, &body);
        assert_eq!(
            before.len(),
            1,
            "case {case:?}: expected exactly the named gateway, got {body}"
        );
        assert_eq!(
            before[0].members.get("state").map(String::as_str),
            Some(stored),
            "case {case:?}: expected the stored state {stored:?}, got {body}"
        );

        tokio::time::sleep(past_the_transition()).await;
        let body = describe_ok(case, &router, &params).await;
        let after = reported(case, &body);
        assert_eq!(
            after[0].members.get("state").map(String::as_str),
            Some(owed),
            "case {case:?}: expected the state {owed:?} once the move fell due, got {body}"
        );
        // The move is applied ONCE: a further read reports the same terminal state.
        let body = describe_ok(case, &router, &params).await;
        assert_eq!(
            reported(case, &body)[0].members.get("state").map(String::as_str),
            Some(owed),
            "case {case:?}: expected the terminal state to hold, got {body}"
        );
    }
}

/// Rule 7: a filter documented as a fact of the CUSTOMER GATEWAY is answered out of the
/// gateway store — `DescribeVpnConnections`'s `bgp-asn` is "the BGP Autonomous System Number
/// (ASN) associated with a BGP device", which the modelled `VpnConnection` carries no member
/// for, so the connection's `CustomerGatewayId` is resolved and the ASN that gateway holds is
/// what the filter compares against.
///
/// The rows differ along which member the gateway reports its ASN in, whether the asked-for
/// ASN is the one it holds, and whether the connection names a gateway the account holds at
/// all. `CreateVpnConnection` is a scaffold stub under its own issue, so the connections are
/// seeded through `crate::state`'s one write path, as its own suite does.
#[tokio::test]
async fn the_bgp_asn_filter_of_a_vpn_connection_resolves_its_customer_gateway() {
    let (router, engine) = super::ec2_query::emulator();

    let mut plain = gateway(VPN_STATE_AVAILABLE);
    plain.bgp_asn = Some("65000".to_string());
    let plain_id = seed(&engine, plain);
    let mut extended = gateway(VPN_STATE_AVAILABLE);
    extended.bgp_asn_extended = Some("4200000000".to_string());
    let extended_id = seed(&engine, extended);

    // One connection per gateway, plus one naming a gateway the account does not hold.
    let on_plain = seed_connection(&engine, &plain_id);
    let on_extended = seed_connection(&engine, &extended_id);
    seed_connection(&engine, "cgw-00000000000000002");

    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "the ASN the gateway reports in BgpAsn",
            "65000",
            vec![on_plain.clone()],
        ),
        (
            "the ASN the gateway reports in BgpAsnExtended",
            "4200000000",
            vec![on_extended.clone()],
        ),
        ("an ASN no gateway of the account holds", "64512", Vec::new()),
    ];

    for (case, asn, expected) in cases {
        let body = describe_ok_action(
            case,
            &router,
            "DescribeVpnConnections",
            &filters(&[("bgp-asn", &[asn])]),
        )
        .await;
        let parsed = document(case, &body);
        let got: Vec<String> = parsed
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "vpnConnectionId")
            .map(|node| node.text().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected the connections {expected:?}, got {got:?} in {body}"
        );
    }
}

/// Seed one `ipsec.1` VPN connection naming `customer_gateway_id`, and answer the
/// `VpnConnectionId` it was minted with. `CreateVpnConnection` is a scaffold stub under its
/// own issue, so the connection goes through `crate::state`'s one write path for one, as its
/// own suite's seeding does.
fn seed_connection(engine: &engine::InMemoryEngine, customer_gateway_id: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_vpn_connection(VpnConnection {
            customer_gateway_id: Some(customer_gateway_id.to_string()),
            r#type: Some(GATEWAY_TYPE_IPSEC_1.to_string()),
            state: Some(VPN_STATE_AVAILABLE.to_string()),
            ..Default::default()
        })
        .expect("the store holds the seeded VPN connection")
}

/// One `action` call requiring success, and the body it answered — [`describe_ok`] for the
/// one call this suite makes to another describe.
async fn describe_ok_action(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[Param],
) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The action that creates a gateway, as ec2Query names it.
const CREATE: &str = "CreateCustomerGateway";

/// One `CreateCustomerGateway` call carrying `params` beside the required `Type`.
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    let mut fields = vec![param("Type", GATEWAY_TYPE_IPSEC_1)];
    fields.extend(params.iter().cloned());
    call(router, CREATE, &fields).await
}

/// `CreateCustomerGateway` requiring success, and the gateway it answered.
async fn create_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> Reported {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {CREATE}, got {status} with body {body}"
    );
    created(case, &body)
        .unwrap_or_else(|| panic!("case {case:?}: expected a <{CREATED_GATEWAY}> in {body}"))
}

/// The form fields for `tags` as a `customer-gateway` tag specification.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_CUSTOMER_GATEWAY,
    )];
    for (position, (key, value)) in tags.iter().enumerate() {
        let index = position + 1;
        params.push(param(&format!("TagSpecification.1.Tag.{index}.Key"), key));
        params.push(param(&format!("TagSpecification.1.Tag.{index}.Value"), value));
    }
    params
}

/// One row of the create table: what makes it a different input, the request's fields beside
/// `Type`, the scalar members the gateway must then report besides its id and state, and the
/// tags it must report.
type CreateCase<'a> = (
    &'a str,
    Vec<Param>,
    Vec<(&'a str, &'a str)>,
    Vec<(String, String)>,
);

/// Rule 2: a create stores the gateway the request describes and answers it in service, and
/// every member of it reads back through the describe.
///
/// The rows differ along which members the request names: none beyond the required type, an
/// IPv4 device with a name and tags, the deprecated `PublicIp` spelling of that address, an
/// IPv6 device with an ASN only `BgpAsnExtended` can carry, and a device identified by a
/// certificate. The ASN rows sit at each published bound: 1 and 2,147,483,647 for `BgpAsn`,
/// 4,294,967,295 for `BgpAsnExtended`, and the documented default when the request names
/// neither.
#[tokio::test]
async fn a_create_stores_the_gateway_the_request_describes_and_reads_back() {
    let (router, _engine) = super::ec2_query::emulator();

    // Each row: what makes it a different input, the request's fields beside `Type`, the
    // scalar members the gateway must then report besides its id and state, and its tags.
    let cases: Vec<CreateCase> = vec![
        (
            "nothing but the required type",
            Vec::new(),
            // "Default: 65000" (`CreateCustomerGatewayRequest$BgpAsn`).
            vec![("bgpAsn", "65000")],
            Vec::new(),
        ),
        (
            "an IPv4 device with a name, the lowest ASN and two tags",
            [
                vec![
                    param("IpAddress", "203.0.113.12"),
                    param("BgpAsn", "1"),
                    param("DeviceName", "branch-office-router"),
                ],
                tag_params(&[("Name", "branch-office"), ("Owner", "TeamA")]),
            ]
            .concat(),
            vec![
                ("ipAddress", "203.0.113.12"),
                ("bgpAsn", "1"),
                ("deviceName", "branch-office-router"),
            ],
            vec![
                ("Name".to_string(), "branch-office".to_string()),
                ("Owner".to_string(), "TeamA".to_string()),
            ],
        ),
        (
            "the deprecated PublicIp spelling of the same address",
            vec![param("PublicIp", "198.51.100.7"), param("BgpAsn", "2147483647")],
            vec![("ipAddress", "198.51.100.7"), ("bgpAsn", "2147483647")],
            Vec::new(),
        ),
        (
            "an IPv6 device with the highest extended ASN",
            vec![
                param("IpAddress", "2001:db8::1"),
                param("BgpAsnExtended", "4294967295"),
            ],
            vec![
                ("ipAddress", "2001:db8::1"),
                ("bgpAsnExtended", "4294967295"),
            ],
            Vec::new(),
        ),
        (
            "a device identified by a certificate",
            vec![
                param("IpAddress", "192.0.2.44"),
                param(
                    "CertificateArn",
                    "arn:aws:acm:us-east-1:123456789012:certificate/\
                     12345678-1234-1234-1234-123456789012",
                ),
            ],
            vec![
                ("ipAddress", "192.0.2.44"),
                ("bgpAsn", "65000"),
                (
                    "certificateArn",
                    "arn:aws:acm:us-east-1:123456789012:certificate/\
                     12345678-1234-1234-1234-123456789012",
                ),
            ],
            Vec::new(),
        ),
    ];

    for (case, params, members, tags) in cases {
        let created = create_ok(case, &router, &params).await;
        let customer_gateway_id = created.id().to_string();
        assert!(
            is_long_id(CUSTOMER_GATEWAY_ID_PREFIX, &customer_gateway_id),
            "case {case:?}: expected a long {CUSTOMER_GATEWAY_ID_PREFIX}- id, got \
             {customer_gateway_id:?}"
        );

        // The members the request chose, plus the two the service owns: the type it asked for
        // and the state a create answers with.
        let mut expected = Reported {
            members: BTreeMap::from([
                ("customerGatewayId".to_string(), customer_gateway_id.clone()),
                ("state".to_string(), VPN_STATE_AVAILABLE.to_string()),
                ("type".to_string(), GATEWAY_TYPE_IPSEC_1.to_string()),
            ]),
            tags: tags.clone(),
        };
        for (name, value) in &members {
            expected
                .members
                .insert((*name).to_string(), (*value).to_string());
        }
        assert_eq!(
            created, expected,
            "case {case:?}: expected the create to answer the gateway it stored, got \
             {created:?}"
        );

        // The round trip: the describe reports exactly what the create answered.
        let body = describe_ok(case, &router, &id_params(&[&customer_gateway_id])).await;
        let read_back = reported(case, &body);
        assert_eq!(
            read_back,
            vec![expected],
            "case {case:?}: expected the describe to report the created gateway, got {body}"
        );

        // A gateway a create answers in service is owed no move, so the clock leaves it
        // exactly as it was reported.
        tokio::time::sleep(past_the_transition()).await;
        let body = describe_ok(case, &router, &id_params(&[&customer_gateway_id])).await;
        assert_eq!(
            reported(case, &body)[0].members.get("state").map(String::as_str),
            Some(VPN_STATE_AVAILABLE),
            "case {case:?}: expected the created gateway to stay in service, got {body}"
        );
    }
}

/// Rule 2, refusals: a value outside what the operation publishes is refused rather than
/// defaulted or stored, and nothing is written.
///
/// The rows differ along which published bound the request misses, and in which direction:
/// one below each ASN range and one above the extended one, the two ASN members given at
/// once, an outside address that is no address, the two spellings of that address disagreeing,
/// a device name one character past its documented length, and a tag specification naming
/// another resource type.
#[tokio::test]
async fn a_create_refuses_a_value_outside_what_it_publishes_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let cases: Vec<(&str, Vec<Param>, &str, String)> = vec![
        (
            "an ASN below what BgpAsn publishes",
            vec![param("BgpAsn", "0")],
            INVALID_PARAMETER_VALUE,
            "Value (0) for parameter bgpAsn is invalid. Invalid Format."
                .to_string(),
        ),
        (
            "an ASN below what BgpAsnExtended publishes",
            vec![param("BgpAsnExtended", "2147483647")],
            INVALID_PARAMETER_VALUE,
            "Value (2147483647) for parameter bgpAsnExtended is invalid. Invalid Format."
                .to_string(),
        ),
        (
            "an ASN above what BgpAsnExtended publishes",
            vec![param("BgpAsnExtended", "4294967296")],
            INVALID_PARAMETER_VALUE,
            "Value (4294967296) for parameter bgpAsnExtended is invalid. Invalid Format."
                .to_string(),
        ),
        (
            "both ASN members at once",
            vec![param("BgpAsn", "65000"), param("BgpAsnExtended", "4200000000")],
            "InvalidParameterCombination",
            "Either BgpAsn or BgpAsnExtended may be specified for a customer gateway, not \
             both"
                .to_string(),
        ),
        (
            "an outside address that is no address",
            vec![param("IpAddress", "203.0.113.999")],
            INVALID_PARAMETER_VALUE,
            "Value (203.0.113.999) for parameter ipAddress is invalid. Expected: an IPv4 or \
             IPv6 address."
                .to_string(),
        ),
        (
            "the two spellings of the address disagreeing",
            vec![
                param("IpAddress", "203.0.113.12"),
                param("PublicIp", "198.51.100.7"),
            ],
            "InvalidParameterCombination",
            "IpAddress and PublicIp name the same address of a customer gateway device, so \
             they cannot be given different values"
                .to_string(),
        ),
        (
            "a device name one character past its documented length",
            vec![param(
                "DeviceName",
                &"d".repeat(MAX_CUSTOMER_GATEWAY_DEVICE_NAME_CHARS + 1),
            )],
            INVALID_PARAMETER_VALUE,
            format!(
                "DeviceName carries {supplied} characters, above the maximum of {maximum}",
                supplied = MAX_CUSTOMER_GATEWAY_DEVICE_NAME_CHARS + 1,
                maximum = MAX_CUSTOMER_GATEWAY_DEVICE_NAME_CHARS,
            ),
        ),
        (
            "a tag specification naming another resource type",
            vec![
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "misdirected"),
            ],
            INVALID_PARAMETER_VALUE,
            format!("'vpc' is not a valid resource type for tagging on {CREATE}"),
        ),
    ];

    for (case, params, code, message) in cases {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the code {code}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected the refusal {message:?}, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: a refused create writes nothing to the store"
        );
        assert!(
            created(case, &body).is_none(),
            "case {case:?}: a refusal answers no gateway, got {body}"
        );
    }
}

/// Rule 2, idempotence: "An identical request returns information about the existing customer
/// gateway; it doesn't create a new customer gateway" — and a request that differs in what
/// that documentation calls identity creates another gateway.
///
/// The rows differ along which member of the documented identity (the type, the address, the
/// ASN and the device name) the second request changes, and along a member that is NOT part of
/// it.
#[tokio::test]
async fn an_identical_create_answers_the_gateway_it_already_made() {
    let (router, engine) = super::ec2_query::emulator();

    let first = vec![
        param("IpAddress", "203.0.113.12"),
        param("BgpAsn", "65000"),
        param("DeviceName", "branch-office-router"),
    ];
    let original = create_ok("the first create", &router, &first).await;
    let original_id = original.id().to_string();

    // Each row: what the second request changes, its fields, and whether it must answer the
    // gateway the first one made.
    let cases: Vec<(&str, Vec<Param>, bool)> = vec![
        ("the identical request", first.clone(), true),
        (
            "the same request carrying tags, which the identity does not name",
            [first.clone(), tag_params(&[("Name", "branch-office")])].concat(),
            true,
        ),
        (
            "a different device name",
            vec![
                param("IpAddress", "203.0.113.12"),
                param("BgpAsn", "65000"),
                param("DeviceName", "second-router"),
            ],
            false,
        ),
        (
            "a different ASN",
            vec![
                param("IpAddress", "203.0.113.12"),
                param("BgpAsn", "65001"),
                param("DeviceName", "branch-office-router"),
            ],
            false,
        ),
        (
            "a different outside address",
            vec![
                param("IpAddress", "198.51.100.7"),
                param("BgpAsn", "65000"),
                param("DeviceName", "branch-office-router"),
            ],
            false,
        ),
    ];

    let mut expected_count = stored_count(&engine);
    for (case, params, identical) in cases {
        let created = create_ok(case, &router, &params).await;
        if identical {
            assert_eq!(
                created.id(),
                original_id,
                "case {case:?}: expected the gateway {original_id} the first create made, \
                 got {created:?}"
            );
        } else {
            assert_ne!(
                created.id(),
                original_id,
                "case {case:?}: expected a gateway of its own, got {created:?}"
            );
            expected_count += 1;
        }
        assert_eq!(
            stored_count(&engine),
            expected_count,
            "case {case:?}: expected the store to hold {expected_count} gateways"
        );
    }
}

/// Rule 2, `DryRun`: a create that would have succeeded answers the permission outcome and
/// leaves the store exactly as it was.
#[tokio::test]
async fn a_dry_run_of_a_create_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    for (case, mut params) in [
        ("a dry run of the minimal create", Vec::new()),
        (
            "a dry run of a create naming every member",
            vec![
                param("IpAddress", "203.0.113.12"),
                param("BgpAsn", "65000"),
                param("DeviceName", "branch-office-router"),
            ],
        ),
    ] {
        params.push(param("DryRun", "true"));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected the code {DRY_RUN_OPERATION}, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: a dry run writes nothing to the store"
        );
        assert!(
            created(case, &body).is_none(),
            "case {case:?}: a dry run answers no gateway, got {body}"
        );
    }
}

/// The action that deletes a gateway, as ec2Query names it.
const DELETE: &str = "DeleteCustomerGateway";

/// One `DeleteCustomerGateway` call naming `customer_gateway_id`, with any extra `params`.
async fn delete(
    router: &MultiServiceRouter,
    customer_gateway_id: &str,
    params: &[Param],
) -> (u16, String) {
    let mut fields = vec![param("CustomerGatewayId", customer_gateway_id)];
    fields.extend(params.iter().cloned());
    call(router, DELETE, &fields).await
}

/// The `CustomerGateway$State` the describe reports for `customer_gateway_id`, or `None` when
/// it reports no such gateway.
async fn described_state(
    case: &str,
    router: &MultiServiceRouter,
    customer_gateway_id: &str,
) -> Option<String> {
    let body = describe_ok(case, router, &id_params(&[customer_gateway_id])).await;
    reported(case, &body)
        .first()
        .and_then(|gateway| gateway.members.get("state").cloned())
}

/// Rule 3: a delete takes the gateway an id names out of service — it keeps being reported, in
/// the terminal state of a gateway that is gone — and repeating the request changes nothing.
///
/// The rows differ along the state the gateway is deleted in: one just created, one seeded in
/// each other state the vocabulary carries. A gateway already gone stays as it is.
#[tokio::test]
async fn a_delete_takes_the_gateway_out_of_service() {
    let (router, engine) = super::ec2_query::emulator();

    let created = create_ok("a created gateway", &router, &[]).await;
    let cases: Vec<(&str, String)> = vec![
        ("a created gateway", created.id().to_string()),
        (
            "a gateway coming up",
            seed(&engine, gateway(VPN_STATE_PENDING)),
        ),
        (
            "a gateway on its way out",
            seed(&engine, gateway(VPN_STATE_DELETING)),
        ),
        ("a gateway already gone", seed(&engine, gateway(VPN_STATE_DELETED))),
    ];

    for (case, customer_gateway_id) in cases {
        let (status, body) = delete(&router, &customer_gateway_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {DELETE}, got {status} with body {body}"
        );
        assert_eq!(
            described_state(case, &router, &customer_gateway_id).await,
            Some(VPN_STATE_DELETED.to_string()),
            "case {case:?}: expected the gateway reported as {VPN_STATE_DELETED}"
        );

        // Repeating the delete reports the one deletion that already happened.
        let (status, body) = delete(&router, &customer_gateway_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated {DELETE} to succeed, got {status} with \
             body {body}"
        );
        assert_eq!(
            described_state(case, &router, &customer_gateway_id).await,
            Some(VPN_STATE_DELETED.to_string()),
            "case {case:?}: expected the repeated delete to change nothing"
        );
    }
}

/// Rule 3, refusals: an id the account holds no gateway for, an empty id and a gateway a VPN
/// connection still uses are each refused, and a dry run of a delete that would have succeeded
/// changes nothing.
///
/// The id rows are the same forms the describe refuses, so one resource is named the one way
/// whichever operation resolves it; the dependency row is the sentence
/// `DeleteCustomerGateway` publishes ("You must delete the VPN connection before you can
/// delete the customer gateway"), and its second row shows a connection no longer in service
/// not blocking the delete.
#[tokio::test]
async fn a_delete_refuses_an_unresolvable_id_a_used_gateway_and_a_dry_run() {
    let (router, engine) = super::ec2_query::emulator();
    let held = seed(&engine, gateway(VPN_STATE_AVAILABLE));

    for (case, id) in [
        ("a long-form id nothing holds", "cgw-00000000000000002"),
        ("a short-form id nothing holds", "cgw-00000000"),
        ("a value that is no id at all", "not-a-gateway"),
    ] {
        let (status, body) = delete(&router, id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_CUSTOMER_GATEWAY_ID_NOT_FOUND,
            "case {case:?}: expected the code {INVALID_CUSTOMER_GATEWAY_ID_NOT_FOUND}, got \
             {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The customerGateway ID '{id}' does not exist"),
            "case {case:?}: expected the refusal to name {id:?}, got {body}"
        );
    }

    // An empty id is a missing parameter rather than a gateway nothing holds.
    let case = "an empty id";
    let (status, body) = delete(&router, "", &[]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        MISSING_PARAMETER,
        "case {case:?}: expected the code {MISSING_PARAMETER}, got {body}"
    );

    // A dry run of a delete that would have succeeded leaves the gateway in service.
    let case = "a dry run of a delete";
    let (status, body) = delete(&router, &held, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        DRY_RUN_OPERATION,
        "case {case:?}: expected the code {DRY_RUN_OPERATION}, got {body}"
    );
    assert_eq!(
        described_state(case, &router, &held).await,
        Some(VPN_STATE_AVAILABLE.to_string()),
        "case {case:?}: a dry run leaves the gateway in service"
    );

    // A VPN connection still in service blocks the delete and names itself in the refusal.
    let case = "a gateway a VPN connection still uses";
    let vpn_connection_id = seed_connection(&engine, &held);
    let (status, body) = delete(&router, &held, &[]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        DEPENDENCY_VIOLATION,
        "case {case:?}: expected the code {DEPENDENCY_VIOLATION}, got {body}"
    );
    assert_eq!(
        error_message(&body),
        format!(
            "The customerGateway '{held}' is used by the VPN connection \
             '{vpn_connection_id}', which must be deleted first"
        ),
        "case {case:?}: expected the refusal to name the connection, got {body}"
    );
    assert_eq!(
        described_state(case, &router, &held).await,
        Some(VPN_STATE_AVAILABLE.to_string()),
        "case {case:?}: a refused delete leaves the gateway in service"
    );

    // The same connection on its way out no longer uses the gateway, so the delete proceeds.
    let case = "a gateway whose VPN connection is on its way out";
    let (status, body) = call(
        &router,
        "DeleteVpnConnection",
        &[param("VpnConnectionId", &vpn_connection_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the connection delete to succeed, got {status} with body \
         {body}"
    );
    let (status, body) = delete(&router, &held, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {DELETE}, got {status} with body {body}"
    );
    assert_eq!(
        described_state(case, &router, &held).await,
        Some(VPN_STATE_DELETED.to_string()),
        "case {case:?}: expected the gateway reported as {VPN_STATE_DELETED}"
    );
}

/// Every gateway the account holds right now, by `CustomerGatewayId` — the WHOLE report,
/// because what a delete must leave untouched is every gateway the request did not name.
async fn reported_by_id(case: &str, router: &MultiServiceRouter) -> BTreeMap<String, Reported> {
    let body = describe_ok(case, router, &[]).await;
    reported(case, &body)
        .into_iter()
        .map(|gateway| (gateway.id().to_string(), gateway))
        .collect()
}

/// The `(VpnConnectionId, CustomerGatewayId, state)` of every VPN connection the account
/// holds, in reported order: what a delete of some OTHER gateway must leave alone.
async fn reported_connections(
    case: &str,
    router: &MultiServiceRouter,
) -> Vec<(String, String, String)> {
    let body = describe_ok_action(case, router, "DescribeVpnConnections", &[]).await;
    let parsed = document(case, &body);
    set_items(parsed.root_element(), "vpnConnectionSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "vpnConnectionId").unwrap_or_default(),
                child_text(item, "customerGatewayId").unwrap_or_default(),
                child_text(item, "state").unwrap_or_default(),
            )
        })
        .collect()
}

/// Rule 3, scope: a delete takes out of service EXACTLY the gateway its id names — it moves
/// that gateway's `CustomerGateway$State` to the terminal `deleted` and changes nothing else.
///
/// Every other member the deleted gateway reports stays as the store holds it, the record
/// itself keeps being reported, every gateway the request did not name stays in the state it
/// was in — including one whose whole description is the same, so the gateway acted on is
/// resolved by its id rather than by what it describes — the VPN connection of another gateway
/// is reported exactly as before, and `deleted` is terminal: the clock owes a gone gateway no
/// further move.
///
/// The rows differ along which members the deleted gateway carries — the certificate form, the
/// device-and-tags form, the extended-ASN form with an IPv6 outside address — and along the
/// state it is deleted in, with every earlier row's gateway still gone.
#[tokio::test]
async fn a_delete_changes_only_the_named_gateway_and_only_its_state() {
    let (router, engine) = super::ec2_query::emulator();

    // A device identified by a certificate, still coming up.
    let mut certificate = gateway(VPN_STATE_PENDING);
    certificate.ip_address = Some("198.51.100.7".to_string());
    certificate.bgp_asn = Some("64512".to_string());
    certificate.certificate_arn = Some(
        "arn:aws:acm:us-east-1:123456789012:certificate/12345678-1234-1234-1234-123456789012"
            .to_string(),
    );

    // An IPv4 device in service, with a device name and a tag.
    let mut device = gateway(VPN_STATE_AVAILABLE);
    device.ip_address = Some("203.0.113.12".to_string());
    device.bgp_asn = Some("65000".to_string());
    device.device_name = Some("branch-office-router".to_string());
    device.tags = Some(vec![Tag {
        key: Some("Name".to_string()),
        value: Some("branch-office".to_string()),
    }]);

    // The same description under a different device name — the one thing that tells two
    // otherwise identical gateways apart ("specify a unique device name for each customer
    // gateway", `CreateCustomerGateway`).
    let mut twin = device.clone();
    twin.device_name = Some("branch-office-standby".to_string());

    // An IPv6 outside address with an ASN above what `BgpAsn` carries.
    let mut extended = gateway(VPN_STATE_AVAILABLE);
    extended.ip_address = Some("2001:db8::1".to_string());
    extended.bgp_asn_extended = Some("4200000000".to_string());

    // The gateway a VPN connection uses: no row below deletes it, so its connection must be
    // reported exactly as it was after every delete.
    let mut connected = gateway(VPN_STATE_AVAILABLE);
    connected.ip_address = Some("192.0.2.9".to_string());
    connected.bgp_asn = Some("65001".to_string());

    let certificate_id = seed(&engine, certificate);
    let device_id = seed(&engine, device);
    let twin_id = seed(&engine, twin);
    let extended_id = seed(&engine, extended);
    let connected_id = seed(&engine, connected);
    let connection = seed_connection(&engine, &connected_id);

    let before = reported_by_id("before any delete", &router).await;
    let connections_before = reported_connections("before any delete", &router).await;
    let stored_before = stored_count(&engine);

    let mut gone: Vec<String> = Vec::new();
    for (case, customer_gateway_id) in [
        ("a gateway identified by a certificate, coming up", &certificate_id),
        ("a gateway in service with a device name and a tag", &device_id),
        ("a gateway with an extended ASN and an IPv6 address", &extended_id),
    ] {
        let (status, body) = delete(&router, customer_gateway_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {DELETE}, got {status} with body {body}"
        );
        gone.push(customer_gateway_id.clone());

        // The whole report, gateway by gateway: each one already deleted carries the terminal
        // state, and NOTHING else differs from what the store held before the first delete.
        let expected: BTreeMap<String, Reported> = before
            .iter()
            .map(|(customer_gateway_id, reported)| {
                let mut reported = reported.clone();
                if gone.contains(customer_gateway_id) {
                    reported
                        .members
                        .insert("state".to_string(), VPN_STATE_DELETED.to_string());
                }
                (customer_gateway_id.clone(), reported)
            })
            .collect();
        let after = reported_by_id(case, &router).await;
        assert_eq!(
            after, expected,
            "case {case:?}: expected only the deleted gateways to differ, and only in their \
             state, got {after:?}"
        );

        // A deleted gateway is still a gateway the API reports, so no record left the store,
        // and no resource naming another gateway moved.
        let stored = stored_count(&engine);
        assert_eq!(
            stored, stored_before,
            "case {case:?}: expected the store to still hold {stored_before} gateways, got \
             {stored}"
        );
        let connections = reported_connections(case, &router).await;
        assert_eq!(
            connections, connections_before,
            "case {case:?}: expected the connection {connection:?} of {connected_id} reported \
             as before, got {connections:?}"
        );
    }

    // `deleted` is terminal and the gateways left alone are owed nothing either, so the clock
    // moves none of them once the transition span has passed.
    tokio::time::sleep(past_the_transition()).await;
    let case = "once the transition span has passed";
    let after = reported_by_id(case, &router).await;
    let state = |customer_gateway_id: &str| -> Option<String> {
        after
            .get(customer_gateway_id)
            .and_then(|reported| reported.members.get("state").cloned())
    };
    for customer_gateway_id in &gone {
        assert_eq!(
            state(customer_gateway_id),
            Some(VPN_STATE_DELETED.to_string()),
            "case {case:?}: expected {customer_gateway_id} still reported as \
             {VPN_STATE_DELETED}, got {after:?}"
        );
    }
    for customer_gateway_id in [&twin_id, &connected_id] {
        assert_eq!(
            state(customer_gateway_id),
            Some(VPN_STATE_AVAILABLE.to_string()),
            "case {case:?}: expected {customer_gateway_id} left in service, got {after:?}"
        );
    }
}
