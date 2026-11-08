// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Dedicated Host slice this change implements: `AllocateHosts`
//! and `DescribeMacHosts`, driven over the wire the service actually speaks — `ec2Query`,
//! through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `AllocateHosts` MINTS `Quantity` HOSTS FROM THE REQUEST and `DescribeMacHosts`
//!    REPORTS THE STORE: every host whose instance family runs macOS is reported, with the
//!    macOS release lines that family's hardware supports, and no host of any other family
//!    is;
//! 2. an id `HostIds` names has to name an EC2 Mac Dedicated Host of the account, or the
//!    request is refused — malformed before unknown;
//! 3. a filter selects on the HOST the projection came from (its zone, the single instance
//!    type it supports), values joined with `OR` and filters with `AND`, and a filter this
//!    operation does not document is refused;
//! 4. the answer is paged in `HostId` order, and a token this service did not mint is
//!    refused;
//! 5. one `ClientToken` names one allocation.
//!
//! Each table row is a distinct input along what the rule generalises over — which Mac
//! family, whether the host was allocated by instance type or by instance family, which
//! zone, which filter, how many hosts, which page size — so no row is the specification.

use service_com_amazonaws_ec2_amazonec2::state::{
    availability_zones_in_region, mac_os_supported_versions, MAC_OS_SUPPORTED_VERSIONS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for; the zone names and ids the assertions use are
/// derived from it.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn allocate(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "AllocateHosts", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeMacHosts", params).await
}

/// The name of the `index`th Availability Zone of the signed Region.
fn zone_name(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_name.clone()
}

/// The id of the `index`th Availability Zone of the signed Region.
fn zone_id(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_id.clone()
}

/// A `HostId` is `h-` followed by 8 or 17 lowercase hex digits.
fn is_host_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("h-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17) && suffix.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The ids in an `AllocateHosts` answer: the `<item>`s of its `hostIdSet`.
fn allocated_ids(body: &str) -> Vec<String> {
    element(body, "hostIdSet")
        .map(|set| elements(&set, "item"))
        .unwrap_or_default()
}

/// The ids a `DescribeMacHosts` answer reports, in the order it reported them.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "hostId")
}

/// The macOS release lines the FIRST reported Mac host carries.
fn reported_versions(body: &str) -> Vec<String> {
    element(body, "macOSLatestSupportedVersionSet")
        .map(|set| elements(&set, "item"))
        .unwrap_or_default()
}

/// Allocate `quantity` hosts configured by `(member, value)` — `InstanceType` or
/// `InstanceFamily` — into the `zone`th zone of the Region, plus whatever `extra`
/// parameters the case needs.
async fn allocate_hosts(
    router: &MultiServiceRouter,
    configuration: (&str, &str),
    quantity: i32,
    zone: usize,
    extra: Params,
) -> (u16, String) {
    let (member, value) = configuration;
    let mut params = vec![
        p(member, value),
        p("AvailabilityZone", zone_name(zone)),
        p("Quantity", quantity.to_string()),
    ];
    params.extend(extra);
    allocate(router, &params).await
}

/// Allocate one host and answer the id it was given, failing the case when the allocation
/// was refused.
async fn allocate_one(
    router: &MultiServiceRouter,
    case: &str,
    configuration: (&str, &str),
    zone: usize,
) -> String {
    let (status, body) = allocate_hosts(router, configuration, 1, zone, Vec::new()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AllocateHosts, got {status} with body {body}"
    );
    let ids = allocated_ids(&body);
    assert_eq!(
        ids.len(),
        1,
        "case {case:?}: expected one allocated host id, got {ids:?}"
    );
    ids.into_iter().next().unwrap_or_default()
}

/// The ids `DescribeMacHosts` reports for `params`, asserting it answered at all.
async fn described_ids(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeMacHosts, got {status} with body {body}"
    );
    reported_ids(&body)
}

/// RULE 1 over the Mac catalog: every Mac instance family the model declares, allocated
/// BOTH ways a request can configure a host — by its single instance type and by the whole
/// family — round-trips through `DescribeMacHosts` with the macOS release lines that
/// family's hardware supports.
///
/// The expected versions come from the service's own catalog rather than a literal here, so
/// the test cannot drift from the source the catalog cites; the rows span every entry of it,
/// including the families the issue never mentioned.
#[tokio::test]
async fn every_mac_family_round_trips_with_its_macos_release_lines() {
    for (family, expected_versions) in MAC_OS_SUPPORTED_VERSIONS {
        let instance_type = format!("{family}.metal");
        for (case, configuration) in [
            (
                format!("{family} by instance type"),
                ("InstanceType", instance_type.as_str()),
            ),
            (
                format!("{family} by instance family"),
                ("InstanceFamily", family),
            ),
        ] {
            let router = emulator();
            let host_id = allocate_one(&router, &case, configuration, 0).await;
            assert!(
                is_host_id(&host_id),
                "case {case:?}: expected an `h-` host id, got {host_id:?}"
            );

            let (status, body) = describe(&router, &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeMacHosts, got {status} \
                 with body {body}"
            );
            assert_eq!(
                reported_ids(&body),
                vec![host_id.clone()],
                "case {case:?}: expected DescribeMacHosts to report the allocated host, \
                 got body {body}"
            );
            let versions = reported_versions(&body);
            assert_eq!(
                versions,
                expected_versions
                    .iter()
                    .map(|version| (*version).to_string())
                    .collect::<Vec<String>>(),
                "case {case:?}: expected the catalog's macOS release lines, got {versions:?}"
            );
            assert_eq!(
                mac_os_supported_versions(family),
                Some(expected_versions),
                "case {case:?}: the catalog must answer for the family it lists"
            );
        }
    }
}

/// RULE 1, the other half: a Dedicated Host of a family that does not run macOS is no EC2
/// Mac Dedicated Host, so `DescribeMacHosts` reports none of them however many are stored,
/// and reports the Mac host standing beside them.
#[tokio::test]
async fn hosts_of_other_families_are_not_reported() {
    let router = emulator();
    // Rows differ in family, in generation and architecture, and in whether the host was
    // allocated by instance type or by whole family.
    for (case, configuration) in [
        ("x86 by type", ("InstanceType", "m5.large")),
        ("x86 by family", ("InstanceFamily", "m5")),
        ("arm by type", ("InstanceType", "c7g.metal")),
        ("burstable by family", ("InstanceFamily", "t3")),
        ("metal x86 by type", ("InstanceType", "c5.metal")),
    ] {
        let host_id = allocate_one(&router, case, configuration, 0).await;
        let reported = described_ids(&router, case, &[]).await;
        assert!(
            !reported.contains(&host_id),
            "case {case:?}: expected DescribeMacHosts not to report host {host_id:?}, \
             got {reported:?}"
        );
    }

    let mac_host = allocate_one(&router, "the one Mac host", ("InstanceType", "mac2.metal"), 0).await;
    let reported = described_ids(&router, "the one Mac host", &[]).await;
    assert_eq!(
        reported,
        vec![mac_host.clone()],
        "case {:?}: expected only the Mac host to be reported, got {reported:?}",
        "the one Mac host"
    );
}

/// RULE 2: `HostIds` narrows the answer to the Mac hosts it names, and an id that names no
/// Mac host of the account is refused — malformed before unknown — rather than reported as
/// an absent row.
#[tokio::test]
async fn host_ids_narrow_and_unresolvable_ids_are_refused() {
    let router = emulator();
    let mac1 = allocate_one(&router, "mac1 host", ("InstanceType", "mac1.metal"), 0).await;
    let mac2 = allocate_one(&router, "mac2 host", ("InstanceType", "mac2.metal"), 0).await;
    let family_host = allocate_one(&router, "family host", ("InstanceFamily", "mac2-m2"), 0).await;
    let not_a_mac = allocate_one(&router, "non-Mac host", ("InstanceType", "m5.large"), 0).await;

    // Rows: one id, several ids, every id, and each way an id can fail to name a Mac host.
    let selections: Vec<(&str, Params, Vec<String>)> = vec![
        (
            "one id",
            indexed("HostId", &[mac2.as_str()]),
            vec![mac2.clone()],
        ),
        (
            "two ids",
            indexed("HostId", &[family_host.as_str(), mac1.as_str()]),
            {
                let mut both = vec![mac1.clone(), family_host.clone()];
                both.sort();
                both
            },
        ),
        (
            "every Mac host, no ids given",
            Vec::new(),
            {
                let mut all = vec![mac1.clone(), mac2.clone(), family_host.clone()];
                all.sort();
                all
            },
        ),
    ];
    for (case, params, expected) in selections {
        let reported = described_ids(&router, case, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the named Mac hosts, got {reported:?}"
        );
    }

    let refusals: Vec<(&str, Params, &str)> = vec![
        (
            "a host of the account that is not a Mac host",
            indexed("HostId", &[not_a_mac.as_str()]),
            "InvalidHostID.NotFound",
        ),
        (
            "a well-formed id no host has",
            indexed("HostId", &["h-0123456789abcdef0"]),
            "InvalidHostID.NotFound",
        ),
        (
            "a short well-formed id no host has",
            indexed("HostId", &["h-0123abcd"]),
            "InvalidHostID.NotFound",
        ),
        (
            "an id of another resource kind",
            indexed("HostId", &["vol-0123456789abcdef0"]),
            "InvalidHostID.Malformed",
        ),
        (
            "an id with the wrong number of digits",
            indexed("HostId", &["h-123"]),
            "InvalidHostID.Malformed",
        ),
        (
            "an id with a non-hex digit",
            indexed("HostId", &["h-0123456789abcdefz"]),
            "InvalidHostID.Malformed",
        ),
        (
            "one good id beside one bad one",
            indexed("HostId", &[mac1.as_str(), "h-not-an-id"]),
            "InvalidHostID.Malformed",
        ),
    ];
    for (case, params, expected_code) in refusals {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DescribeMacHosts, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code}, got {code} with body {body}"
        );
    }
}

/// RULE 3: a filter selects on the host the reported `MacHost` was projected from, with
/// EC2's own value semantics — values joined with `OR`, filters with `AND`, wildcards
/// honoured — and a filter name this operation does not document is refused.
#[tokio::test]
async fn filters_select_on_the_host() {
    let router = emulator();
    let first_zone = allocate_one(&router, "mac1 in zone 0", ("InstanceType", "mac1.metal"), 0).await;
    let second_zone = allocate_one(&router, "mac2 in zone 1", ("InstanceType", "mac2.metal"), 1).await;
    let family_host =
        allocate_one(&router, "mac2-m2pro family host", ("InstanceFamily", "mac2-m2pro"), 0).await;

    let mut every_host = vec![
        first_zone.clone(),
        second_zone.clone(),
        family_host.clone(),
    ];
    every_host.sort();
    let mut zone_zero = vec![first_zone.clone(), family_host.clone()];
    zone_zero.sort();

    // Rows differ in which attribute is filtered on, in how many values the filter holds,
    // in whether the value is literal or a wildcard, and in how many filters are combined.
    let cases: Vec<(&str, Params, Vec<String>)> = vec![
        (
            "availability-zone of the first zone",
            filters(&[("availability-zone", &[zone_name(0).as_str()])]),
            zone_zero.clone(),
        ),
        (
            "availability-zone of the second zone",
            filters(&[("availability-zone", &[zone_name(1).as_str()])]),
            vec![second_zone.clone()],
        ),
        (
            "availability-zone with both zones as values",
            filters(&[(
                "availability-zone",
                &[zone_name(0).as_str(), zone_name(1).as_str()],
            )]),
            every_host.clone(),
        ),
        (
            "availability-zone as a wildcard over the Region",
            filters(&[("availability-zone", &[format!("{REGION}*").as_str()])]),
            every_host.clone(),
        ),
        (
            "availability-zone of a zone no host is in",
            filters(&[("availability-zone", &[zone_name(2).as_str()])]),
            Vec::new(),
        ),
        (
            "instance-type of the one host configured for it",
            filters(&[("instance-type", &["mac1.metal"])]),
            vec![first_zone.clone()],
        ),
        (
            "instance-type as a wildcard over the mac2 types",
            filters(&[("instance-type", &["mac2*"])]),
            vec![second_zone.clone()],
        ),
        (
            "instance-type of the family host's own family, which supports no single type",
            filters(&[("instance-type", &["mac2-m2pro.metal"])]),
            Vec::new(),
        ),
        (
            "instance-type with no values at all",
            filters(&[("instance-type", &[])]),
            Vec::new(),
        ),
        (
            "two filters, joined with AND",
            filters(&[
                ("availability-zone", &[zone_name(0).as_str()]),
                ("instance-type", &["mac1.metal"]),
            ]),
            vec![first_zone.clone()],
        ),
        (
            "two filters whose AND selects nothing",
            filters(&[
                ("availability-zone", &[zone_name(1).as_str()]),
                ("instance-type", &["mac1.metal"]),
            ]),
            Vec::new(),
        ),
    ];
    for (case, params, expected) in cases {
        let reported = described_ids(&router, case, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the hosts the filters select, got {reported:?}"
        );
    }

    // A filter name this operation does not document is refused, whether or not a host
    // would have matched it: `host-id` and `tag:` are filters of OTHER describes.
    for (case, name) in [
        ("an undocumented filter name", "host-id"),
        ("a tag filter this operation does not document", "tag:Name"),
        ("a filter of another resource", "instance-id"),
        ("a misspelling of a documented filter", "availability-zones"),
    ] {
        let params = filters(&[(name, &["anything"])]);
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DescribeMacHosts, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "InvalidRequest",
            "case {case:?}: expected error code InvalidRequest, got {code} \
             with body {body}"
        );
        // The message is the service's own constant for this refusal: it names neither the
        // filter nor a value, so every row of this table reads the same.
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert_eq!(
            message, "Invalid filter",
            "case {case:?}: expected the message 'Invalid filter', got {message:?} \
             with body {body}"
        );
    }
}

/// RULE 4: the answer is paged in `HostId` order — a page carries at most `MaxResults`
/// hosts and a token only when another host follows — and a token this service did not mint
/// is refused rather than read as a position.
#[tokio::test]
async fn pages_in_host_id_order() {
    let router = emulator();
    // Seven hosts, allocated in one call and in several, so the page boundary falls inside
    // one allocation's worth of ids.
    let (status, body) = allocate_hosts(&router, ("InstanceType", "mac2.metal"), 4, 0, Vec::new()).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from AllocateHosts, got {status} with body {body}",
        "a batch of four"
    );
    let mut every_host = allocated_ids(&body);
    assert_eq!(
        every_host.len(),
        4,
        "case {:?}: expected four allocated host ids, got {every_host:?}",
        "a batch of four"
    );
    for nth in 0..3 {
        let case = format!("single allocation {nth}");
        every_host.push(allocate_one(&router, &case, ("InstanceFamily", "mac1"), nth % 3).await);
    }
    every_host.sort();

    let page_size = 5;
    let (status, body) = describe(&router, &[p("MaxResults", page_size.to_string())]).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from DescribeMacHosts, got {status} with body {body}",
        "first page"
    );
    let first_page = reported_ids(&body);
    assert_eq!(
        first_page,
        every_host[..page_size],
        "case {:?}: expected the first {page_size} hosts in id order, got {first_page:?}",
        "first page"
    );
    let token = element(&body, "nextToken").unwrap_or_default();
    assert!(
        !token.is_empty(),
        "case {:?}: expected a nextToken while hosts remain, got body {body}",
        "first page"
    );

    let (status, body) = describe(
        &router,
        &[p("MaxResults", page_size.to_string()), p("NextToken", token)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from DescribeMacHosts, got {status} with body {body}",
        "second page"
    );
    let second_page = reported_ids(&body);
    assert_eq!(
        second_page,
        every_host[page_size..],
        "case {:?}: expected the remaining hosts in id order, got {second_page:?}",
        "second page"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "case {:?}: expected no nextToken on the last page, got body {body}",
        "second page"
    );

    // The model's own `@range` on `MaxResults` (5..500, `DescribeMacHostsRequestMaxResults`)
    // is the published page-size bound: each published bound is served, and one past either
    // end is refused.
    for (case, max_results, expected_status) in [
        ("the smallest published page size", 5, 200),
        ("the largest published page size", 500, 200),
        ("one below the smallest", 4, 400),
        ("one above the largest", 501, 400),
    ] {
        let (status, body) = describe(&router, &[p("MaxResults", max_results.to_string())]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for MaxResults={max_results}, \
             got {status} with body {body}"
        );
    }

    // A token this service did not mint names no position, so it is refused.
    for (case, token) in [
        ("a token a caller assembled", "h-0123456789abcdef0"),
        ("an empty-looking token", "ec2-1"),
        ("a token with a broken checksum", "ec2-1:0000:aaaa"),
    ] {
        let (status, body) = describe(&router, &[p("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DescribeMacHosts, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "InvalidPaginationToken",
            "case {case:?}: expected error code InvalidPaginationToken, got {code} \
             with body {body}"
        );
    }
}

/// RULE 5: "Unique, case-sensitive identifier that you provide to ensure the idempotency of
/// the request" (`AllocateHostsRequest$ClientToken`) — a repeat of one request reports the
/// hosts that request allocated and allocates none.
#[tokio::test]
async fn one_client_token_names_one_allocation() {
    // Rows differ in how many hosts the one call allocates.
    for quantity in [1, 3] {
        let case = format!("a batch of {quantity}");
        let router = emulator();
        let token = vec![p("ClientToken", format!("idempotent-{quantity}"))];
        let (status, body) = allocate_hosts(
            &router,
            ("InstanceType", "mac2-m1ultra.metal"),
            quantity,
            0,
            token.clone(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from AllocateHosts, got {status} with body {body}"
        );
        let first = allocated_ids(&body);
        assert_eq!(
            first.len(),
            usize::try_from(quantity).unwrap_or_default(),
            "case {case:?}: expected {quantity} allocated host ids, got {first:?}"
        );

        let (status, body) = allocate_hosts(
            &router,
            ("InstanceType", "mac2-m1ultra.metal"),
            quantity,
            0,
            token,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the replayed AllocateHosts, got {status} \
             with body {body}"
        );
        let replayed = allocated_ids(&body);
        assert_eq!(
            replayed, first,
            "case {case:?}: expected the replay to report the hosts the first call \
             allocated, got {replayed:?}"
        );

        let mut expected = first.clone();
        expected.sort();
        let reported = described_ids(&router, &case, &[]).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the store to hold only the one allocation's hosts, \
             got {reported:?}"
        );
    }
}

/// A request `AllocateHosts` cannot satisfy is refused with EC2's own code, and nothing is
/// stored: each row is one way a request can be wrong, and `DescribeMacHosts` reports no
/// host after any of them.
#[tokio::test]
async fn allocate_hosts_refuses_a_request_it_cannot_satisfy() {
    let zone = zone_name(0);
    let rows: Vec<(&str, Params, &str)> = vec![
        (
            // The two members ask for two DIFFERENT hosts — one that supports a single
            // instance type, one that supports a whole family — so no pair of values makes
            // the request answerable: the REQUEST is invalid, which is `InvalidRequest`,
            // where the zone-name/zone-id row below is a pair that agrees for some values
            // and so is a parameter COMBINATION.
            "both instanceType and instanceFamily",
            vec![
                p("InstanceType", "mac2.metal"),
                p("InstanceFamily", "mac2"),
                p("AvailabilityZone", zone.clone()),
                p("Quantity", "1"),
            ],
            "InvalidRequest",
        ),
        (
            "neither instanceType nor instanceFamily",
            vec![p("AvailabilityZone", zone.clone()), p("Quantity", "1")],
            "MissingParameter",
        ),
        (
            // Hardware EC2 does not offer makes the host CONFIGURATION impossible rather
            // than inviting a retry with another value for the parameter, so these three
            // rows carry `InvalidHostConfiguration`.
            "an instance type EC2 does not offer",
            vec![
                p("InstanceType", "mac9.metal"),
                p("AvailabilityZone", zone.clone()),
                p("Quantity", "1"),
            ],
            "InvalidHostConfiguration",
        ),
        (
            "an instance family EC2 does not offer",
            vec![
                p("InstanceFamily", "mac9"),
                p("AvailabilityZone", zone.clone()),
                p("Quantity", "1"),
            ],
            "InvalidHostConfiguration",
        ),
        (
            "an instance type given where a family belongs",
            vec![
                p("InstanceFamily", "mac2.metal"),
                p("AvailabilityZone", zone.clone()),
                p("Quantity", "1"),
            ],
            "InvalidHostConfiguration",
        ),
        (
            "no zone at all",
            vec![p("InstanceType", "mac2.metal"), p("Quantity", "1")],
            "MissingParameter",
        ),
        (
            "both a zone name and a zone id",
            vec![
                p("InstanceType", "mac2.metal"),
                p("AvailabilityZone", zone.clone()),
                p("AvailabilityZoneId", zone_id(0)),
                p("Quantity", "1"),
            ],
            "InvalidParameterCombination",
        ),
        (
            // A value that names no zone of the signed Region leaves the request with
            // nowhere to put the host, so the REQUEST is what is invalid — the class the
            // recorded refusal of these two members carries, where
            // `InvalidParameterValue` invites a retry with another value for the parameter.
            "a zone name the Region does not have",
            vec![
                p("InstanceType", "mac2.metal"),
                p("AvailabilityZone", "eu-west-1a"),
                p("Quantity", "1"),
            ],
            "InvalidRequest",
        ),
        (
            "a zone id the Region does not have",
            vec![
                p("InstanceType", "mac2.metal"),
                p("AvailabilityZoneId", "euw1-az1"),
                p("Quantity", "1"),
            ],
            "InvalidRequest",
        ),
        (
            "no quantity",
            vec![
                p("InstanceType", "mac2.metal"),
                p("AvailabilityZone", zone.clone()),
            ],
            "MissingParameter",
        ),
        (
            "a quantity of zero",
            vec![
                p("InstanceType", "mac2.metal"),
                p("AvailabilityZone", zone.clone()),
                p("Quantity", "0"),
            ],
            "InvalidParameterValue",
        ),
        (
            "a negative quantity",
            vec![
                p("InstanceType", "mac2.metal"),
                p("AvailabilityZone", zone.clone()),
                p("Quantity", "-2"),
            ],
            "InvalidParameterValue",
        ),
        (
            "an Outpost the account does not have",
            vec![
                p("InstanceType", "mac2.metal"),
                p("AvailabilityZone", zone.clone()),
                p("Quantity", "1"),
                p(
                    "OutpostArn",
                    "arn:aws:outposts:us-west-2:111122223333:outpost/op-0123456789abcdef0",
                ),
            ],
            "InvalidParameterValue",
        ),
        (
            "an Outpost hardware asset",
            vec![
                p("InstanceType", "mac2.metal"),
                p("AvailabilityZone", zone.clone()),
                p("Quantity", "1"),
                p("AssetId.1", "asset-0123"),
            ],
            "InvalidParameterValue",
        ),
        (
            "tags for a resource this call does not create",
            vec![
                p("InstanceType", "mac2.metal"),
                p("AvailabilityZone", zone.clone()),
                p("Quantity", "1"),
                p("TagSpecification.1.ResourceType", "volume"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "nope"),
            ],
            "InvalidParameterValue",
        ),
        (
            "a tag key in the reserved namespace",
            vec![
                p("InstanceType", "mac2.metal"),
                p("AvailabilityZone", zone.clone()),
                p("Quantity", "1"),
                p("TagSpecification.1.ResourceType", "dedicated-host"),
                p("TagSpecification.1.Tag.1.Key", "aws:owner"),
                p("TagSpecification.1.Tag.1.Value", "me"),
            ],
            "InvalidParameterValue",
        ),
    ];
    for (case, params, expected_code) in rows {
        let router = emulator();
        let (status, body) = allocate(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from AllocateHosts, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code}, got {code} with body {body}"
        );
        let reported = described_ids(&router, case, &[]).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: expected a refused allocation to store nothing, got {reported:?}"
        );
    }
}

/// A host the request tags carries those tags, and `AllocateHosts` accepts the one resource
/// type it may tag: the host round-trips as a Mac host either way, so tagging neither
/// changes nor hides it.
#[tokio::test]
async fn tags_do_not_change_which_hosts_are_mac_hosts() {
    // Rows differ in how many tags the request carries.
    for (case, tags) in [
        ("no tags", Vec::new()),
        ("one tag", vec![("Name", "build-fleet")]),
        (
            "several tags",
            vec![("Name", "build-fleet"), ("Team", "ios"), ("Tier", "ci")],
        ),
    ] {
        let router = emulator();
        let mut extra = Vec::new();
        if !tags.is_empty() {
            extra.push(p("TagSpecification.1.ResourceType", "dedicated-host"));
            for (index, (key, value)) in tags.iter().enumerate() {
                extra.push(p(&format!("TagSpecification.1.Tag.{}.Key", index + 1), *key));
                extra.push(p(
                    &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                    *value,
                ));
            }
        }
        let (status, body) =
            allocate_hosts(&router, ("InstanceType", "mac2-m2.metal"), 1, 0, extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from AllocateHosts, got {status} with body {body}"
        );
        let ids = allocated_ids(&body);
        let reported = described_ids(&router, case, &[]).await;
        assert_eq!(
            reported, ids,
            "case {case:?}: expected the tagged host to be reported, got {reported:?}"
        );
    }
}
