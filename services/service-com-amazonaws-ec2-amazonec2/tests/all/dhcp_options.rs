// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the DHCP option set a VPC inherits — `CreateVpc` and
//! `DescribeDhcpOptions` — driven over the wire the service actually speaks: `ec2Query`,
//! an `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by
//! the `ec2` signing name in the request's SigV4 credential scope, so every call here
//! carries one.
//!
//! The rule under test is that A VPC'S `dhcpOptionsId` NAMES A DHCP OPTION SET THE ACCOUNT
//! HOLDS: the ONE default option set of the Region the VPC was created in, which
//! `DescribeDhcpOptions` reports with the two options AWS puts in a default set —
//! `domain-name` at the Region's private DNS domain and `domain-name-servers` at
//! `AmazonProvidedDNS` (Amazon VPC User Guide, "DHCP option sets"). So every VPC of one
//! Region reports the same set, a VPC of another Region reports that Region's, and the id
//! a VPC reports always reads back through `DescribeDhcpOptions`.
//!
//! Every table row is a distinct input along what that generalises over — the Region and
//! its private DNS domain, how many VPCs of a Region ask for a set, which filter selects,
//! how many ids are named, the page size — so the one Region a recording happens to be
//! made in is one row among many rather than the specification.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, owned};
use super::wire::{is_long_id, param, Param};

/// The Region the account's seeded default VPC belongs to, and so the Region whose default
/// option set a fresh store already holds ([`ec2::state::DEFAULT_REGION`]).
const SEEDED_REGION: &str = ec2::state::DEFAULT_REGION;

/// The Regions the tables below create VPCs in, with the private DNS domain each one's
/// default option set carries as its `domain-name`.
///
/// The domains are the rule [`ec2::state::private_dns_domain`] states, not a transcription
/// of a recording: `ec2.internal` in `us-east-1` ([`ec2::state::EC2_INTERNAL_REGION`], the
/// one Region that does not follow the pattern) and `<region>.compute.internal` everywhere
/// else. Six Regions, so the store holds more option sets than the smallest page size the
/// model admits (`DescribeDhcpOptionsMaxResults`, `range 5..1000`).
const REGIONS: [(&str, &str); 6] = [
    ("us-east-1", "ec2.internal"),
    ("us-west-2", "us-west-2.compute.internal"),
    ("eu-west-1", "eu-west-1.compute.internal"),
    ("eu-central-1", "eu-central-1.compute.internal"),
    ("ap-southeast-1", "ap-southeast-1.compute.internal"),
    ("sa-east-1", "sa-east-1.compute.internal"),
];

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call signed for `region` that has to succeed, and the body it answered.
async fn ok(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[Param],
) -> String {
    let (status, body) = super::ec2_query::call(router, region, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case and showing the body.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// A VPC with `cidr_block` created in `region`, as the `dhcpOptionsId` it reports.
async fn created_vpc_option_set(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    cidr_block: &str,
) -> String {
    let body = ok(
        case,
        router,
        region,
        "CreateVpc",
        &[param("CidrBlock", cidr_block)],
    )
    .await;
    member(case, &body, "dhcpOptionsId")
}

/// The `dhcpOptionsId`s `DescribeDhcpOptions` reports for `params`, signed for
/// [`SEEDED_REGION`] — the store is per account rather than per Region, so the Region of
/// the read only selects the ec2Query envelope.
async fn described_ids(case: &str, router: &MultiServiceRouter, params: &[Param]) -> Vec<String> {
    let body = ok(case, router, SEEDED_REGION, "DescribeDhcpOptions", params).await;
    elements(&body, "dhcpOptionsId")
}

/// One `Filters` entry of a table row: the filter's name and its values, owned because a
/// row filters on an id the test only learns at run time.
type FilterEntry = (&'static str, Vec<String>);

/// One row of the filter table: the case, the filters it sends and the option set ids it
/// expects back.
type FilterCase = (&'static str, Vec<FilterEntry>, Vec<String>);

/// The form fields for a `Filters` list whose values are OWNED, as
/// [`super::ec2_query::filters`] writes them — the tables below filter on ids the test
/// only learns at run time.
fn filter_params(entries: &[FilterEntry]) -> Vec<Param> {
    let borrowed: Vec<(&str, Vec<&str>)> = entries
        .iter()
        .map(|(name, values)| (*name, values.iter().map(String::as_str).collect()))
        .collect();
    let slices: Vec<(&str, &[&str])> = borrowed
        .iter()
        .map(|(name, values)| (*name, values.as_slice()))
        .collect();
    filters(&slices)
}

// ---------------------------------------------------------------------------
// What a VPC inherits
// ---------------------------------------------------------------------------

/// The rule: every VPC created in one Region reports that Region's one default DHCP option
/// set, a VPC of another Region reports that Region's own, and the set each id names reads
/// back through `DescribeDhcpOptions` carrying `domain-name` at the Region's private DNS
/// domain and `domain-name-servers` at `AmazonProvidedDNS`.
///
/// The rows are the Regions of [`REGIONS`] — including the one Region whose private DNS
/// domain does not follow the pattern — and each creates several VPCs, so "the same set"
/// is asserted over a count rather than on a single call.
#[tokio::test]
async fn every_vpc_of_a_region_inherits_that_regions_default_dhcp_option_set() {
    let router = emulator();
    let mut per_region: Vec<(&str, &str, String)> = Vec::new();

    for (index, (region, domain)) in REGIONS.iter().enumerate() {
        let case = *region;
        // Three VPCs of the Region, each with a block of its own: the set is a fact of the
        // Region, so the second and third must report the first's id.
        let mut ids = Vec::new();
        for vpc in 0..3u8 {
            let cidr_block = format!("10.{}.{vpc}.0/24", index + 20);
            ids.push(created_vpc_option_set(case, &router, region, &cidr_block).await);
        }
        let first = ids.first().expect("three VPCs were created").clone();
        assert!(
            is_long_id("dopt", &first),
            "case {case:?}: expected a dopt-<17 hex> option set, got {first:?}"
        );
        for id in &ids {
            assert_eq!(
                id, &first,
                "case {case:?}: every VPC of one Region inherits the same option set, got \
                 {ids:?}"
            );
        }

        // The round trip: the id the VPC reported names a set the account holds.
        let body = ok(
            case,
            &router,
            region,
            "DescribeDhcpOptions",
            &indexed("DhcpOptionsId", &[&first]),
        )
        .await;
        assert_eq!(
            elements(&body, "dhcpOptionsId"),
            vec![first.clone()],
            "case {case:?}: expected exactly the inherited option set, got {body}"
        );
        assert_eq!(
            member(case, &body, "ownerId"),
            engine::emulator_account_id(),
            "case {case:?}: expected the emulator account as owner, got {body}"
        );
        assert_eq!(
            elements(&body, "key"),
            owned(&[
                ec2::state::DHCP_OPTION_DOMAIN_NAME,
                ec2::state::DHCP_OPTION_DOMAIN_NAME_SERVERS,
            ]),
            "case {case:?}: a default set carries the two documented options, got {body}"
        );
        assert_eq!(
            elements(&body, "value"),
            owned(&[domain, ec2::state::AMAZON_PROVIDED_DNS]),
            "case {case:?}: expected the Region's private DNS domain and the Amazon \
             resolver, got {body}"
        );
        per_region.push((region, domain, first));
    }

    // One set per Region, and no Region sharing another's.
    for (region, _, id) in &per_region {
        let sharing: Vec<&str> = per_region
            .iter()
            .filter(|(other, _, other_id)| other != region && other_id == id)
            .map(|(other, _, _)| *other)
            .collect();
        assert!(
            sharing.is_empty(),
            "case {region:?}: a Region's default option set is its own, but {sharing:?} \
             report {id:?} too"
        );
    }
    // The Region the account's default VPC belongs to was one of the rows, so its set is
    // the one the store already held rather than a second one minted beside it.
    let seeded = ok(
        "the seeded Region",
        &router,
        SEEDED_REGION,
        "DescribeVpcs",
        &filters(&[("is-default", &["true"])]),
    )
    .await;
    let seeded_set = member("the seeded Region", &seeded, "dhcpOptionsId");
    let (_, _, created_in_seeded_region) = per_region
        .iter()
        .find(|(region, _, _)| *region == SEEDED_REGION)
        .expect("the seeded Region is one of the rows");
    assert_eq!(
        &seeded_set, created_in_seeded_region,
        "a VPC created in the account's own Region inherits the set its default VPC \
         already had, got {seeded_set:?}"
    );
}

/// The rule: the option set store is BOUNDED — the Region a set is minted for comes from
/// the caller's credential scope, so a `CreateVpc` whose Region has no set yet is refused
/// with `DhcpOptionsLimitExceeded` once the store holds [`ec2::state::MAX_DHCP_OPTION_SETS`]
/// of them, while a VPC of a Region whose set is already held still succeeds.
#[tokio::test]
async fn a_vpc_of_a_new_region_is_refused_once_the_option_set_store_is_full() {
    let router = emulator();
    // The store starts with the seeded default VPC's set, so this many more Regions fill
    // it exactly.
    let remaining = ec2::state::MAX_DHCP_OPTION_SETS - 1;
    for index in 0..remaining {
        let region = format!("zz-fill-{index}");
        let case = region.clone();
        created_vpc_option_set(&case, &router, &region, "10.0.0.0/16").await;
    }
    let held = described_ids("a full store", &router, &[]).await;
    assert_eq!(
        held.len(),
        ec2::state::MAX_DHCP_OPTION_SETS,
        "the fill must take the store to its ceiling, got {} sets",
        held.len()
    );

    // One Region past the ceiling: refused, and nothing stored.
    let (status, body) = super::ec2_query::call(
        &router,
        "zz-over-the-ceiling",
        "CreateVpc",
        &[param("CidrBlock", "10.0.0.0/16")],
    )
    .await;
    assert_eq!(
        status, 400,
        "a Region past the ceiling must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::DHCP_OPTIONS_LIMIT_EXCEEDED,
        "expected {}, got {body}",
        ec2::state::DHCP_OPTIONS_LIMIT_EXCEEDED
    );

    // A Region whose set the store already holds is unaffected: the ceiling is on the
    // SETS, not on the VPCs.
    let inherited =
        created_vpc_option_set("a Region already held", &router, SEEDED_REGION, "10.1.0.0/16")
            .await;
    let after = described_ids("after the refusal", &router, &[]).await;
    assert_eq!(
        after, held,
        "neither the refusal nor a VPC of a held Region changes the store, got {after:?} \
         where {held:?} was there"
    );
    assert!(
        after.contains(&inherited),
        "the VPC of a held Region inherited a set the store holds, got {inherited:?}"
    );
}

/// The rule: a fresh account already holds the default option set its default VPC names, so
/// `DescribeDhcpOptions` reports it before anything has created a VPC.
#[tokio::test]
async fn a_fresh_account_holds_the_option_set_its_default_vpc_names() {
    let case = "a fresh account";
    let router = emulator();
    let vpcs = ok(
        case,
        &router,
        SEEDED_REGION,
        "DescribeVpcs",
        &filters(&[("is-default", &["true"])]),
    )
    .await;
    let named = member(case, &vpcs, "dhcpOptionsId");

    let reported = described_ids(case, &router, &[]).await;
    assert_eq!(
        reported,
        vec![named.clone()],
        "case {case:?}: expected exactly the default VPC's option set {named:?}, got \
         {reported:?}"
    );
    // A read changes nothing: the second call reports the same store as the first.
    let again = described_ids(case, &router, &[]).await;
    assert_eq!(
        again, reported,
        "case {case:?}: a describe must not change what a later caller sees, got {again:?} \
         where {reported:?} was there"
    );
}

// ---------------------------------------------------------------------------
// What DescribeDhcpOptions selects
// ---------------------------------------------------------------------------

/// Create one VPC in each Region of [`REGIONS`], and answer `(region, domain, option set
/// id)` per Region in the order the store reports them.
async fn one_vpc_per_region(router: &MultiServiceRouter) -> Vec<(&'static str, &'static str, String)> {
    let mut sets = Vec::new();
    for (index, (region, domain)) in REGIONS.iter().enumerate() {
        let cidr_block = format!("10.{}.0.0/16", index + 40);
        let id = created_vpc_option_set(region, router, region, &cidr_block).await;
        sets.push((*region, *domain, id));
    }
    sets
}

/// The rule: `DescribeDhcpOptions` reports the store narrowed by the `DhcpOptionsIds` and
/// the `Filters` the request names, each documented filter selecting on the option set's
/// own attributes.
///
/// The rows differ in which filter selects, how many values it carries and how many sets it
/// is expected to pick, so no row is the recorded call.
#[tokio::test]
async fn describe_dhcp_options_selects_by_id_and_by_every_documented_filter() {
    let router = emulator();
    let sets = one_vpc_per_region(&router).await;
    // Every set the store holds: the Regions' plus the seeded default VPC's, which is the
    // `us-east-1` row's own set.
    let all: Vec<String> = {
        let mut ids: Vec<String> = sets.iter().map(|(_, _, id)| id.clone()).collect();
        ids.sort();
        ids.dedup();
        ids
    };
    assert_eq!(
        described_ids("every set", &router, &[]).await,
        all,
        "a describe with no narrowing reports every option set the account holds"
    );

    let account = engine::emulator_account_id().to_string();
    let us_east_1 = sets
        .iter()
        .find(|(region, _, _)| *region == "us-east-1")
        .map(|(_, _, id)| id.clone())
        .expect("us-east-1 is one of the rows");
    let sa_east_1 = sets
        .iter()
        .find(|(region, _, _)| *region == "sa-east-1")
        .map(|(_, _, id)| id.clone())
        .expect("sa-east-1 is one of the rows");

    // (case, the filter and its values, the ids expected)
    let cases: Vec<FilterCase> = vec![
        // `dhcp-options-id`: "The ID of a DHCP options set."
        (
            "one id",
            vec![("dhcp-options-id", vec![us_east_1.clone()])],
            vec![us_east_1.clone()],
        ),
        (
            "two ids, joined with OR",
            vec![(
                "dhcp-options-id",
                vec![us_east_1.clone(), sa_east_1.clone()],
            )],
            {
                let mut both = vec![us_east_1.clone(), sa_east_1.clone()];
                both.sort();
                both
            },
        ),
        (
            "an id no set has",
            vec![("dhcp-options-id", vec!["dopt-0000000000000dead".into()])],
            Vec::new(),
        ),
        // `key`: "The key for one of the options (for example, domain-name)" — every
        // default set carries both keys, so either selects all of them.
        (
            "key domain-name",
            vec![(
                "key",
                vec![ec2::state::DHCP_OPTION_DOMAIN_NAME.to_string()],
            )],
            all.clone(),
        ),
        (
            "key domain-name-servers",
            vec![(
                "key",
                vec![ec2::state::DHCP_OPTION_DOMAIN_NAME_SERVERS.to_string()],
            )],
            all.clone(),
        ),
        (
            "a key no option has",
            vec![("key", vec!["ntp-servers".into()])],
            Vec::new(),
        ),
        // `value`: "The value for one of the options" — the domain differs per Region, the
        // resolver does not.
        (
            "value, the one Region with its own domain",
            vec![("value", vec!["ec2.internal".into()])],
            vec![us_east_1.clone()],
        ),
        (
            "value, a Region following the pattern",
            vec![("value", vec!["sa-east-1.compute.internal".into()])],
            vec![sa_east_1.clone()],
        ),
        (
            "value, the resolver every set names",
            vec![("value", vec![ec2::state::AMAZON_PROVIDED_DNS.to_string()])],
            all.clone(),
        ),
        // EC2's own `*` wildcard, matched the way every other filter value is.
        (
            "value, a wildcard over the domains",
            vec![("value", vec!["*.compute.internal".into()])],
            {
                let mut without_us_east_1 = all.clone();
                without_us_east_1.retain(|id| id != &us_east_1);
                without_us_east_1
            },
        ),
        // `owner-id`: "The ID of the Amazon Web Services account that owns the DHCP options
        // set."
        (
            "owner-id, the account",
            vec![("owner-id", vec![account.clone()])],
            all.clone(),
        ),
        (
            "owner-id, another account",
            vec![("owner-id", vec!["111122223333".into()])],
            Vec::new(),
        ),
        // `tag-key` and `tag:<key>`: nothing has tagged a set the service created, so both
        // select nothing rather than everything.
        (
            "tag-key",
            vec![("tag-key", vec!["Name".into()])],
            Vec::new(),
        ),
        (
            "tag:<key>",
            vec![("tag:Name", vec!["web".into()])],
            Vec::new(),
        ),
        // Two filters, joined with AND: one that holds for every set and one for a single
        // Region's.
        (
            "two filters",
            vec![
                ("owner-id", vec![account.clone()]),
                ("value", vec!["ec2.internal".into()]),
            ],
            vec![us_east_1.clone()],
        ),
        // The same two, where the second cannot hold: an empty set, not an error.
        (
            "two filters that cannot both hold",
            vec![
                ("owner-id", vec!["111122223333".into()]),
                ("value", vec!["ec2.internal".into()]),
            ],
            Vec::new(),
        ),
    ];

    for (case, entries, expected) in cases {
        let params = filter_params(&entries);
        let reported = described_ids(case, &router, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // `DhcpOptionsIds` narrows the same way, and the response keeps the store's id order
    // whichever order the request named them in.
    let mut both = vec![us_east_1.clone(), sa_east_1.clone()];
    both.sort();
    for (case, named) in [
        ("ids in store order", vec![&us_east_1, &sa_east_1]),
        ("ids reversed", vec![&sa_east_1, &us_east_1]),
    ] {
        let ids: Vec<&str> = named.iter().map(|id| id.as_str()).collect();
        let reported = described_ids(case, &router, &indexed("DhcpOptionsId", &ids)).await;
        assert_eq!(
            reported, both,
            "case {case:?}: expected the named sets in store order, got {reported:?}"
        );
    }
}

/// The rule: `DescribeDhcpOptions` pages its result, a `NextToken` appearing only while a
/// set follows the page and naming where the next one starts.
///
/// The rows are page sizes: the smallest the model admits, one in the middle, and one past
/// the number of sets the store holds.
#[tokio::test]
async fn describe_dhcp_options_pages_its_result() {
    let router = emulator();
    one_vpc_per_region(&router).await;
    let all = described_ids("every set", &router, &[]).await;
    assert!(
        all.len() >= 6,
        "the table needs more sets than the smallest page size, got {all:?}"
    );

    for limit in [5usize, 6, 1000] {
        let case = format!("MaxResults={limit}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![param("MaxResults", &limit.to_string())];
            if let Some(next) = token.as_deref() {
                params.push(param("NextToken", next));
            }
            let body = ok(&case, &router, SEEDED_REGION, "DescribeDhcpOptions", &params).await;
            let page = elements(&body, "dhcpOptionsId");
            assert!(
                page.len() <= limit,
                "case {case:?}: a page holds at most {limit} sets, got {page:?}"
            );
            seen.extend(page);
            match element(&body, "nextToken") {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            seen, all,
            "case {case:?}: paging must report every set exactly once, got {seen:?}"
        );
    }
}

/// The rule: a request this operation cannot answer is refused with EC2's own code for what
/// is wrong with it, and a dry run reports the permission outcome without reading.
#[tokio::test]
async fn describe_dhcp_options_refuses_what_it_cannot_answer() {
    let router = emulator();
    let held = described_ids("before the refusals", &router, &[]).await;

    // (case, the request's form fields, the HTTP status, the EC2 error code)
    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "an id no option set has",
            indexed("DhcpOptionsId", &["dopt-0000000000000dead"]),
            400,
            ec2::state::INVALID_DHCP_OPTION_ID_NOT_FOUND,
        ),
        (
            "a known id beside an unknown one",
            indexed(
                "DhcpOptionsId",
                &[
                    held.first().map(String::as_str).unwrap_or_default(),
                    "dopt-0000000000000dead",
                ],
            ),
            400,
            ec2::state::INVALID_DHCP_OPTION_ID_NOT_FOUND,
        ),
        (
            "an undocumented filter",
            filters(&[("domain-name", &["ec2.internal"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a filter name in the wrong case",
            filters(&[("Key", &["domain-name"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag filter with no key",
            filters(&[("tag:", &["web"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a pagination token this service did not issue",
            vec![param("NextToken", "not-a-token")],
            400,
            "InvalidPaginationToken",
        ),
        // "If you have the required permissions, the error response is DryRunOperation"
        // (`DescribeDhcpOptionsRequest$DryRun`), which EC2 answers with HTTP 412.
        (
            "a dry run",
            vec![param("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
        // The request-controlled factors of the matching cost, each one past the ceiling
        // `crate::state` publishes for every filtered describe of this service.
        (
            "one filter past the maximum",
            {
                let names: Vec<String> = (0..=ec2::state::MAX_FILTERS)
                    .map(|index| format!("tag:key{index}"))
                    .collect();
                let entries: Vec<(&str, &[&str])> = names
                    .iter()
                    .map(|name| (name.as_str(), &["anything"][..]))
                    .collect();
                filters(&entries)
            },
            400,
            "InvalidParameterValue",
        ),
        (
            "one filter value past the maximum",
            {
                let values: Vec<String> = (0..=ec2::state::MAX_FILTER_VALUES)
                    .map(|index| format!("value{index}"))
                    .collect();
                let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
                filters(&[("key", borrowed.as_slice())])
            },
            400,
            "InvalidParameterValue",
        ),
        (
            "a filter value one byte past the maximum",
            {
                let oversize = "v".repeat(ec2::state::MAX_FILTER_VALUE_BYTES + 1);
                filters(&[("value", &[oversize.as_str()])])
            },
            400,
            "InvalidParameterValue",
        ),
        (
            "one id past the maximum",
            {
                let ids: Vec<String> = (0..=ec2::state::MAX_RESOURCE_IDS)
                    .map(|index| format!("dopt-{index:017x}"))
                    .collect();
                let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
                indexed("DhcpOptionsId", &borrowed)
            },
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, params, status_expected, expected) in cases {
        let (status, body) =
            super::ec2_query::call(&router, SEEDED_REGION, "DescribeDhcpOptions", &params).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }

    let after = described_ids("after the refusals", &router, &[]).await;
    assert_eq!(
        after, held,
        "a refused describe must change nothing, got {after:?} where {held:?} was there"
    );
}
