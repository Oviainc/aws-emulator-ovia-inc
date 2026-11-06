// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetIpamRouteProtectionFindings`, driven over `ec2Query` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * The report is ONE FINDING PER ROUTE ANNOUNCEMENT the IPAM manages: a BYOIP address range
//!   in one of that IPAM's pools, announced by each ASN the range still holds an association
//!   with, with every member read off the stored range and the stored pool — so a range in
//!   another IPAM's pool, or in no IPAM pool at all, is no route of this IPAM.
//! * Route origin validation, as `IpamRouteProtectionFinding$RpkiStatus` states it: `unknown`
//!   when no published ROA covers the prefix, `valid` when a covering ROA authorizes the
//!   originating ASN for a prefix no longer than the maximum length it authorizes, and
//!   `invalid` when a ROA covers the prefix but authorizes neither — the ROAs being the
//!   routing policy registrations the IPAM's internet registry associations hold.
//! * The report is narrowed by `Filters`, which refuse a name the operation does not resolve,
//!   and paged by `MaxResults`/`NextToken` in (prefix, ASN) order with a token that only
//!   resumes the IPAM it was minted for.
//! * An IPAM id outside the form this service issues is refused with
//!   `InvalidIpamId.Malformed` before one that names no IPAM is refused with
//!   `InvalidIpamId.NotFound`; `DryRun` answers the permission question and reads nothing; and
//!   reading the report changes nothing a later caller sees.
//!
//! Each table row is a distinct input along what those rules generalise over: the prefix and
//! its length, how many ASNs announce it, whether an association is one the range still holds,
//! which `ByoipCidrState` the range sits in, how its advertisement method is spelled, whether
//! it is still in a public IPv4 pool, which pool of the IPAM manages it, and whether the ROA
//! covering it is for the prefix itself or for a shorter one that permits more specific
//! announcements.

use ec2::ops::types::{
    AsnAssociation, ByoipCidr, IpamRoutingPolicyRegistration, IpamInternetRegistryAssociation,
    BYOIP_CIDR_STATE_VALUES, IPAM_BYOIP_ADVERTISEMENT_TYPE_VALUES, IPAM_BYOIP_CIDR_STATE_VALUES,
    IPAM_RPKI_STATUS_VALUES,
};
use ec2::state::{
    ByoipCidrRecord, ComAmazonawsEc2Amazonec2State as Ec2State,
    IpamRoutingPolicyRegistrationRecord, ASN_ASSOCIATION_STATE_ASSOCIATED,
    ASN_ASSOCIATION_STATE_DISASSOCIATED, BYOIP_CIDR_STATE_ADVERTISED,
    BYOIP_CIDR_STATE_PENDING_ADVERTISING, BYOIP_CIDR_STATE_PENDING_PROVISION,
    BYOIP_CIDR_STATE_PROVISIONED, BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE,
    DRY_RUN_OPERATION, INVALID_IPAM_ID_MALFORMED, INVALID_IPAM_ID_NOT_FOUND,
    INVALID_PAGINATION_TOKEN, INVALID_PARAMETER_VALUE,
    IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, filters, unescaped};
use super::wire::{child_text, document, set_items};

/// The Region every call here is signed for, which is therefore each IPAM's home Region.
const REGION: &str = "us-east-1";

/// The Region one of the IPAM's pools allocates in, so a finding's `ResourceRegion` is
/// asserted both for a pool that names a locale and for one that names none.
const POOL_LOCALE: &str = "us-west-2";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The address family the pools here hold, as `AddressFamily` spells it.
const FAMILY_IPV4: &str = "ipv4";

/// The Regional Internet Registry the seeded association is with: the modelled `Rir`
/// catalog's `arin` member (`ec2::ops::types::RIR_VALUES`).
const RIR_ARIN: &str = "arin";

/// The `IpamInternetRegistryAssociationState` the seeded association sits in, which
/// [`ec2::ops::types::IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES`] is asserted to carry
/// below.
const ASSOCIATION_STATE_ENABLE_COMPLETE: &str = "enable-complete";

/// One `(name, value)` request member.
fn p(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// What a suite drives: the router, the engine whose store holds the seeded ranges, and the
/// two IPAMs and three pools the rows are spread over.
struct Fixture {
    router: MultiServiceRouter,
    engine: std::sync::Arc<engine::InMemoryEngine>,
    /// The IPAM whose findings every assertion below is about.
    ipam_id: String,
    /// A pool of that IPAM created with a `Locale`.
    located_pool: String,
    /// A pool of that IPAM created with no `Locale` at all.
    unlocated_pool: String,
    /// Another IPAM of the same account, whose own pool holds a range of its own.
    other_ipam_id: String,
    /// That other IPAM's pool.
    other_pool: String,
}

/// Create one IPAM over the wire and answer its id and the id of its default public scope.
async fn create_ipam(router: &MultiServiceRouter) -> (String, String) {
    let (status, body) = invoke(router, "CreateIpam", &[]).await;
    assert_eq!(
        status, 200,
        "creating an IPAM: expected HTTP 200, got {status} with body {body}"
    );
    let ipam_id = element(&body, "ipamId")
        .unwrap_or_else(|| panic!("the created IPAM reports an id, got {body}"));
    let public_scope = element(&body, "publicDefaultScopeId").unwrap_or_else(|| {
        panic!("the created IPAM reports its default public scope, got {body}")
    });
    (ipam_id, public_scope)
}

/// Create one IPv4 pool in `scope_id`, with `locale` when the row names one, and answer the
/// `IpamPoolId` the service minted.
async fn create_pool(
    router: &MultiServiceRouter,
    scope_id: &str,
    locale: Option<&str>,
) -> String {
    let mut params = vec![p("IpamScopeId", scope_id), p("AddressFamily", FAMILY_IPV4)];
    if let Some(locale) = locale {
        params.push(p("Locale", locale));
    }
    let (status, body) = invoke(router, "CreateIpamPool", &params).await;
    assert_eq!(
        status, 200,
        "creating a pool in {scope_id} with locale {locale:?}: expected HTTP 200, got {status} \
         with body {body}"
    );
    element(&body, "ipamPoolId")
        .unwrap_or_else(|| panic!("the created pool reports an id, got {body}"))
}

/// Seed one BYOIP address range of the account, managed by `ipam_pool_id` when the row names
/// one.
fn seed_route(engine: &engine::InMemoryEngine, route: &SeededRoute, ipam_pool_id: Option<&str>) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let stored = ByoipCidrRecord {
        byoip_cidr: ByoipCidr {
            cidr: Some(route.cidr.to_string()),
            state: Some(route.state.to_string()),
            advertisement_type: route.advertisement_type.map(str::to_string),
            network_border_group: Some(REGION.to_string()),
            pool_id: route.pool_id.map(str::to_string),
            ipam_pool_id: ipam_pool_id.map(str::to_string),
            asn_associations: Some(
                route
                    .asn_associations
                    .iter()
                    .map(|(asn, state)| AsnAssociation {
                        asn: Some((*asn).to_string()),
                        cidr: Some(route.cidr.to_string()),
                        state: Some((*state).to_string()),
                        status_message: None,
                    })
                    .collect(),
            ),
            description: None,
            status_message: None,
        },
        released_at: None,
        pending: None,
    };
    assert!(
        guard.insert_byoip_cidr(stored),
        "the store holds the seeded address range {}",
        route.cidr
    );
}

/// Seed one internet registry association of `ipam_id` and answer its id.
fn seed_association(engine: &engine::InMemoryEngine, ipam_id: &str) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .insert_ipam_internet_registry_association(IpamInternetRegistryAssociation {
            ipam_id: Some(ipam_id.to_string()),
            ipam_region: Some(REGION.to_string()),
            rir: Some(RIR_ARIN.to_string()),
            organization_handle: Some("EXAMPLE-ORG".to_string()),
            state: Some(ASSOCIATION_STATE_ENABLE_COMPLETE.to_string()),
            ..Default::default()
        })
        .expect("the store holds the seeded association")
}

/// Seed one routing policy registration — the ROA this report validates against — under
/// `association_id`, in service.
fn seed_roa(engine: &engine::InMemoryEngine, association_id: &str, roa: &SeededRoa) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .insert_ipam_routing_policy_registration(
            association_id,
            IpamRoutingPolicyRegistrationRecord::in_state(
                IpamRoutingPolicyRegistration {
                    cidr: Some(roa.prefix.to_string()),
                    asns: Some(roa.asns.iter().map(|asn| (*asn).to_string()).collect()),
                    permit_more_specific_announcements: Some(roa.max_length.is_some()),
                    max_length: roa.max_length,
                    description: None,
                    latest_delta_id: None,
                    state: None,
                },
                IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
            ),
        )
        .expect("the store holds the seeded routing policy registration");
}

/// One BYOIP address range the store is seeded with.
struct SeededRoute {
    /// The prefix the account holds, in canonical form.
    cidr: &'static str,
    /// The `ByoipCidrState` it sits in.
    state: &'static str,
    /// How `ByoipCidr$AdvertisementType` spells its advertisement method, when it has one.
    advertisement_type: Option<&'static str>,
    /// The public IPv4 pool it is in, when the store holds it in one.
    pool_id: Option<&'static str>,
    /// Its ASN associations, each with the `AsnAssociationState` it is in.
    asn_associations: &'static [(&'static str, &'static str)],
}

/// One ROA the IPAM has published.
struct SeededRoa {
    /// The prefix it authorizes.
    prefix: &'static str,
    /// The ASNs authorized to originate it.
    asns: &'static [&'static str],
    /// Its `MaxLength`, which the seed also reads as "more specific announcements are
    /// permitted" — the registration member that governs them.
    max_length: Option<i32>,
}

/// The ROAs the IPAM publishes: one for a prefix itself, one for a shorter prefix that permits
/// more specific announcements up to a maximum length, one authorizing NO ASN at all, and two
/// more for the multi-announcement and ASN-less rows below.
const SEEDED_ROAS: [SeededRoa; 5] = [
    SeededRoa {
        prefix: "203.0.113.0/24",
        asns: &["64512"],
        max_length: None,
    },
    SeededRoa {
        prefix: "198.51.100.0/22",
        asns: &["64513", "65000"],
        max_length: Some(23),
    },
    SeededRoa {
        prefix: "192.0.2.0/24",
        asns: &[],
        max_length: None,
    },
    SeededRoa {
        prefix: "203.0.115.0/24",
        asns: &["64512"],
        max_length: None,
    },
    SeededRoa {
        prefix: "203.0.116.0/24",
        asns: &["64512"],
        max_length: None,
    },
];

/// One announcement the report is expected to carry.
struct Expected {
    /// What this row is called in a failure message.
    case: &'static str,
    /// The prefix announced.
    cidr: &'static str,
    /// The ASN announcing it, or `None` for a range holding no ASN association.
    asn: Option<&'static str>,
    /// The `IpamRpkiStatus` route origin validation gives it.
    rpki_status: &'static str,
    /// The `IpamByoipCidrState` the finding reports.
    state: &'static str,
    /// The `IpamByoipAdvertisementType` the finding reports, when the range has one.
    advertisement_type: Option<&'static str>,
    /// The public IPv4 pool the finding reports, when the range is in one.
    pool_id: Option<&'static str>,
    /// Whether the pool managing it is the one created with a `Locale`.
    in_located_pool: bool,
    /// The (prefix, ASN, maximum length, match) of every ROA the finding reports, in the order
    /// it reports them.
    roas: &'static [(&'static str, &'static str, i32, bool)],
}

/// The routes of the IPAM under test, each managed by one of its two pools.
///
/// The first of each pair is the range as the store holds it; the second says whether the
/// LOCATED pool manages it.
const SEEDED_ROUTES: [(SeededRoute, bool); 7] = [
    (
        SeededRoute {
            cidr: "192.0.2.0/24",
            state: BYOIP_CIDR_STATE_ADVERTISED,
            advertisement_type: Some("unicast"),
            pool_id: None,
            asn_associations: &[("64520", ASN_ASSOCIATION_STATE_ASSOCIATED)],
        },
        true,
    ),
    (
        SeededRoute {
            cidr: "198.51.100.0/23",
            state: BYOIP_CIDR_STATE_PROVISIONED,
            advertisement_type: Some("anycast"),
            pool_id: Some("ipv4pool-ec2-0123456789abcdef0"),
            asn_associations: &[("64513", ASN_ASSOCIATION_STATE_ASSOCIATED)],
        },
        true,
    ),
    (
        SeededRoute {
            cidr: "198.51.102.0/24",
            state: BYOIP_CIDR_STATE_PENDING_ADVERTISING,
            advertisement_type: None,
            pool_id: None,
            asn_associations: &[("64513", ASN_ASSOCIATION_STATE_ASSOCIATED)],
        },
        false,
    ),
    (
        SeededRoute {
            cidr: "203.0.113.0/24",
            state: BYOIP_CIDR_STATE_PROVISIONED,
            advertisement_type: Some("unicast"),
            pool_id: None,
            asn_associations: &[("64512", ASN_ASSOCIATION_STATE_ASSOCIATED)],
        },
        false,
    ),
    (
        SeededRoute {
            cidr: "203.0.114.0/24",
            state: BYOIP_CIDR_STATE_PENDING_PROVISION,
            advertisement_type: None,
            pool_id: None,
            asn_associations: &[("64530", ASN_ASSOCIATION_STATE_ASSOCIATED)],
        },
        true,
    ),
    (
        SeededRoute {
            cidr: "203.0.115.0/24",
            state: BYOIP_CIDR_STATE_ADVERTISED,
            advertisement_type: Some("anycast"),
            pool_id: None,
            asn_associations: &[
                ("64512", ASN_ASSOCIATION_STATE_ASSOCIATED),
                ("64540", ASN_ASSOCIATION_STATE_ASSOCIATED),
            ],
        },
        false,
    ),
    (
        SeededRoute {
            cidr: "203.0.116.0/24",
            state: BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE,
            advertisement_type: None,
            pool_id: None,
            // An association the range no longer holds is no announcement of it.
            asn_associations: &[("64550", ASN_ASSOCIATION_STATE_DISASSOCIATED)],
        },
        true,
    ),
];

/// The prefix of a range the account holds in NO IPAM pool, which is therefore no route of any
/// IPAM.
const UNMANAGED_CIDR: &str = "203.0.118.0/24";

/// The prefix of a range the OTHER IPAM's pool manages.
const OTHER_IPAM_CIDR: &str = "203.0.117.0/24";

/// The announcements the IPAM's report is expected to carry, in the order it pages them.
const EXPECTED: [Expected; 8] = [
    Expected {
        case: "a ROA covering the prefix that authorizes no ASN at all",
        cidr: "192.0.2.0/24",
        asn: Some("64520"),
        rpki_status: "invalid",
        state: BYOIP_CIDR_STATE_ADVERTISED,
        advertisement_type: Some("regional"),
        pool_id: None,
        in_located_pool: true,
        roas: &[("192.0.2.0/24", "", 24, false)],
    },
    Expected {
        case: "a shorter ROA permitting more specific announcements, at its maximum length",
        cidr: "198.51.100.0/23",
        asn: Some("64513"),
        rpki_status: "valid",
        state: BYOIP_CIDR_STATE_PROVISIONED,
        advertisement_type: Some("global"),
        pool_id: Some("ipv4pool-ec2-0123456789abcdef0"),
        in_located_pool: true,
        roas: &[
            ("198.51.100.0/22", "64513", 23, true),
            ("198.51.100.0/22", "65000", 23, false),
        ],
    },
    Expected {
        case: "the same ROA, announced one bit longer than the maximum length it authorizes",
        cidr: "198.51.102.0/24",
        asn: Some("64513"),
        rpki_status: "invalid",
        state: BYOIP_CIDR_STATE_PENDING_ADVERTISING,
        advertisement_type: None,
        pool_id: None,
        in_located_pool: false,
        roas: &[
            ("198.51.100.0/22", "64513", 23, false),
            ("198.51.100.0/22", "65000", 23, false),
        ],
    },
    Expected {
        case: "a ROA for the prefix itself, announced by the ASN it authorizes",
        cidr: "203.0.113.0/24",
        asn: Some("64512"),
        rpki_status: "valid",
        state: BYOIP_CIDR_STATE_PROVISIONED,
        advertisement_type: Some("regional"),
        pool_id: None,
        in_located_pool: false,
        roas: &[("203.0.113.0/24", "64512", 24, true)],
    },
    Expected {
        case: "a prefix no ROA covers",
        cidr: "203.0.114.0/24",
        asn: Some("64530"),
        rpki_status: "unknown",
        state: BYOIP_CIDR_STATE_PENDING_PROVISION,
        advertisement_type: None,
        pool_id: None,
        in_located_pool: true,
        roas: &[],
    },
    Expected {
        case: "one prefix announced by two ASNs: the authorized one",
        cidr: "203.0.115.0/24",
        asn: Some("64512"),
        rpki_status: "valid",
        state: BYOIP_CIDR_STATE_ADVERTISED,
        advertisement_type: Some("global"),
        pool_id: None,
        in_located_pool: false,
        roas: &[("203.0.115.0/24", "64512", 24, true)],
    },
    Expected {
        case: "one prefix announced by two ASNs: the one the ROA does not authorize",
        cidr: "203.0.115.0/24",
        asn: Some("64540"),
        rpki_status: "invalid",
        state: BYOIP_CIDR_STATE_ADVERTISED,
        advertisement_type: Some("global"),
        pool_id: None,
        in_located_pool: false,
        roas: &[("203.0.115.0/24", "64512", 24, false)],
    },
    Expected {
        case: "a range holding no ASN association, covered by a ROA",
        cidr: "203.0.116.0/24",
        asn: None,
        rpki_status: "invalid",
        state: BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE,
        advertisement_type: None,
        pool_id: None,
        in_located_pool: true,
        roas: &[("203.0.116.0/24", "64512", 24, false)],
    },
];

/// One finding as the wire reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedFinding {
    cidr: Option<String>,
    asn: Option<String>,
    rpki_status: Option<String>,
    rpki_strength: Option<String>,
    state: Option<String>,
    advertisement_type: Option<String>,
    network_border_group: Option<String>,
    pool_id: Option<String>,
    ipam_pool_id: Option<String>,
    resource_owner_id: Option<String>,
    resource_region: Option<String>,
    sample_time: Option<String>,
    roa_sample_time: Option<String>,
    /// Each reported ROA as (prefix, ASN, maximum length, match).
    roas: Vec<(String, String, String, String)>,
    /// How many overlapping routes the finding reports.
    overlaps: usize,
}

/// The findings a response carries, in the order it reported them.
///
/// Read through the XML parser rather than by substring, so the finding's own `<asn>` is never
/// confused with the one inside a reported ROA.
fn reported(case: &str, body: &str) -> Vec<ReportedFinding> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "routeProtectionFindingSet")
        .into_iter()
        .map(|item| ReportedFinding {
            cidr: child_text(item, "cidr"),
            asn: child_text(item, "asn"),
            rpki_status: child_text(item, "rpkiStatus"),
            rpki_strength: child_text(item, "rpkiStrength"),
            state: child_text(item, "state"),
            advertisement_type: child_text(item, "advertisementType"),
            network_border_group: child_text(item, "networkBorderGroup"),
            pool_id: child_text(item, "poolId"),
            ipam_pool_id: child_text(item, "ipamPoolId"),
            resource_owner_id: child_text(item, "resourceOwnerId"),
            resource_region: child_text(item, "resourceRegion"),
            sample_time: child_text(item, "sampleTime"),
            roa_sample_time: child_text(item, "roaSampleTime"),
            roas: set_items(item, "roaSet")
                .into_iter()
                .map(|roa| {
                    (
                        child_text(roa, "prefix").unwrap_or_default(),
                        child_text(roa, "asn").unwrap_or_default(),
                        child_text(roa, "maxLength").unwrap_or_default(),
                        child_text(roa, "match").unwrap_or_default(),
                    )
                })
                .collect(),
            overlaps: set_items(item, "routeOverlapSet").len(),
        })
        .collect()
}

/// The emulator with both IPAMs, their pools and every seeded range and ROA in place.
async fn fixture() -> Fixture {
    let (router, engine) = super::ec2_query::emulator();
    let (ipam_id, public_scope) = create_ipam(&router).await;
    let located_pool = create_pool(&router, &public_scope, Some(POOL_LOCALE)).await;
    let unlocated_pool = create_pool(&router, &public_scope, None).await;
    let (other_ipam_id, other_scope) = create_ipam(&router).await;
    let other_pool = create_pool(&router, &other_scope, None).await;

    for (route, in_located_pool) in &SEEDED_ROUTES {
        let pool = if *in_located_pool {
            &located_pool
        } else {
            &unlocated_pool
        };
        seed_route(&engine, route, Some(pool));
    }
    seed_route(
        &engine,
        &SeededRoute {
            cidr: OTHER_IPAM_CIDR,
            state: BYOIP_CIDR_STATE_ADVERTISED,
            advertisement_type: Some("unicast"),
            pool_id: None,
            asn_associations: &[("64560", ASN_ASSOCIATION_STATE_ASSOCIATED)],
        },
        Some(&other_pool),
    );
    seed_route(
        &engine,
        &SeededRoute {
            cidr: UNMANAGED_CIDR,
            state: BYOIP_CIDR_STATE_PROVISIONED,
            advertisement_type: None,
            pool_id: Some("ipv4pool-ec2-0123456789abcdef1"),
            asn_associations: &[("64570", ASN_ASSOCIATION_STATE_ASSOCIATED)],
        },
        None,
    );

    let association = seed_association(&engine, &ipam_id);
    for roa in &SEEDED_ROAS {
        seed_roa(&engine, &association, roa);
    }
    // A ROA the OTHER IPAM published for one of this IPAM's prefixes authorizes nothing here:
    // the authorizations a report validates against are the ones its own IPAM holds.
    let other_association = seed_association(&engine, &other_ipam_id);
    seed_roa(
        &engine,
        &other_association,
        &SeededRoa {
            prefix: "203.0.114.0/24",
            asns: &["64530"],
            max_length: None,
        },
    );

    Fixture {
        router,
        engine,
        ipam_id,
        located_pool,
        unlocated_pool,
        other_ipam_id,
        other_pool,
    }
}

/// `GetIpamRouteProtectionFindings` for `ipam_id` with `extra` form fields.
async fn get_findings(
    fixture: &Fixture,
    ipam_id: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![p("IpamId", ipam_id)];
    params.extend(extra.iter().cloned());
    invoke(&fixture.router, "GetIpamRouteProtectionFindings", &params).await
}

/// The same call, requiring success.
async fn findings_ok(
    case: &str,
    fixture: &Fixture,
    ipam_id: &str,
    extra: &[(String, String)],
) -> String {
    let (status, body) = get_findings(fixture, ipam_id, extra).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// The announcement one finding is of, as the page order names it.
fn announcement(finding: &ReportedFinding) -> (String, String) {
    (
        finding.cidr.clone().unwrap_or_default(),
        finding.asn.clone().unwrap_or_default(),
    )
}

#[tokio::test]
async fn every_route_announcement_the_ipam_manages_is_reported_with_its_validation() {
    let fixture = fixture().await;
    let body = findings_ok("the whole report", &fixture, &fixture.ipam_id, &[]).await;
    assert_eq!(
        element(&body, "ipamId").as_deref(),
        Some(fixture.ipam_id.as_str()),
        "the report names the IPAM it is of, got {body}"
    );
    let reported = reported("the whole report", &body);
    let announcements: Vec<(String, String)> = reported.iter().map(announcement).collect();
    let expected_announcements: Vec<(String, String)> = EXPECTED
        .iter()
        .map(|row| {
            (
                row.cidr.to_string(),
                row.asn.unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        announcements, expected_announcements,
        "the report carries one finding per route announcement the IPAM manages, in \
         (prefix, ASN) order: got {announcements:?}"
    );

    let owner = engine::emulator_account_id().to_string();
    for (row, finding) in EXPECTED.iter().zip(&reported) {
        let case = row.case;
        assert_eq!(
            finding.rpki_status.as_deref(),
            Some(row.rpki_status),
            "case {case:?}: expected RPKI status {:?}, got {:?}",
            row.rpki_status,
            finding.rpki_status
        );
        assert_eq!(
            finding.state.as_deref(),
            Some(row.state),
            "case {case:?}: expected the range's stored state {:?}, got {:?}",
            row.state,
            finding.state
        );
        assert_eq!(
            finding.advertisement_type.as_deref(),
            row.advertisement_type,
            "case {case:?}: expected advertisement type {:?}, got {:?}",
            row.advertisement_type,
            finding.advertisement_type
        );
        assert_eq!(
            finding.pool_id.as_deref(),
            row.pool_id,
            "case {case:?}: expected the stored public IPv4 pool {:?}, got {:?}",
            row.pool_id,
            finding.pool_id
        );
        let expected_pool = if row.in_located_pool {
            &fixture.located_pool
        } else {
            &fixture.unlocated_pool
        };
        assert_eq!(
            finding.ipam_pool_id.as_deref(),
            Some(expected_pool.as_str()),
            "case {case:?}: expected the managing pool {expected_pool:?}, got {:?}",
            finding.ipam_pool_id
        );
        // "The locale of the IPAM pool" (`IpamPool$Locale`) when the pool names one, and the
        // Region of the IPAM pool otherwise.
        let expected_region = if row.in_located_pool { POOL_LOCALE } else { REGION };
        assert_eq!(
            finding.resource_region.as_deref(),
            Some(expected_region),
            "case {case:?}: expected the resource Region {expected_region:?}, got {:?}",
            finding.resource_region
        );
        assert_eq!(
            finding.resource_owner_id.as_deref(),
            Some(owner.as_str()),
            "case {case:?}: expected the owning account {owner:?}, got {:?}",
            finding.resource_owner_id
        );
        assert_eq!(
            finding.network_border_group.as_deref(),
            Some(REGION),
            "case {case:?}: expected the range's stored network border group, got {:?}",
            finding.network_border_group
        );
        assert_eq!(
            finding.rpki_strength, None,
            "case {case:?}: nothing in the account's state sets an RPKI enforcement \
             strength, got {:?}",
            finding.rpki_strength
        );
        let expected_roas: Vec<(String, String, String, String)> = row
            .roas
            .iter()
            .map(|(prefix, asn, max_length, matches)| {
                (
                    (*prefix).to_string(),
                    (*asn).to_string(),
                    max_length.to_string(),
                    matches.to_string(),
                )
            })
            .collect();
        assert_eq!(
            finding.roas, expected_roas,
            "case {case:?}: expected the covering ROAs {expected_roas:?}, got {:?}",
            finding.roas
        );
        assert_eq!(
            finding.overlaps, 0,
            "case {case:?}: this emulator holds no announcement beside the account's own \
             ranges, so no overlap can be detected, got {} overlap(s)",
            finding.overlaps
        );
        for (member, at) in [
            ("sampleTime", &finding.sample_time),
            ("roaSampleTime", &finding.roa_sample_time),
        ] {
            let at = at
                .as_deref()
                .unwrap_or_else(|| panic!("case {case:?}: expected a <{member}>, got none"));
            assert!(
                super::ec2_query::as_timestamp(at).is_some(),
                "case {case:?}: expected {member} to be an ISO 8601 instant, got {at:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_token_pages_the_announcements_and_only_resumes_its_own_ipam() {
    let fixture = fixture().await;
    // The smallest page the model admits (`IpamMaxResults`, range 5..1000), so the walk below
    // takes more than one page over the seeded announcements.
    let first = findings_ok(
        "the first page",
        &fixture,
        &fixture.ipam_id,
        &[p("MaxResults", "5")],
    )
    .await;
    let first_page = reported("the first page", &first);
    assert_eq!(
        first_page.len(),
        5,
        "a page of five carries five announcements, got {}",
        first_page.len()
    );
    let token = element(&first, "nextToken")
        .unwrap_or_else(|| panic!("a page an announcement follows carries a token, got {first}"));

    let second = findings_ok(
        "the second page",
        &fixture,
        &fixture.ipam_id,
        &[p("MaxResults", "5"), p("NextToken", &token)],
    )
    .await;
    let second_page = reported("the second page", &second);
    assert_eq!(
        second_page.len(),
        EXPECTED.len() - 5,
        "the rest of the walk carries the remaining announcements, got {}",
        second_page.len()
    );
    assert_eq!(
        element(&second, "nextToken"),
        None,
        "the last page of the walk carries no token, got {second}"
    );
    let walked: Vec<(String, String)> = first_page
        .iter()
        .chain(&second_page)
        .map(announcement)
        .collect();
    let whole: Vec<(String, String)> = reported(
        "the whole report",
        &findings_ok("the whole report", &fixture, &fixture.ipam_id, &[]).await,
    )
    .iter()
    .map(announcement)
    .collect();
    assert_eq!(
        walked, whole,
        "paging the report visits every announcement exactly once, in the report's own \
         order: got {walked:?}"
    );

    // A token names a position in ONE IPAM's report, so another IPAM's walk cannot resume
    // from it.
    let (status, body) = get_findings(
        &fixture,
        &fixture.other_ipam_id,
        &[p("NextToken", &token)],
    )
    .await;
    assert_eq!(
        error_code(&body),
        INVALID_PAGINATION_TOKEN,
        "a token minted for another IPAM's report is refused: got {status} with body {body}"
    );
}

/// One row of the filter table: the case, the `Filters` the request carries, and the
/// (prefix, ASN) announcements the report must then select.
type FilterCase = (
    &'static str,
    Vec<(&'static str, &'static [&'static str])>,
    Vec<(&'static str, &'static str)>,
);

#[tokio::test]
async fn filters_narrow_the_report_and_an_unresolvable_filter_is_refused() {
    let fixture = fixture().await;
    // Each row names a filter and the announcements it must select — a distinct input along
    // what filtering generalises over: the finding's own members, a member of a ROA it
    // reports, several values joined with OR, and two filters joined with AND.
    let cases: Vec<FilterCase> = vec![
        (
            "every invalid announcement",
            vec![("rpki-status", &["invalid"][..])],
            vec![
                ("192.0.2.0/24", "64520"),
                ("198.51.102.0/24", "64513"),
                ("203.0.115.0/24", "64540"),
                ("203.0.116.0/24", ""),
            ],
        ),
        (
            "one prefix",
            vec![("cidr", &["203.0.115.0/24"][..])],
            vec![
                ("203.0.115.0/24", "64512"),
                ("203.0.115.0/24", "64540"),
            ],
        ),
        (
            "two ASNs joined with OR",
            vec![("asn", &["64512", "64530"][..])],
            vec![
                ("203.0.113.0/24", "64512"),
                ("203.0.114.0/24", "64530"),
                ("203.0.115.0/24", "64512"),
            ],
        ),
        (
            "a prefix and a status joined with AND",
            vec![
                ("cidr", &["203.0.115.0/24"][..]),
                ("rpki-status", &["valid"][..]),
            ],
            vec![("203.0.115.0/24", "64512")],
        ),
        (
            "the pool that manages the announcement, by wildcard on the Region it allocates in",
            vec![("resource-region", &["us-west-*"][..])],
            vec![
                ("192.0.2.0/24", "64520"),
                ("198.51.100.0/23", "64513"),
                ("203.0.114.0/24", "64530"),
                ("203.0.116.0/24", ""),
            ],
        ),
        (
            "a ROA's own prefix",
            vec![("roa-prefix", &["198.51.100.0/22"][..])],
            vec![
                ("198.51.100.0/23", "64513"),
                ("198.51.102.0/24", "64513"),
            ],
        ),
        (
            "an advertisement method",
            vec![("advertisement-type", &["global"][..])],
            vec![
                ("198.51.100.0/23", "64513"),
                ("203.0.115.0/24", "64512"),
                ("203.0.115.0/24", "64540"),
            ],
        ),
        (
            "a value nothing carries",
            vec![("rpki-status", &["valid"][..]), ("asn", &["64999"][..])],
            vec![],
        ),
    ];
    for (case, entries, expected) in cases {
        let body = findings_ok(case, &fixture, &fixture.ipam_id, &filters(&entries)).await;
        let selected: Vec<(String, String)> =
            reported(case, &body).iter().map(announcement).collect();
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(cidr, asn)| ((*cidr).to_string(), (*asn).to_string()))
            .collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: expected the announcements {expected:?}, got {selected:?}"
        );
    }

    // A filter this operation does not resolve is refused rather than read as "no findings".
    let (status, body) = get_findings(
        &fixture,
        &fixture.ipam_id,
        &filters(&[("rpki-state", &["valid"])]),
    )
    .await;
    assert_eq!(
        error_code(&body),
        INVALID_PARAMETER_VALUE,
        "an unresolvable filter name is refused: got {status} with body {body}"
    );
    assert!(
        unescaped(&body).contains("The filter 'rpki-state' is invalid"),
        "the refusal names the filter at fault, got {body}"
    );
}

#[tokio::test]
async fn only_the_routes_the_named_ipam_manages_are_reported() {
    let fixture = fixture().await;
    let body = findings_ok("this IPAM", &fixture, &fixture.ipam_id, &[]).await;
    let prefixes: Vec<String> = reported("this IPAM", &body)
        .iter()
        .filter_map(|finding| finding.cidr.clone())
        .collect();
    for absent in [OTHER_IPAM_CIDR, UNMANAGED_CIDR] {
        assert!(
            !prefixes.contains(&absent.to_string()),
            "a range in another IPAM's pool or in no IPAM pool is no route of this IPAM, \
             but {absent} was reported: {prefixes:?}"
        );
    }

    // The other IPAM's own report carries its own route, managed by its own pool, and
    // nothing of this IPAM's.
    let other = findings_ok("the other IPAM", &fixture, &fixture.other_ipam_id, &[]).await;
    let reported_other = reported("the other IPAM", &other);
    let announcements: Vec<(String, String)> =
        reported_other.iter().map(announcement).collect();
    assert_eq!(
        announcements,
        vec![(OTHER_IPAM_CIDR.to_string(), "64560".to_string())],
        "the other IPAM reports exactly the one route its own pool manages, got \
         {announcements:?}"
    );
    assert_eq!(
        reported_other
            .first()
            .and_then(|finding| finding.ipam_pool_id.clone())
            .as_deref(),
        Some(fixture.other_pool.as_str()),
        "that route is reported as managed by the other IPAM's pool, got {other}"
    );
    // Its ROA for one of this IPAM's prefixes is not an authorization of this IPAM's route:
    // the prefix stays `unknown` above, which the whole-report test pins.
    assert_eq!(
        reported_other
            .first()
            .and_then(|finding| finding.rpki_status.clone())
            .as_deref(),
        Some("unknown"),
        "no ROA of the other IPAM covers its own route, got {other}"
    );
}

#[tokio::test]
async fn an_ipam_the_account_does_not_hold_is_refused_before_a_report_is_built() {
    let fixture = fixture().await;
    // Each row: the case, the `IpamId` the request names, and the code EC2 answers. A value
    // outside the form this service ISSUES is malformed whatever it names; one of that form
    // that names nothing the account holds is missing.
    for (case, ipam_id, expected) in [
        ("an id under no prefix at all", "not-an-id", INVALID_IPAM_ID_MALFORMED),
        (
            "an id under another resource's prefix",
            "ipam-pool-0123456789abcdef0",
            INVALID_IPAM_ID_MALFORMED,
        ),
        (
            "an id with too few digits",
            "ipam-0123456",
            INVALID_IPAM_ID_MALFORMED,
        ),
        (
            "an id with a digit outside hexadecimal",
            "ipam-0123456789abcdefg",
            INVALID_IPAM_ID_MALFORMED,
        ),
        (
            "a well-formed short id the account holds no IPAM for",
            "ipam-01234567",
            INVALID_IPAM_ID_NOT_FOUND,
        ),
        (
            "a well-formed long id the account holds no IPAM for",
            "ipam-0123456789abcdef0",
            INVALID_IPAM_ID_NOT_FOUND,
        ),
    ] {
        let (status, body) = get_findings(&fixture, ipam_id, &[]).await;
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {status} with body {body}"
        );
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
    }
}

#[tokio::test]
async fn a_dry_run_answers_the_permission_question_and_reports_nothing() {
    let fixture = fixture().await;
    let (status, body) = get_findings(&fixture, &fixture.ipam_id, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "a dry run answers the permission question, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        DRY_RUN_OPERATION,
        "a dry run answers {DRY_RUN_OPERATION}, got {body}"
    );
    assert!(
        reported("the dry run", &body).is_empty(),
        "a dry run reports no finding, got {body}"
    );
    // A dry run that would have been REFUSED is refused with that reason instead.
    let (status, body) = get_findings(&fixture, "not-an-id", &[p("DryRun", "true")]).await;
    assert_eq!(
        error_code(&body),
        INVALID_IPAM_ID_MALFORMED,
        "a dry run naming something that is not an IPAM id is refused for that reason, got \
         {status} with body {body}"
    );
}

#[tokio::test]
async fn reading_the_report_changes_nothing_a_later_caller_sees() {
    let fixture = fixture().await;
    let before = fixture
        .engine
        .state::<Ec2State>()
        .lock()
        .byoip_cidrs
        .iter()
        .map(|(cidr, record)| (cidr.clone(), format!("{:?}", record.byoip_cidr)))
        .collect::<Vec<_>>();
    let first = reported(
        "the first read",
        &findings_ok("the first read", &fixture, &fixture.ipam_id, &[]).await,
    );
    let second = reported(
        "the second read",
        &findings_ok("the second read", &fixture, &fixture.ipam_id, &[]).await,
    );
    let without_sample_times = |findings: Vec<ReportedFinding>| -> Vec<ReportedFinding> {
        findings
            .into_iter()
            .map(|finding| ReportedFinding {
                // The sample instants are facts of WHEN the report was taken, not of the
                // routes, so they are the one member two reads may differ on.
                sample_time: None,
                roa_sample_time: None,
                ..finding
            })
            .collect()
    };
    assert_eq!(
        without_sample_times(first),
        without_sample_times(second),
        "two reads of the report answer the same findings"
    );
    let after = fixture
        .engine
        .state::<Ec2State>()
        .lock()
        .byoip_cidrs
        .iter()
        .map(|(cidr, record)| (cidr.clone(), format!("{:?}", record.byoip_cidr)))
        .collect::<Vec<_>>();
    assert_eq!(
        before, after,
        "reporting the findings writes nothing to the ranges it reports"
    );
}

/// The state and status words the rows above pin are the model's own: each is a member of the
/// catalog the model publishes for its enum, so a row cannot pin a word the API does not have.
#[test]
fn the_asserted_words_are_the_models_own() {
    for (case, word, catalog) in [
        ("a valid route", "valid", IPAM_RPKI_STATUS_VALUES),
        ("an invalid route", "invalid", IPAM_RPKI_STATUS_VALUES),
        ("a route no ROA covers", "unknown", IPAM_RPKI_STATUS_VALUES),
        (
            "a route advertised from one location",
            "regional",
            IPAM_BYOIP_ADVERTISEMENT_TYPE_VALUES,
        ),
        (
            "a route advertised from many",
            "global",
            IPAM_BYOIP_ADVERTISEMENT_TYPE_VALUES,
        ),
    ] {
        assert!(
            catalog.contains(&word),
            "case {case:?}: {word:?} must be a member of the modelled catalog {catalog:?}"
        );
    }
    // Every state a seeded range sits in is a word BOTH catalogs carry: the finding reports a
    // range's own `ByoipCidrState` under `IpamByoipCidrState`.
    for (route, _) in &SEEDED_ROUTES {
        assert!(
            BYOIP_CIDR_STATE_VALUES.contains(&route.state)
                && IPAM_BYOIP_CIDR_STATE_VALUES.contains(&route.state),
            "the seeded state {:?} must be a member of both {BYOIP_CIDR_STATE_VALUES:?} and \
             {IPAM_BYOIP_CIDR_STATE_VALUES:?}",
            route.state
        );
    }
    assert!(
        ec2::ops::types::IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES
            .contains(&ASSOCIATION_STATE_ENABLE_COMPLETE),
        "the seeded association state must be a member of its modelled catalog"
    );
}
