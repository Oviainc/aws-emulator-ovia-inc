// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateDhcpOptions`, driven over the wire the service actually
//! speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: `CreateDhcpOptions` STORES THE OPTIONS THE REQUEST NAMES as a set
//! of the account, minting the set's id and deriving its owning account itself, so the set
//! reads back through `DescribeDhcpOptions` carrying exactly those options and the tags the
//! request assigned — and an option name the API does not publish, or values outside what
//! the named option publishes, is refused with `InvalidParameterValue` and nothing is
//! stored.
//!
//! Every table row is a distinct input along what that generalises over — which of the
//! published options the request names, how many of them, how many values each carries and
//! of what form (an address of either family, the reserved resolver word, free-form text, a
//! node type, a lease time) — so the one set a recording happens to create is one row among
//! many rather than the specification.

use std::collections::BTreeMap;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{elements, error_code, indexed};
use super::wire::{child, child_text, document, is_long_id, param, set_items, Param};

/// The Region every call here is signed for: a DHCP option set belongs to the account
/// rather than to one Region, so the Region only selects the ec2Query envelope.
const REGION: &str = ec2::state::DEFAULT_REGION;

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `CreateDhcpOptions` call, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateDhcpOptions", params).await
}

/// The `DhcpConfiguration.N.Key` / `DhcpConfiguration.N.Value.M` form fields for a list of
/// options, as ec2Query flattens a list of structures into indexed keys.
fn configuration_params(options: &[(&str, Vec<&str>)]) -> Vec<Param> {
    let mut fields = Vec::new();
    for (index, (key, values)) in options.iter().enumerate() {
        let nth = index + 1;
        fields.push(param(&format!("DhcpConfiguration.{nth}.Key"), key));
        fields.extend(indexed(&format!("DhcpConfiguration.{nth}.Value"), values));
    }
    fields
}

/// The `TagSpecification` form fields that tag the option set the call creates.
fn tag_params(resource_type: &str, tags: &[(&str, &str)]) -> Vec<Param> {
    let mut fields = vec![param("TagSpecification.1.ResourceType", resource_type)];
    for (index, (key, value)) in tags.iter().enumerate() {
        let nth = index + 1;
        fields.push(param(&format!("TagSpecification.1.Tag.{nth}.Key"), key));
        fields.push(param(&format!("TagSpecification.1.Tag.{nth}.Value"), value));
    }
    fields
}

/// One DHCP option set as a response reports it: the account that owns it, its options in
/// the order reported, and its tags.
#[derive(Debug, Default, PartialEq, Eq)]
struct ReportedSet {
    /// `<ownerId>`, the account the service derived.
    owner_id: String,
    /// `<dhcpConfigurationSet>`, as (option name, values).
    options: Vec<(String, Vec<String>)>,
    /// `<tagSet>`, as (key, value).
    tags: Vec<(String, String)>,
}

/// Every DHCP option set a response carries, by `dhcpOptionsId`.
///
/// Read through the XML tree rather than by substring, so an option's `<value>` is never
/// read as a tag's and an option set of a `DescribeDhcpOptions` page is never mixed with
/// its neighbour.
fn reported_sets(case: &str, body: &str) -> BTreeMap<String, ReportedSet> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "dhcpOptionsId").is_some())
        .map(|node| {
            let options = set_items(node, "dhcpConfigurationSet")
                .into_iter()
                .map(|item| {
                    let values = set_items(item, "valueSet")
                        .into_iter()
                        .map(|value| child_text(value, "value").unwrap_or_default())
                        .collect();
                    (child_text(item, "key").unwrap_or_default(), values)
                })
                .collect();
            let tags = set_items(node, "tagSet")
                .into_iter()
                .map(|item| {
                    (
                        child_text(item, "key").unwrap_or_default(),
                        child_text(item, "value").unwrap_or_default(),
                    )
                })
                .collect();
            (
                child_text(node, "dhcpOptionsId").unwrap_or_default(),
                ReportedSet {
                    owner_id: child_text(node, "ownerId").unwrap_or_default(),
                    options,
                    tags,
                },
            )
        })
        .collect()
}

/// The one option set a `CreateDhcpOptions` response carries, with its id.
fn created_set(case: &str, body: &str) -> (String, ReportedSet) {
    let mut sets = reported_sets(case, body);
    assert_eq!(
        sets.len(),
        1,
        "case {case:?}: expected ONE option set in the response, got {body}"
    );
    sets.pop_first()
        .unwrap_or_else(|| panic!("case {case:?}: no option set in {body}"))
}

/// The one option set `DescribeDhcpOptions` reports for `dhcp_options_id`.
async fn described_set(
    case: &str,
    router: &MultiServiceRouter,
    dhcp_options_id: &str,
) -> (String, ReportedSet) {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeDhcpOptions",
        &indexed("DhcpOptionsId", &[dhcp_options_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading {dhcp_options_id} back, got {status} with \
         body {body}"
    );
    created_set(case, &body)
}

/// Every `dhcpOptionsId` the account holds.
async fn held_ids(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = super::ec2_query::call(router, REGION, "DescribeDhcpOptions", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeDhcpOptions, got {status} with body \
         {body}"
    );
    elements(&body, "dhcpOptionsId")
}

/// One row of the create table: the case, the options the request names and the tags it
/// assigns.
type CreateCase = (
    &'static str,
    Vec<(&'static str, Vec<&'static str>)>,
    Vec<(&'static str, &'static str)>,
);

// ---------------------------------------------------------------------------
// What a create stores
// ---------------------------------------------------------------------------

/// The rule: the set a `CreateDhcpOptions` answers carries exactly the options the request
/// named, with an id the service minted and the account that owns it, and reads back
/// through `DescribeDhcpOptions` identically.
///
/// The rows span the published option catalog: each option on its own, the forms its values
/// can take (an IPv4 address, an IPv6 address, the reserved resolver word, free-form text),
/// the counts it admits at its published ceiling, and every option at once with tags.
#[tokio::test]
async fn create_dhcp_options_stores_the_options_the_request_names() {
    let router = emulator();
    let account = engine::emulator_account_id().to_string();
    let before = held_ids("before any create", &router).await;

    let cases: Vec<CreateCase> = vec![
        // `domain-name` — "Otherwise, specify a custom domain name."
        (
            "one custom domain name",
            vec![(ec2::state::DHCP_OPTION_DOMAIN_NAME, vec!["example.internal"])],
            Vec::new(),
        ),
        // The documentation's own us-east-1 value, which is also the Region's private DNS
        // domain: a caller may name it explicitly.
        (
            "the documented us-east-1 domain name",
            vec![(ec2::state::DHCP_OPTION_DOMAIN_NAME, vec!["ec2.internal"])],
            Vec::new(),
        ),
        // "Some Linux operating systems accept multiple domain names separated by spaces" —
        // one value carrying a space is still one value.
        (
            "a domain name carrying a space",
            vec![(
                ec2::state::DHCP_OPTION_DOMAIN_NAME,
                vec!["example.internal corp.example.internal"],
            )],
            Vec::new(),
        ),
        // `domain-name-servers` — "The IP addresses of up to four DNS servers, or
        // AmazonProvidedDNS."
        (
            "the Amazon-provided resolver",
            vec![(
                ec2::state::DHCP_OPTION_DOMAIN_NAME_SERVERS,
                vec![ec2::state::AMAZON_PROVIDED_DNS],
            )],
            Vec::new(),
        ),
        (
            "one DNS server",
            vec![(
                ec2::state::DHCP_OPTION_DOMAIN_NAME_SERVERS,
                vec!["10.0.0.2"],
            )],
            Vec::new(),
        ),
        (
            "four DNS servers, the published ceiling",
            vec![(
                ec2::state::DHCP_OPTION_DOMAIN_NAME_SERVERS,
                vec!["10.0.0.2", "10.0.0.3", "192.0.2.1", "2001:db8::2"],
            )],
            Vec::new(),
        ),
        // `ntp-servers` — "up to eight Network Time Protocol (NTP) servers (four IPv4
        // addresses and four IPv6 addresses)".
        (
            "eight NTP servers, four of each family",
            vec![(
                ec2::state::DHCP_OPTION_NTP_SERVERS,
                vec![
                    "169.254.169.123",
                    "10.0.0.4",
                    "192.0.2.2",
                    "198.51.100.3",
                    "fd00:ec2::123",
                    "2001:db8::1",
                    "2001:db8::2",
                    "2001:db8::3",
                ],
            )],
            Vec::new(),
        ),
        // `netbios-name-servers` — "The IP addresses of up to four NetBIOS name servers."
        (
            "four NetBIOS name servers",
            vec![(
                ec2::state::DHCP_OPTION_NETBIOS_NAME_SERVERS,
                vec!["10.0.0.5", "10.0.0.6", "10.0.0.7", "10.0.0.8"],
            )],
            Vec::new(),
        ),
        // `netbios-node-type` — "The NetBIOS node type (1, 2, 4, or 8)", each published
        // value a row of its own.
        (
            "node type 1",
            vec![(ec2::state::DHCP_OPTION_NETBIOS_NODE_TYPE, vec!["1"])],
            Vec::new(),
        ),
        (
            "node type 2, the recommended one",
            vec![(ec2::state::DHCP_OPTION_NETBIOS_NODE_TYPE, vec!["2"])],
            Vec::new(),
        ),
        (
            "node type 4",
            vec![(ec2::state::DHCP_OPTION_NETBIOS_NODE_TYPE, vec!["4"])],
            Vec::new(),
        ),
        (
            "node type 8",
            vec![(ec2::state::DHCP_OPTION_NETBIOS_NODE_TYPE, vec!["8"])],
            Vec::new(),
        ),
        // `ipv6-address-preferred-lease-time` — "Acceptable values are between 140 and
        // 2147483647 seconds": each published bound, and a value inside the range.
        (
            "the shortest lease time",
            vec![(
                ec2::state::DHCP_OPTION_IPV6_ADDRESS_PREFERRED_LEASE_TIME,
                vec!["140"],
            )],
            Vec::new(),
        ),
        (
            "an hour of lease time",
            vec![(
                ec2::state::DHCP_OPTION_IPV6_ADDRESS_PREFERRED_LEASE_TIME,
                vec!["3600"],
            )],
            Vec::new(),
        ),
        (
            "the longest lease time",
            vec![(
                ec2::state::DHCP_OPTION_IPV6_ADDRESS_PREFERRED_LEASE_TIME,
                vec!["2147483647"],
            )],
            Vec::new(),
        ),
        // Several options at once, in an order the catalog does not list them in: the set
        // reports what the request named.
        (
            "a domain and its servers",
            vec![
                (
                    ec2::state::DHCP_OPTION_DOMAIN_NAME_SERVERS,
                    vec!["10.1.0.2", "10.1.0.3"],
                ),
                (ec2::state::DHCP_OPTION_DOMAIN_NAME, vec!["corp.internal"]),
            ],
            Vec::new(),
        ),
        // Every published option at once, tagged.
        (
            "every option, with tags",
            vec![
                (ec2::state::DHCP_OPTION_DOMAIN_NAME, vec!["all.internal"]),
                (
                    ec2::state::DHCP_OPTION_DOMAIN_NAME_SERVERS,
                    vec!["10.2.0.2"],
                ),
                (ec2::state::DHCP_OPTION_NTP_SERVERS, vec!["10.2.0.3"]),
                (
                    ec2::state::DHCP_OPTION_NETBIOS_NAME_SERVERS,
                    vec!["10.2.0.4"],
                ),
                (ec2::state::DHCP_OPTION_NETBIOS_NODE_TYPE, vec!["2"]),
                (
                    ec2::state::DHCP_OPTION_IPV6_ADDRESS_PREFERRED_LEASE_TIME,
                    vec!["1200"],
                ),
            ],
            vec![("Name", "every-option"), ("Owner", "TeamA")],
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, options, tags) in &cases {
        let mut params = configuration_params(options);
        if !tags.is_empty() {
            params.extend(tag_params(ec2::state::RESOURCE_TYPE_DHCP_OPTIONS, tags));
        }
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateDhcpOptions, got {status} with body \
             {body}"
        );

        let (dhcp_options_id, reported) = created_set(case, &body);
        assert!(
            is_long_id("dopt", &dhcp_options_id),
            "case {case:?}: expected a dopt-<17 hex> id, got {dhcp_options_id:?}"
        );
        assert!(
            !minted.contains(&dhcp_options_id),
            "case {case:?}: every create mints a new id, got {dhcp_options_id:?} again"
        );

        let expected = ReportedSet {
            owner_id: account.clone(),
            options: options
                .iter()
                .map(|(key, values)| {
                    (
                        (*key).to_string(),
                        values.iter().map(|value| (*value).to_string()).collect(),
                    )
                })
                .collect(),
            tags: tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the set {expected:?}, got {reported:?}"
        );

        // The round trip: the set reads back through `DescribeDhcpOptions` exactly as the
        // create reported it.
        let (read_id, read_back) = described_set(case, &router, &dhcp_options_id).await;
        assert_eq!(
            read_id, dhcp_options_id,
            "case {case:?}: expected to read {dhcp_options_id} back, got {read_id}"
        );
        assert_eq!(
            read_back, expected,
            "case {case:?}: expected {expected:?} from DescribeDhcpOptions, got {read_back:?}"
        );

        minted.push(dhcp_options_id);
    }

    // Every created set joined the ONE store of option sets, beside what the account
    // already held.
    let mut expected_held = before;
    expected_held.extend(minted.iter().cloned());
    expected_held.sort();
    let mut held = held_ids("after every create", &router).await;
    held.sort();
    assert_eq!(
        held, expected_held,
        "expected the account to hold every created set, got {held:?}"
    );
}

// ---------------------------------------------------------------------------
// What a create refuses
// ---------------------------------------------------------------------------

/// One row of the refusal table: the case, the form fields it sends, the HTTP status and the
/// EC2 error code it must be refused with.
type RefusalCase = (&'static str, Vec<Param>, u16, &'static str);

/// The rule: an option name the API does not publish, values outside what the named option
/// publishes, a request naming no option at all and a request naming one option twice are
/// all refused, and nothing is stored.
///
/// The rows span the ways a request can be outside the rule, and the published ranges are
/// probed one past each bound ("Acceptable values are between 140 and 2147483647 seconds",
/// "up to four DNS servers", "The NetBIOS node type (1, 2, 4, or 8)").
#[tokio::test]
async fn create_dhcp_options_refuses_what_the_options_do_not_admit() {
    let router = emulator();
    let before = held_ids("before every refusal", &router).await;

    let cases: Vec<RefusalCase> = vec![
        // A required member the request omits entirely is refused by the service's own
        // request boundary, with `MalformedInput` rather than the handler's
        // `MissingParameter` — the same answer every other operation of this service gives
        // for an absent required member.
        (
            "no DhcpConfigurations at all",
            Vec::new(),
            400,
            "MalformedInput",
        ),
        (
            "an option with no value",
            configuration_params(&[(ec2::state::DHCP_OPTION_DOMAIN_NAME, Vec::new())]),
            400,
            "MissingParameter",
        ),
        (
            "an option name the API does not publish",
            configuration_params(&[("domain-search", vec!["example.internal"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "an option name in the wrong case",
            configuration_params(&[("Domain-Name", vec!["example.internal"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "one option named twice",
            configuration_params(&[
                (ec2::state::DHCP_OPTION_DOMAIN_NAME, vec!["first.internal"]),
                (ec2::state::DHCP_OPTION_DOMAIN_NAME, vec!["second.internal"]),
            ]),
            400,
            "InvalidParameterValue",
        ),
        (
            "five DNS servers, one past the published ceiling",
            configuration_params(&[(
                ec2::state::DHCP_OPTION_DOMAIN_NAME_SERVERS,
                vec!["10.0.0.2", "10.0.0.3", "10.0.0.4", "10.0.0.5", "10.0.0.6"],
            )]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a DNS server that is not an address",
            configuration_params(&[(
                ec2::state::DHCP_OPTION_DOMAIN_NAME_SERVERS,
                vec!["ns1.example.internal"],
            )]),
            400,
            "InvalidParameterValue",
        ),
        (
            "five IPv4 NTP servers, one past the per-family ceiling",
            configuration_params(&[(
                ec2::state::DHCP_OPTION_NTP_SERVERS,
                vec!["10.0.0.2", "10.0.0.3", "10.0.0.4", "10.0.0.5", "10.0.0.6"],
            )]),
            400,
            "InvalidParameterValue",
        ),
        (
            "five IPv6 NTP servers, one past the per-family ceiling",
            configuration_params(&[(
                ec2::state::DHCP_OPTION_NTP_SERVERS,
                vec![
                    "2001:db8::1",
                    "2001:db8::2",
                    "2001:db8::3",
                    "2001:db8::4",
                    "2001:db8::5",
                ],
            )]),
            400,
            "InvalidParameterValue",
        ),
        (
            "five NetBIOS name servers, one past the published ceiling",
            configuration_params(&[(
                ec2::state::DHCP_OPTION_NETBIOS_NAME_SERVERS,
                vec!["10.0.0.2", "10.0.0.3", "10.0.0.4", "10.0.0.5", "10.0.0.6"],
            )]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a NetBIOS node type the API does not publish",
            configuration_params(&[(ec2::state::DHCP_OPTION_NETBIOS_NODE_TYPE, vec!["3"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "two NetBIOS node types",
            configuration_params(&[(
                ec2::state::DHCP_OPTION_NETBIOS_NODE_TYPE,
                vec!["2", "4"],
            )]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a lease time one second below the published floor",
            configuration_params(&[(
                ec2::state::DHCP_OPTION_IPV6_ADDRESS_PREFERRED_LEASE_TIME,
                vec!["139"],
            )]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a lease time one second above the published ceiling",
            configuration_params(&[(
                ec2::state::DHCP_OPTION_IPV6_ADDRESS_PREFERRED_LEASE_TIME,
                vec!["2147483648"],
            )]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a lease time that is not a number of seconds",
            configuration_params(&[(
                ec2::state::DHCP_OPTION_IPV6_ADDRESS_PREFERRED_LEASE_TIME,
                vec!["1h"],
            )]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag specification for another resource type",
            {
                let mut params = configuration_params(&[(
                    ec2::state::DHCP_OPTION_DOMAIN_NAME,
                    vec!["example.internal"],
                )]);
                params.extend(tag_params(
                    ec2::state::RESOURCE_TYPE_VPC,
                    &[("Name", "misdirected")],
                ));
                params
            },
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag key in the reserved namespace",
            {
                let mut params = configuration_params(&[(
                    ec2::state::DHCP_OPTION_DOMAIN_NAME,
                    vec!["example.internal"],
                )]);
                params.extend(tag_params(
                    ec2::state::RESOURCE_TYPE_DHCP_OPTIONS,
                    &[("aws:created-by", "me")],
                ));
                params
            },
            400,
            "InvalidParameterValue",
        ),
        // "If you have the required permissions, the error response is DryRunOperation"
        // (`CreateDhcpOptionsRequest$DryRun`), which EC2 answers with HTTP 412 — on a
        // request that would otherwise have succeeded, so nothing is stored.
        (
            "a dry run of a request that would succeed",
            {
                let mut params = configuration_params(&[(
                    ec2::state::DHCP_OPTION_DOMAIN_NAME,
                    vec!["dry.internal"],
                )]);
                params.push(param("DryRun", "true"));
                params
            },
            412,
            "DryRunOperation",
        ),
    ];

    for (case, params, status_expected, code_expected) in &cases {
        let (status, body) = create(&router, params).await;
        assert_eq!(
            status, *status_expected,
            "case {case:?}: expected HTTP {status_expected} from CreateDhcpOptions, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *code_expected,
            "case {case:?}: expected {code_expected}, got {body}"
        );
    }

    let held = held_ids("after every refusal", &router).await;
    assert_eq!(
        held, before,
        "no refusal may store an option set: expected {before:?}, got {held:?}"
    );
}

/// The rule: the option set store is BOUNDED — a create is refused with
/// `DhcpOptionsLimitExceeded` once the account holds [`ec2::state::MAX_DHCP_OPTION_SETS`]
/// sets, the same ceiling the Region's default set counts against, and nothing is stored.
#[tokio::test]
async fn create_dhcp_options_is_refused_once_the_store_is_full() {
    let router = emulator();
    // The store starts with the seeded default VPC's set, so this many more fill it exactly.
    let before = held_ids("before the fill", &router).await;
    let remaining = ec2::state::MAX_DHCP_OPTION_SETS - before.len();
    for index in 0..remaining {
        let case = format!("fill {index}");
        let domain = format!("fill{index}.internal");
        let params =
            configuration_params(&[(ec2::state::DHCP_OPTION_DOMAIN_NAME, vec![domain.as_str()])]);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a set inside the ceiling must be created, got {status} with body \
             {body}"
        );
    }
    let full = held_ids("a full store", &router).await;
    assert_eq!(
        full.len(),
        ec2::state::MAX_DHCP_OPTION_SETS,
        "the fill must take the store to its ceiling, got {} sets",
        full.len()
    );

    let (status, body) = create(
        &router,
        &configuration_params(&[(
            ec2::state::DHCP_OPTION_DOMAIN_NAME,
            vec!["over-the-ceiling.internal"],
        )]),
    )
    .await;
    assert_eq!(
        status, 400,
        "a create past the ceiling must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::DHCP_OPTIONS_LIMIT_EXCEEDED,
        "expected {}, got {body}",
        ec2::state::DHCP_OPTIONS_LIMIT_EXCEEDED
    );
    let after = held_ids("after the refusal", &router).await;
    assert_eq!(
        after, full,
        "the refused create must store nothing: expected {full:?}, got {after:?}"
    );
}

/// The rule: a set a caller created is a set the account HOLDS, so `AssociateDhcpOptions`
/// resolves its id and the VPC reports it from then on — the same store
/// `DescribeDhcpOptions` reads.
#[tokio::test]
async fn a_created_option_set_can_be_associated_with_a_vpc() {
    let router = emulator();
    let (status, body) = create(
        &router,
        &configuration_params(&[
            (ec2::state::DHCP_OPTION_DOMAIN_NAME, vec!["assoc.internal"]),
            (
                ec2::state::DHCP_OPTION_DOMAIN_NAME_SERVERS,
                vec!["10.3.0.2"],
            ),
        ]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the set to associate must be created, got {status} with body {body}"
    );
    let (dhcp_options_id, _) = created_set("the set to associate", &body);

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "CreateVpc",
        &[param("CidrBlock", "10.3.0.0/16")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the VPC to associate must be created, got {status} with body {body}"
    );
    let vpc_id = super::wire::member("the VPC to associate", &body, "vpcId");
    let inherited = super::wire::member("the VPC to associate", &body, "dhcpOptionsId");
    assert_ne!(
        inherited, dhcp_options_id,
        "a new VPC inherits the Region's default set, not the created one"
    );

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "AssociateDhcpOptions",
        &[
            param("DhcpOptionsId", &dhcp_options_id),
            param("VpcId", &vpc_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 associating {dhcp_options_id} with {vpc_id}, got {status} with \
         body {body}"
    );

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "DescribeVpcs",
        &indexed("VpcId", &[vpc_id.as_str()]),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 reading {vpc_id} back, got {status} with body {body}"
    );
    let reported = super::wire::member("after the association", &body, "dhcpOptionsId");
    assert_eq!(
        reported, dhcp_options_id,
        "expected {vpc_id} to report the created set, got {reported:?}"
    );
}
