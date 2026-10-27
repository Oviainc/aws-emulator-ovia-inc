// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AllocateHosts`, driven over the wire the service actually speaks:
//! `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=AllocateHosts`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation ALLOCATES EXACTLY THE NUMBER OF HOSTS THE
//! REQUEST ASKS FOR AND PERSISTS EACH OF THEM AS CONFIGURED: one `Quantity` of distinct
//! Dedicated Hosts, each carrying the configuration that one request named — the instance
//! type or the instance family, the Availability Zone (named by its name or by its id, which
//! name one zone either way), and the tags — so a following `DescribeHosts` reports every
//! one of them with those members and with the hosts the account already held untouched.
//!
//! The suites beside this one check the operations that READ those hosts back
//! (`dedicated_hosts` for `DescribeHosts`, `mac_dedicated_hosts` for `DescribeMacHosts`,
//! `release_hosts` for `ReleaseHosts`) and the refusals a bad request gets
//! (`resource_specific_error_codes`). What is pinned HERE is the write itself, along the
//! dimensions the rule generalises over: how MANY hosts one call allocates, whether an
//! instance type or an instance family configures them, which zone they go into and which
//! of the two members names it, and which tags they carry.

use ec2::state::{
    availability_zones_in_region, host_id_is_well_formed, instance_family_is_modelled,
    instance_family_of, instance_type_is_modelled, ALLOWS_MULTIPLE_INSTANCE_TYPES_OFF,
    ALLOWS_MULTIPLE_INSTANCE_TYPES_ON, AMD_SEV_SNP_DISABLED, AVAILABILITY_ZONES_PER_REGION,
    DEDICATED_HOST_TAGGABLE_RESOURCE, DEFAULT_AUTO_PLACEMENT, DEFAULT_HOST_RECOVERY,
    HOST_LIMIT_EXCEEDED, HOST_STATE_AVAILABLE, INVALID_PARAMETER_VALUE,
    MAX_STORED_DEDICATED_HOSTS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, elements, error_code, indexed};

/// The Region every call here is signed for; the zone names and ids the hosts go into are
/// derived from it.
const REGION: &str = "us-east-2";

/// One `(key, value)` request parameter.
fn param(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The name of the `index`th Availability Zone of the signed Region.
fn zone_name(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_name.clone()
}

/// The id of the `index`th Availability Zone of the signed Region.
fn zone_id(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_id.clone()
}

/// Which member a request names the Availability Zone by: "The Availability Zone in which to
/// allocate the Dedicated Host" (`AllocateHostsRequest$AvailabilityZone`) or "The ID of the
/// Availability Zone" (`$AvailabilityZoneId`) — two spellings of one zone.
#[derive(Clone, Copy, Debug)]
enum ZoneBy {
    Name,
    Id,
}

impl ZoneBy {
    /// The zone parameter for the `index`th zone in this spelling.
    fn param(self, index: usize) -> (String, String) {
        match self {
            Self::Name => param("AvailabilityZone", zone_name(index)),
            Self::Id => param("AvailabilityZoneId", zone_id(index)),
        }
    }
}

/// How one call configures the hosts it allocates: by the single instance type they support
/// ("the Dedicated Hosts support instances of the specified instance type only",
/// `AllocateHostsRequest$InstanceType`) or by the whole instance family
/// ("the Dedicated Hosts support multiple instance types within that instance family",
/// `$InstanceFamily`).
#[derive(Clone, Copy, Debug)]
enum Configuration {
    /// One `InstanceType` value, e.g. a single size of a family.
    InstanceType(&'static str),
    /// One `InstanceFamily` value.
    InstanceFamily(&'static str),
}

impl Configuration {
    /// The form parameter that carries this configuration.
    fn param(self) -> (String, String) {
        match self {
            Self::InstanceType(instance_type) => param("InstanceType", instance_type),
            Self::InstanceFamily(instance_family) => param("InstanceFamily", instance_family),
        }
    }

    /// The instance family a host allocated for this configuration is configured for: "The
    /// instance family […] For example, m5" (`HostProperties$InstanceFamily`), which an
    /// instance type carries as its leading component.
    fn instance_family(self) -> &'static str {
        match self {
            Self::InstanceType(instance_type) => instance_family_of(instance_type),
            Self::InstanceFamily(instance_family) => instance_family,
        }
    }

    /// The instance type a host allocated for this configuration reports: "If the host
    /// supports multiple instance types, no instanceType is returned"
    /// (`HostProperties$InstanceType`).
    fn reported_instance_type(self) -> Option<&'static str> {
        match self {
            Self::InstanceType(instance_type) => Some(instance_type),
            Self::InstanceFamily(_) => None,
        }
    }

    /// Whether a host allocated for this configuration "supports multiple instance types in
    /// the instance family" (`Host$AllowsMultipleInstanceTypes`).
    fn allows_multiple_instance_types(self) -> &'static str {
        match self {
            Self::InstanceType(_) => ALLOWS_MULTIPLE_INSTANCE_TYPES_OFF,
            Self::InstanceFamily(_) => ALLOWS_MULTIPLE_INSTANCE_TYPES_ON,
        }
    }

    /// Fail the case unless the value names hardware the MODEL declares — the instance type
    /// catalog is the modelled `InstanceType` enum, which
    /// [`instance_type_is_modelled`]/[`instance_family_is_modelled`] are the crate's one
    /// reading of, so no row of a table here can pin a value EC2 does not offer.
    fn assert_modelled(self, case: &str) {
        match self {
            Self::InstanceType(instance_type) => assert!(
                instance_type_is_modelled(instance_type),
                "case {case:?}: {instance_type:?} must be a value of the modelled \
                 InstanceType enum"
            ),
            Self::InstanceFamily(instance_family) => assert!(
                instance_family_is_modelled(instance_family),
                "case {case:?}: {instance_family:?} must be the family of at least one \
                 value of the modelled InstanceType enum"
            ),
        }
    }
}

/// The `TagSpecification` form parameters that tag the hosts one call allocates: "The tags
/// to apply to the Dedicated Host during creation"
/// (`AllocateHostsRequest$TagSpecifications`).
///
/// Built here rather than in the shared [`super::ec2_query`] harness because that is how
/// every suite in this crate that sends a `TagSpecifications` list builds it — the harness
/// owns the request PLUMBING (the form body, the indexed-list flattening it reuses below),
/// and which resource type and tag list a request carries is the suite's own input.
fn host_tags(tags: &[(&str, &str)]) -> Params {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        DEDICATED_HOST_TAGGABLE_RESOURCE,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            *key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            *value,
        ));
    }
    params
}

/// One allocation: the configuration, how many hosts, which zone and how it is named, and
/// the tags the call asks for.
#[derive(Clone, Copy, Debug)]
struct Allocation {
    configuration: Configuration,
    quantity: usize,
    zone: usize,
    zone_by: ZoneBy,
    tags: &'static [(&'static str, &'static str)],
}

/// The ids in an `AllocateHosts` answer: the `<item>`s of its `hostIdSet`.
fn allocated_ids(body: &str) -> Vec<String> {
    element(body, "hostIdSet")
        .map(|set| elements(&set, "item"))
        .unwrap_or_default()
}

/// The request `allocation` makes, plus whatever `extra` parameters the case adds.
fn allocation_params(allocation: Allocation, extra: Params) -> Params {
    let mut params = vec![
        allocation.configuration.param(),
        allocation.zone_by.param(allocation.zone),
        param("Quantity", allocation.quantity.to_string()),
    ];
    params.extend(host_tags(allocation.tags));
    params.extend(extra);
    params
}

/// One `AllocateHosts` call for `allocation` with `extra` request parameters: its status and
/// body, whatever the service answered.
async fn allocate_raw(
    router: &MultiServiceRouter,
    allocation: Allocation,
    extra: Params,
) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "AllocateHosts",
        &allocation_params(allocation, extra),
    )
    .await
}

/// Run one `AllocateHosts` call for `allocation` and answer the ids it reports, failing the
/// case when it was refused or reported anything other than `quantity` distinct, well-formed
/// host ids.
async fn allocate(router: &MultiServiceRouter, case: &str, allocation: Allocation) -> Vec<String> {
    allocate_with(router, case, allocation, Vec::new()).await
}

/// [`allocate`], with `extra` request parameters beside the ones `allocation` names.
async fn allocate_with(
    router: &MultiServiceRouter,
    case: &str,
    allocation: Allocation,
    extra: Params,
) -> Vec<String> {
    allocation.configuration.assert_modelled(case);
    let (status, body) = allocate_raw(router, allocation, extra).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AllocateHosts, got {status} with body {body}"
    );
    let ids = allocated_ids(&body);
    assert_eq!(
        ids.len(),
        allocation.quantity,
        "case {case:?}: expected {} allocated host ids, got {ids:?}",
        allocation.quantity
    );
    for id in &ids {
        assert!(
            host_id_is_well_formed(id),
            "case {case:?}: expected a well-formed host id, got {id:?} in {ids:?}"
        );
    }
    let mut distinct = ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "case {case:?}: expected one host per unit of quantity, got {ids:?}"
    );
    ids
}

/// Every host id `DescribeHosts` reports for the whole account, in the order it reported
/// them.
async fn described_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = super::ec2_query::call(router, REGION, "DescribeHosts", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeHosts, got {status} with body {body}"
    );
    elements(&body, "hostId")
}

/// The `DescribeHosts` report for the one host `host_id` names, as the response body.
///
/// The one reader of a single host in this suite: every projection below reads the SAME
/// response, so the members and the tags an assertion compares cannot come from two
/// different reports of one host.
async fn described_host(router: &MultiServiceRouter, case: &str, host_id: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeHosts",
        &indexed("HostId", &[host_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 describing {host_id}, got {status} with body {body}"
    );
    let reported = elements(&body, "hostId");
    assert_eq!(
        reported,
        vec![host_id.to_string()],
        "case {case:?}: expected the describe to report only {host_id}, got {reported:?}"
    );
    body
}

/// The members one host's report carries, as `(member, text)` pairs — `None` for a member
/// the report leaves out.
fn host_members(body: &str) -> Vec<(&'static str, Option<String>)> {
    [
        "hostId",
        "availabilityZone",
        "availabilityZoneId",
        "state",
        "instanceType",
        "instanceFamily",
        "allowsMultipleInstanceTypes",
        "autoPlacement",
        "hostRecovery",
        "amdSevSnp",
        "ownerId",
        "allocationTime",
    ]
    .into_iter()
    .map(|name| (name, element(body, name)))
    .collect()
}

/// The value `members` carries for `name`.
fn reported_member<'members>(
    members: &'members [(&'static str, Option<String>)],
    name: &str,
) -> Option<&'members str> {
    members
        .iter()
        .find(|(member, _)| *member == name)
        .and_then(|(_, value)| value.as_deref())
}

/// The tags one host's report carries, as `(key, value)` pairs in the order reported.
fn host_tag_pairs(body: &str) -> Vec<(String, String)> {
    // The `tagSet` of the one reported host, read out first so a member named `value`
    // anywhere else in the envelope cannot be paired with a tag key.
    let Some(tag_set) = element(body, "tagSet") else {
        return Vec::new();
    };
    let keys = elements(&tag_set, "key");
    let values = elements(&tag_set, "value");
    keys.into_iter().zip(values).collect()
}

/// Assert that the one host `host_id` names was stored as `allocation` asked for it.
async fn assert_host_matches(
    router: &MultiServiceRouter,
    case: &str,
    host_id: &str,
    allocation: Allocation,
) {
    // ONE report of the host, read by both projections below.
    let body = described_host(router, case, host_id).await;
    let members = host_members(&body);
    let configuration = allocation.configuration;
    for (name, expected) in [
        ("hostId", Some(host_id.to_string())),
        // The zone is the one the request named, in BOTH spellings, whichever of the two
        // members the request used to name it.
        ("availabilityZone", Some(zone_name(allocation.zone))),
        ("availabilityZoneId", Some(zone_id(allocation.zone))),
        // "Allocates a Dedicated Host to your account" (`AllocateHosts`): the host is
        // allocated by the time the call answers.
        ("state", Some(HOST_STATE_AVAILABLE.to_string())),
        (
            "instanceType",
            configuration.reported_instance_type().map(str::to_string),
        ),
        (
            "instanceFamily",
            Some(configuration.instance_family().to_string()),
        ),
        (
            "allowsMultipleInstanceTypes",
            Some(configuration.allows_multiple_instance_types().to_string()),
        ),
        // The request named neither setting, so each host carries the documented default:
        // "Default: off" (`$AutoPlacement`) and "Host recovery is disabled by default […]
        // Default: off" (`$HostRecovery`), and AMD SEV-SNP is disabled when the request
        // carries no `CpuOptions` (`HostCpuOptionsRequest$AmdSevSnp`).
        ("autoPlacement", Some(DEFAULT_AUTO_PLACEMENT.to_string())),
        ("hostRecovery", Some(DEFAULT_HOST_RECOVERY.to_string())),
        ("amdSevSnp", Some(AMD_SEV_SNP_DISABLED.to_string())),
        (
            "ownerId",
            Some(engine::emulator_account_id().to_string()),
        ),
    ] {
        let actual = reported_member(&members, name).map(str::to_string);
        assert_eq!(
            actual, expected,
            "case {case:?}: host {host_id} must report {name} as {expected:?}, got {actual:?}"
        );
    }
    let allocation_time = reported_member(&members, "allocationTime").map(str::to_string);
    assert!(
        allocation_time
            .as_deref()
            .and_then(as_timestamp)
            .is_some(),
        "case {case:?}: host {host_id} must report an allocationTime, got {allocation_time:?}"
    );

    let mut expected_tags: Vec<(String, String)> = allocation
        .tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    // The tags a host carries are a SET: which tags were asked for is the rule under test,
    // so both sides are compared in one order rather than pinning the order the store
    // happened to keep them in.
    expected_tags.sort();
    let mut reported_tags = host_tag_pairs(&body);
    reported_tags.sort();
    assert_eq!(
        reported_tags, expected_tags,
        "case {case:?}: host {host_id} must carry the tags the request asked for \
         ({expected_tags:?}), got {reported_tags:?}"
    );
}

/// The rule: one call allocates exactly `Quantity` hosts, each configured as the request
/// asked, and the store holds exactly those hosts afterwards.
///
/// The rows differ along what the rule generalises over: how many hosts one call allocates,
/// whether an instance type or a whole instance family configures them, which zone they go
/// into, which of the two members names that zone, and how many tags they carry. Every
/// instance type and family named here is checked against the modelled `InstanceType` enum
/// before the call, so a row cannot pin hardware EC2 does not offer.
#[tokio::test]
async fn allocate_hosts_mints_quantity_hosts_configured_by_the_request() {
    let rows: Vec<(&str, Allocation)> = vec![
        (
            "one host for a single instance type, zone by name, no tags",
            Allocation {
                configuration: Configuration::InstanceType("c6i.large"),
                quantity: 1,
                zone: 0,
                zone_by: ZoneBy::Name,
                tags: &[],
            },
        ),
        (
            "two hosts for a whole instance family, zone by name, one tag",
            Allocation {
                configuration: Configuration::InstanceFamily("c6g"),
                quantity: 2,
                zone: 1,
                zone_by: ZoneBy::Name,
                tags: &[("Name", "build-fleet")],
            },
        ),
        (
            "three hosts for a single instance type, zone by id, two tags",
            Allocation {
                configuration: Configuration::InstanceType("c6gn.16xlarge"),
                quantity: 3,
                zone: 2,
                zone_by: ZoneBy::Id,
                tags: &[("Name", "latency"), ("Team", "search")],
            },
        ),
        (
            "five hosts for a whole instance family, zone by id, no tags",
            Allocation {
                configuration: Configuration::InstanceFamily("c6id"),
                quantity: 5,
                zone: 0,
                zone_by: ZoneBy::Id,
                tags: &[],
            },
        ),
        (
            "one host for a bare metal instance type, zone by id, one tag",
            Allocation {
                configuration: Configuration::InstanceType("c5.metal"),
                quantity: 1,
                zone: 1,
                zone_by: ZoneBy::Id,
                tags: &[("Purpose", "licensing")],
            },
        ),
    ];

    for (case, allocation) in rows {
        let (router, _engine) = super::ec2_query::emulator();
        let allocated = allocate(&router, case, allocation).await;

        // The hosts the call reported are the hosts the account now holds: nothing else was
        // stored, and every id the answer named can be read back.
        let mut expected = allocated.clone();
        expected.sort();
        let reported = described_ids(&router, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the store to hold exactly the allocated hosts, \
             got {reported:?}"
        );

        for host_id in &allocated {
            assert_host_matches(&router, case, host_id, allocation).await;
        }
    }
}

/// The zone may be named by its NAME or by its ID, and the two name one zone: a host
/// allocated either way sits in the same zone and reports both of its spellings.
#[tokio::test]
async fn the_zone_may_be_named_by_its_name_or_by_its_id() {
    // Every zone of the Region, so the rule is checked on each one rather than on the first.
    for zone in 0..AVAILABILITY_ZONES_PER_REGION {
        let case = format!("zone {zone} of {REGION}");
        let (router, _engine) = super::ec2_query::emulator();

        let mut allocated = Vec::new();
        for zone_by in [ZoneBy::Name, ZoneBy::Id] {
            let allocation = Allocation {
                configuration: Configuration::InstanceType("c6a.metal"),
                quantity: 1,
                zone,
                zone_by,
                tags: &[],
            };
            let ids = allocate(&router, &case, allocation).await;
            let host_id = ids.into_iter().next().unwrap_or_else(|| {
                panic!("case {case:?}: an allocation of one host must report its id")
            });
            assert_host_matches(&router, &case, &host_id, allocation).await;
            allocated.push(host_id);
        }

        assert_ne!(
            allocated.first(),
            allocated.last(),
            "case {case:?}: two allocations must mint two hosts, got {allocated:?}"
        );
        let mut expected = allocated.clone();
        expected.sort();
        let reported = described_ids(&router, &case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected both hosts to be held, got {reported:?}"
        );
    }
}

/// A later allocation adds its own hosts and leaves the ones the account already holds as
/// they were stored: one call's configuration never reaches another call's hosts.
#[tokio::test]
async fn a_later_allocation_leaves_the_hosts_already_held_as_stored() {
    // Batches differing in configuration, size, zone and zone spelling, applied in order to
    // ONE account.
    let batches: Vec<(&str, Allocation)> = vec![
        (
            "first batch: two hosts for one instance type",
            Allocation {
                configuration: Configuration::InstanceType("c6g.medium"),
                quantity: 2,
                zone: 0,
                zone_by: ZoneBy::Name,
                tags: &[("Batch", "first")],
            },
        ),
        (
            "second batch: one host for a whole family in another zone",
            Allocation {
                configuration: Configuration::InstanceFamily("c5"),
                quantity: 1,
                zone: 1,
                zone_by: ZoneBy::Id,
                tags: &[],
            },
        ),
        (
            "third batch: three hosts for one instance type in a third zone",
            Allocation {
                configuration: Configuration::InstanceType("c6i.metal"),
                quantity: 3,
                zone: 2,
                zone_by: ZoneBy::Name,
                tags: &[("Batch", "third"), ("Owner", "platform")],
            },
        ),
    ];

    let (router, _engine) = super::ec2_query::emulator();
    let mut held: Vec<(String, Allocation)> = Vec::new();
    for (case, allocation) in batches {
        let before = described_ids(&router, case).await;
        let allocated = allocate(&router, case, allocation).await;
        held.extend(
            allocated
                .iter()
                .map(|host_id| (host_id.clone(), allocation)),
        );

        // The account gained exactly this batch, and no host of an earlier batch moved.
        let mut expected: Vec<String> = held.iter().map(|(host_id, _)| host_id.clone()).collect();
        expected.sort();
        let reported = described_ids(&router, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: the account held {} hosts before this batch of {}, so it must \
             hold {} now, got {reported:?}",
            before.len(),
            allocation.quantity,
            expected.len()
        );

        // Every host the account holds — this batch's and every earlier batch's — still
        // reports the configuration ITS OWN call asked for.
        for (host_id, stored_as) in &held {
            assert_host_matches(&router, case, host_id, *stored_as).await;
        }
    }
}

/// One `ClientToken` names ONE allocation: "unique, case-sensitive identifier that you
/// provide to ensure the idempotency of the request"
/// (`AllocateHostsRequest$ClientToken`) — a repeat of the request reports the hosts that
/// request allocated and allocates none.
///
/// The rows differ in how many hosts the one call allocates and in what configures them, so
/// the rule is asserted over a batch as well as over a single host.
#[tokio::test]
async fn one_client_token_names_one_allocation() {
    let rows: Vec<(&str, Allocation)> = vec![
        (
            "one host by instance type",
            Allocation {
                configuration: Configuration::InstanceType("c6i.large"),
                quantity: 1,
                zone: 0,
                zone_by: ZoneBy::Name,
                tags: &[("Name", "idempotent")],
            },
        ),
        (
            "three hosts by instance family",
            Allocation {
                configuration: Configuration::InstanceFamily("c6g"),
                quantity: 3,
                zone: 1,
                zone_by: ZoneBy::Id,
                tags: &[],
            },
        ),
    ];

    for (case, allocation) in rows {
        let (router, _engine) = super::ec2_query::emulator();
        let token = vec![param("ClientToken", format!("token-{case}"))];

        let first = allocate_with(&router, case, allocation, token.clone()).await;
        let replayed = allocate_with(&router, case, allocation, token).await;
        assert_eq!(
            replayed, first,
            "case {case:?}: the replay must report the hosts the first call allocated, \
             got {replayed:?}"
        );

        // And it allocated none: the account holds exactly the one allocation's hosts, each
        // still as that call configured it.
        let mut expected = first.clone();
        expected.sort();
        let reported = described_ids(&router, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: a replayed request must not allocate more hosts, got {reported:?}"
        );
        for host_id in &first {
            assert_host_matches(&router, case, host_id, allocation).await;
        }
    }
}

/// A `Quantity` the service cannot satisfy is refused with EC2's own code, and nothing
/// partial is stored: the count the request names is never the size of a write.
///
/// The rows are the ways a count can be unanswerable — none at all, a negative count, one
/// past the number of hosts this emulator stores ([`MAX_STORED_DEDICATED_HOSTS`]), and the
/// largest value the member can carry — each asserted against the account the earlier
/// allocation left, which must be exactly what the account still holds afterwards.
#[tokio::test]
async fn a_quantity_the_service_cannot_satisfy_is_refused_and_stores_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    let held = allocate(
        &router,
        "the host held before the refusals",
        Allocation {
            configuration: Configuration::InstanceType("c6i.large"),
            quantity: 1,
            zone: 0,
            zone_by: ZoneBy::Name,
            tags: &[("Name", "held")],
        },
    )
    .await;

    let ceiling = MAX_STORED_DEDICATED_HOSTS + 1;
    let rows: Vec<(&str, String, &str)> = vec![
        ("no host at all", "0".to_string(), INVALID_PARAMETER_VALUE),
        ("a negative count", "-2".to_string(), INVALID_PARAMETER_VALUE),
        (
            "one host past what this emulator stores",
            ceiling.to_string(),
            HOST_LIMIT_EXCEEDED,
        ),
        (
            "the largest count the member can carry",
            i32::MAX.to_string(),
            HOST_LIMIT_EXCEEDED,
        ),
    ];
    for (case, quantity, expected_code) in &rows {
        // The quantity is sent as the case names it, so a value no `usize` holds is sent as
        // written rather than being folded into the `Allocation` shape.
        let params = vec![
            param("InstanceType", "c6i.large"),
            param("AvailabilityZone", zone_name(0)),
            param("Quantity", quantity.clone()),
        ];
        let (status, body) =
            super::ec2_query::call(&router, REGION, "AllocateHosts", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?} (Quantity={quantity}): expected HTTP 400, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, *expected_code,
            "case {case:?} (Quantity={quantity}): expected error code {expected_code}, \
             got {code} with body {body}"
        );
        assert!(
            allocated_ids(&body).is_empty(),
            "case {case:?} (Quantity={quantity}): a refusal reports no host id, got {body}"
        );

        let reported = described_ids(&router, case).await;
        assert_eq!(
            reported, held,
            "case {case:?} (Quantity={quantity}): a refused allocation must store nothing, \
             got {reported:?}"
        );
    }
}
