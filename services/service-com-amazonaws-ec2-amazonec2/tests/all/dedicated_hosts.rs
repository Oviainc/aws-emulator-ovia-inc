// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeHosts`, driven over the wire the service actually speaks:
//! `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeHosts`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the Dedicated Hosts the account holds, narrowed by the `HostIds` and the documented
//! `Filter`s the request names, cut into `MaxResults`-sized pages in `HostId` order, with
//! every member of a stored host reported as stored and nothing written by a read. Every
//! host these tests describe was allocated through `AllocateHosts` over the same wire, so
//! the two operations are checked against each other rather than against a hand-written
//! record, and every table row is a distinct input along what the rule generalises over —
//! which filter, which values, which ids, which page size.

use ec2::state::{
    AMD_SEV_SNP_DISABLED, ALLOWS_MULTIPLE_INSTANCE_TYPES_OFF, ALLOWS_MULTIPLE_INSTANCE_TYPES_ON,
    DEFAULT_AUTO_PLACEMENT, DEFAULT_HOST_RECOVERY, DEDICATED_HOST_TAGGABLE_RESOURCE,
    HOST_STATE_AVAILABLE, availability_zones_in_region,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for; the zone names and ids the hosts are allocated
/// into are derived from it.
const REGION: &str = "us-east-1";

/// `on`, for the host settings a request switches on (`AutoPlacement`, `HostRecovery`,
/// `HostMaintenance` — each modelled enum publishes exactly `on` and `off`).
const ON: &str = "on";

/// `released`, the `AllocationState` member a host the account has given back carries
/// ("Dedicated Hosts that have recently been released are listed with the state
/// released", `DescribeHosts`). `ReleaseHosts` is what would move a host into it, and that
/// operation is still a stub, so no host here is in it.
const STATE_RELEASED: &str = "released";

/// One `(key, value)` request parameter.
fn param(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `DescribeHosts` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeHosts", params).await
}

/// The host ids a successful response reports, in the order it reported them.
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
    elements(&body, "hostId")
}

/// The name of the `index`th Availability Zone of the signed Region.
fn zone_name(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_name.clone()
}

/// The id of the `index`th Availability Zone of the signed Region.
fn zone_id(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_id.clone()
}

/// How one seeded host is allocated: the member that configures it (`InstanceType` or
/// `InstanceFamily`) and its value, the zone it goes into, and the extra request parameters
/// that make it differ from its neighbours.
struct Allocation {
    configuration: (&'static str, &'static str),
    zone: usize,
    extra: Vec<(String, String)>,
}

/// The `TagSpecification` form parameters for `tags` on the host the call allocates.
fn host_tags(tags: &[(&str, &str)]) -> Vec<(String, String)> {
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

/// The Dedicated Hosts these tests allocate, in allocation order. They differ along every
/// attribute the documented filters select on: the zone, auto-placement, the client token,
/// whether the host was allocated for one instance type or for a whole instance family, and
/// the tags it carries.
fn allocations() -> Vec<Allocation> {
    vec![
        // One instance type, auto-placement left at its default, a client token and tags.
        Allocation {
            configuration: ("InstanceType", "m5.large"),
            zone: 0,
            extra: {
                let mut extra = vec![param("ClientToken", "token-alpha")];
                extra.extend(host_tags(&[("Name", "prod-a"), ("Team", "payments")]));
                extra
            },
        },
        // A whole instance family, so the host reports no instance type of its own and
        // allows multiple instance types.
        Allocation {
            configuration: ("InstanceFamily", "m5"),
            zone: 0,
            extra: {
                let mut extra = vec![param("AutoPlacement", ON)];
                extra.extend(host_tags(&[("Name", "prod-b")]));
                extra
            },
        },
        // Another zone, another instance type, auto-placement on.
        Allocation {
            configuration: ("InstanceType", "c5.xlarge"),
            zone: 1,
            extra: vec![param("AutoPlacement", ON)],
        },
        // A second `m5.large` host, so the `instance-type` filter selects a SET.
        Allocation {
            configuration: ("InstanceType", "m5.large"),
            zone: 1,
            extra: host_tags(&[("Team", "payments")]),
        },
        // A bare-metal host in a third zone.
        Allocation {
            configuration: ("InstanceType", "r5.metal"),
            zone: 2,
            extra: Vec::new(),
        },
        // A family host with host recovery and host maintenance both on.
        Allocation {
            configuration: ("InstanceFamily", "c5"),
            zone: 2,
            extra: vec![
                param("AutoPlacement", ON),
                param("HostRecovery", ON),
                param("HostMaintenance", ON),
            ],
        },
        // A Mac host with its own client token.
        Allocation {
            configuration: ("InstanceType", "mac1.metal"),
            zone: 0,
            extra: {
                let mut extra = vec![param("ClientToken", "token-beta")];
                extra.extend(host_tags(&[("Name", "build")]));
                extra
            },
        },
    ]
}

/// Allocate the hosts of [`allocations`] through `AllocateHosts` over the wire, and answer
/// the ids they were given, in allocation order.
async fn seed(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for (position, allocation) in allocations().into_iter().enumerate() {
        let (member, value) = allocation.configuration;
        let mut params = vec![
            param(member, value),
            param("AvailabilityZone", zone_name(allocation.zone)),
            param("Quantity", "1"),
        ];
        params.extend(allocation.extra);
        let (status, body) = super::ec2_query::call(router, REGION, "AllocateHosts", &params).await;
        assert_eq!(
            status, 200,
            "host {position}: AllocateHosts must succeed, got {status} with body {body}"
        );
        let allocated = element(&body, "hostIdSet")
            .map(|set| elements(&set, "item"))
            .unwrap_or_default();
        assert_eq!(
            allocated.len(),
            1,
            "host {position}: expected exactly one allocated id, got {allocated:?}"
        );
        ids.push(allocated[0].clone());
    }
    ids
}

/// `ids` as the borrowed slice the tables take.
fn borrowed(ids: &[String]) -> Vec<&str> {
    ids.iter().map(String::as_str).collect()
}

/// The allocated ids at `positions` (counting from 0, in allocation order), sorted the way
/// the describe reports them: by host id.
fn ids_at(allocated: &[String], positions: &[usize]) -> Vec<String> {
    let mut picked: Vec<String> = positions
        .iter()
        .map(|position| {
            allocated
                .get(*position)
                .unwrap_or_else(|| panic!("host {position} must have been allocated"))
                .clone()
        })
        .collect();
    picked.sort();
    picked
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// An account that has allocated no Dedicated Host is reported as an empty set rather than
/// a refusal: nothing allocates one until a caller asks.
#[tokio::test]
async fn an_account_that_allocated_no_host_is_reported_as_an_empty_set() {
    let (router, _engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "an account with no hosts: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "hostId").is_empty(),
        "an account with no hosts: expected no host reported, got {body}"
    );
}

/// Every host the store holds is reported, in host-id order.
#[tokio::test]
async fn every_allocated_host_is_reported_in_id_order() {
    let (router, _engine) = super::ec2_query::emulator();
    let allocated = seed(&router).await;

    let mut expected = allocated.clone();
    expected.sort();
    let reported = described_ids(&router, "every host", &[]).await;
    assert_eq!(
        reported, expected,
        "every host: expected the whole store in id order, got {reported:?}"
    );
}

/// A host allocated after a call is reported by the next one — the answer is computed from
/// the store, not fixed.
#[tokio::test]
async fn a_host_allocated_after_a_call_is_reported_by_the_next_one() {
    let (router, _engine) = super::ec2_query::emulator();
    let allocated = seed(&router).await;

    let before = described_ids(&router, "before the allocation", &[]).await;
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "AllocateHosts",
        &[
            param("InstanceType", "m6i.large"),
            param("AvailabilityZone", zone_name(0)),
            param("Quantity", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the late allocation must succeed, got {status} with body {body}"
    );
    let late_id = element(&body, "hostIdSet")
        .map(|set| elements(&set, "item"))
        .unwrap_or_default()
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the late allocation must report an id, got {body}"));

    let after = described_ids(&router, "after the allocation", &[]).await;
    let mut expected = allocated;
    expected.push(late_id.clone());
    expected.sort();
    assert_eq!(
        after, expected,
        "after the allocation: expected {late_id:?} to be reported too, got {after:?}"
    );
    assert!(
        !before.contains(&late_id),
        "before the allocation: {late_id:?} did not exist yet, got {before:?}"
    );
}

/// Describing hosts is a REPORT: two identical calls answer the same thing and the store is
/// the same size afterwards, so a read cannot change what a later caller sees.
#[tokio::test]
async fn describing_hosts_changes_nothing_a_later_caller_sees() {
    let (router, engine) = super::ec2_query::emulator();
    let allocated = seed(&router).await;

    let (first_status, first) = describe(&router, &[]).await;
    assert_eq!(
        first_status, 200,
        "first read: expected HTTP 200, got {first_status} with body {first}"
    );
    let (second_status, second) = describe(&router, &[]).await;
    assert_eq!(
        second_status, 200,
        "second read: expected HTTP 200, got {second_status} with body {second}"
    );
    assert_eq!(
        element(&first, "hostSet"),
        element(&second, "hostSet"),
        "a read must not change what a later caller sees: first {first}, second {second}"
    );
    assert_eq!(
        engine
            .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
            .lock()
            .dedicated_hosts
            .len(),
        allocated.len(),
        "describing hosts must not write to the store"
    );
}

/// Every member `AllocateHosts` stored round-trips through the describe: the zone, the
/// settings, the properties, the owner, the CPU options and the tags come back as stored.
#[tokio::test]
async fn an_allocated_host_round_trips_through_the_describe() {
    let (router, _engine) = super::ec2_query::emulator();
    let allocated = seed(&router).await;
    let single_type = ids_at(&allocated, &[0]);
    let family = ids_at(&allocated, &[1]);
    let maintained = ids_at(&allocated, &[5]);

    // The host allocated for ONE instance type.
    let (status, body) = describe(&router, &indexed("HostId", &borrowed(&single_type))).await;
    assert_eq!(
        status, 200,
        "the single-type host: expected HTTP 200, got {status} with body {body}"
    );
    for (name, expected) in [
        ("hostId", single_type[0].clone()),
        ("availabilityZone", zone_name(0)),
        ("availabilityZoneId", zone_id(0)),
        ("autoPlacement", DEFAULT_AUTO_PLACEMENT.to_string()),
        ("hostRecovery", DEFAULT_HOST_RECOVERY.to_string()),
        // A host that supports a single instance type only reports `off`
        // (`Host$AllowsMultipleInstanceTypes`).
        (
            "allowsMultipleInstanceTypes",
            ALLOWS_MULTIPLE_INSTANCE_TYPES_OFF.to_string(),
        ),
        ("state", HOST_STATE_AVAILABLE.to_string()),
        ("clientToken", "token-alpha".to_string()),
        ("ownerId", engine::emulator_account_id().to_string()),
        ("instanceType", "m5.large".to_string()),
        ("instanceFamily", "m5".to_string()),
        ("amdSevSnp", AMD_SEV_SNP_DISABLED.to_string()),
        ("memberOfServiceLinkedResourceGroup", "false".to_string()),
    ] {
        assert_eq!(
            element(&body, name).as_deref(),
            Some(expected.as_str()),
            "the single-type host: expected {name} to be {expected:?}, got {body}"
        );
    }
    assert!(
        element(&body, "allocationTime").is_some(),
        "the single-type host: expected an allocationTime, got {body}"
    );
    // "This returns a null response if the Dedicated Host doesn't have an associated
    // reservation" (`Host$HostReservationId`), and nothing has released this host.
    for absent in ["hostReservationId", "releaseTime", "hostMaintenance"] {
        assert!(
            element(&body, absent).is_none(),
            "the single-type host: expected no {absent}, got {body}"
        );
    }
    assert_eq!(
        elements(&body, "key"),
        vec!["Name".to_string(), "Team".to_string()],
        "the single-type host: expected both tags, got {body}"
    );

    // The host allocated for a whole FAMILY reports no instance type of its own and allows
    // multiple instance types.
    let (status, body) = describe(&router, &indexed("HostId", &borrowed(&family))).await;
    assert_eq!(
        status, 200,
        "the family host: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "instanceFamily").as_deref(),
        Some("m5"),
        "the family host: expected the instance family, got {body}"
    );
    assert_eq!(
        element(&body, "allowsMultipleInstanceTypes").as_deref(),
        Some(ALLOWS_MULTIPLE_INSTANCE_TYPES_ON),
        "the family host: expected allowsMultipleInstanceTypes to be on, got {body}"
    );
    assert!(
        element(&body, "instanceType").is_none(),
        "the family host: \"if the host supports multiple instance types, no instanceType \
         is returned\", got {body}"
    );
    assert_eq!(
        element(&body, "autoPlacement").as_deref(),
        Some(ON),
        "the family host: expected the auto-placement the request asked for, got {body}"
    );

    // The host the request switched host recovery and host maintenance on for.
    let (status, body) = describe(&router, &indexed("HostId", &borrowed(&maintained))).await;
    assert_eq!(
        status, 200,
        "the maintained host: expected HTTP 200, got {status} with body {body}"
    );
    for (name, expected) in [("hostRecovery", ON), ("hostMaintenance", ON)] {
        assert_eq!(
            element(&body, name).as_deref(),
            Some(expected),
            "the maintained host: expected {name} to be {expected:?}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// `HostIds`
// ---------------------------------------------------------------------------

/// `HostIds` narrows the report to the hosts it names, whatever and however many it names.
#[tokio::test]
async fn host_ids_narrow_the_report_to_the_hosts_they_name() {
    let (router, _engine) = super::ec2_query::emulator();
    let allocated = seed(&router).await;

    // Rows: one id, two, the same id twice, a set in reverse order, and every id.
    let cases: Vec<(&str, Vec<usize>)> = vec![
        ("one id", vec![0]),
        ("two ids", vec![0, 3]),
        ("the same id twice", vec![2, 2]),
        ("three ids in reverse", vec![6, 4, 1]),
        ("every id", vec![0, 1, 2, 3, 4, 5, 6]),
    ];
    for (case, positions) in cases {
        let requested = ids_at(&allocated, &positions);
        let mut expected = requested.clone();
        expected.dedup();
        let reported =
            described_ids(&router, case, &indexed("HostId", &borrowed(&requested))).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly the hosts named, got {reported:?}"
        );
    }
}

/// A host id this account holds no Dedicated Host for is refused, not reported as an absent
/// row: to EC2 an id it never issued is not a valid host id, whatever its shape, and the
/// refusal names every such id of the request.
#[tokio::test]
async fn a_host_id_the_account_holds_no_host_for_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let allocated = seed(&router).await;

    // Rows: a well-formed long id and a well-formed short id nothing was allocated under,
    // then ids that are not host ids at all — the wrong prefix, no prefix, a non-hex
    // suffix, a suffix of the wrong length, an uppercase suffix, and free text.
    let cases = [
        "h-0123456789abcdef0",
        "h-0fffffff",
        "i-0123456789abcdef0",
        "0123456789abcdef0",
        "h-zzzzzzzzzzzzzzzzz",
        "h-0123",
        "h-0123456789ABCDEF0",
        "not-a-host-id",
    ];
    for host_id in cases {
        let (status, body) = describe(&router, &indexed("HostId", &[host_id])).await;
        assert_eq!(
            status, 400,
            "case {host_id:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidHostID.Malformed",
            "case {host_id:?}: expected InvalidHostID.Malformed, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert_eq!(
            message,
            format!("The specified Dedicated host IDs ['{host_id}'] are not valid."),
            "case {host_id:?}: expected the refusal to name the id, got {message:?}"
        );
    }

    // A request mixing allocated ids with ids the account holds no host for is refused
    // whole, rather than answering with the hosts it could find, and the refusal names
    // every unresolved id — in the order the request listed them, and no other.
    let known = ids_at(&allocated, &[0, 1]);
    let (status, body) = describe(
        &router,
        &indexed(
            "HostId",
            &[
                known[0].as_str(),
                "h-0fffffff",
                known[1].as_str(),
                "not-a-host-id",
            ],
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "allocated ids beside unresolvable ones: expected HTTP 400, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidHostID.Malformed",
        "allocated ids beside unresolvable ones: expected InvalidHostID.Malformed, got {body}"
    );
    let message = unescaped(&element(&body, "Message").unwrap_or_default());
    assert_eq!(
        message,
        "The specified Dedicated host IDs ['h-0fffffff', 'not-a-host-id'] are not valid.",
        "allocated ids beside unresolvable ones: expected only the unresolved ids named, \
         got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// `Filter`
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute of a host, with multiple values
/// joined by OR and EC2's `*`/`?` wildcards honoured.
#[tokio::test]
async fn each_documented_filter_selects_on_its_own_attribute() {
    let (router, _engine) = super::ec2_query::emulator();
    let allocated = seed(&router).await;
    let zone_b = zone_name(1);
    let zone_a = zone_name(0);
    let zone_c = zone_name(2);
    let zone_prefix = format!("{REGION}*");

    // One row per filter name `DescribeHostsRequest$Filter` documents, plus rows that vary
    // the value form: several values (OR), a wildcard, and a value nothing carries. The
    // `tag:<key>` family is tested too — EC2 supports it for every resource whose describe
    // reports `Tags`.
    let cases: Vec<(&str, Vec<&str>, Vec<usize>)> = vec![
        ("auto-placement", vec![ON], vec![1, 2, 5]),
        (
            "auto-placement",
            vec![DEFAULT_AUTO_PLACEMENT],
            vec![0, 3, 4, 6],
        ),
        ("availability-zone", vec![zone_b.as_str()], vec![2, 3]),
        (
            "availability-zone",
            vec![zone_a.as_str(), zone_c.as_str()],
            vec![0, 1, 4, 5, 6],
        ),
        (
            "availability-zone",
            vec![zone_prefix.as_str()],
            vec![0, 1, 2, 3, 4, 5, 6],
        ),
        ("availability-zone", vec!["eu-west-1a"], vec![]),
        ("client-token", vec!["token-alpha"], vec![0]),
        ("client-token", vec!["token-alpha", "token-beta"], vec![0, 6]),
        // No host has a reservation: `PurchaseHostReservation` is what would give one, and
        // it is still a stub.
        ("host-reservation-id", vec!["hr-0123456789abcdef0"], vec![]),
        // A host allocated for a family reports no instance type, so `instance-type`
        // selects only the hosts allocated for one.
        ("instance-type", vec!["m5.large"], vec![0, 3]),
        ("instance-type", vec!["m5.*"], vec![0, 3]),
        ("instance-type", vec!["c5.xlarge", "r5.metal"], vec![2, 4]),
        ("instance-type", vec!["m5"], vec![]),
        (
            "state",
            vec![HOST_STATE_AVAILABLE],
            vec![0, 1, 2, 3, 4, 5, 6],
        ),
        // Nothing has released a host, so none is in the `released` state.
        ("state", vec![STATE_RELEASED], vec![]),
        // "Filter values are case-sensitive" (`Filter$Values`).
        ("state", vec!["AVAILABLE"], vec![]),
        ("tag-key", vec!["Team"], vec![0, 3]),
        ("tag-key", vec!["Name"], vec![0, 1, 6]),
        ("tag-key", vec!["Owner"], vec![]),
        ("tag:Name", vec!["prod-*"], vec![0, 1]),
        ("tag:Name", vec!["build"], vec![6]),
        ("tag:Team", vec!["payments"], vec![0, 3]),
        // A filter carrying no value at all matches nothing: there is no value for the
        // attribute to match.
        ("state", vec![], vec![]),
    ];
    for (name, values, positions) in cases {
        let case = format!("{name}={values:?}");
        let expected = ids_at(&allocated, &positions);
        let reported = described_ids(&router, &case, &filters(&[(name, &values)])).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One AND-combined case: the filter entries to send, and the allocated positions they must
/// select.
type AndCase<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with AND, so each one can only narrow the set further.
#[tokio::test]
async fn several_filters_are_joined_with_and() {
    let (router, _engine) = super::ec2_query::emulator();
    let allocated = seed(&router).await;
    let zone_a = zone_name(0);
    let zone_b = zone_name(1);
    let zones: [&[&str]; 2] = [&[zone_a.as_str()], &[zone_b.as_str()]];

    let cases: Vec<AndCase<'_>> = vec![
        (
            vec![
                ("availability-zone", zones[1]),
                ("instance-type", &["m5.large"][..]),
            ],
            vec![3],
        ),
        (
            vec![
                ("state", &[HOST_STATE_AVAILABLE][..]),
                ("auto-placement", &[ON][..]),
            ],
            vec![1, 2, 5],
        ),
        (
            vec![
                ("tag-key", &["Name"][..]),
                ("auto-placement", &[DEFAULT_AUTO_PLACEMENT][..]),
                ("availability-zone", zones[0]),
            ],
            vec![0, 6],
        ),
        // Two filters that cannot hold together select nothing.
        (
            vec![
                ("availability-zone", zones[0]),
                ("availability-zone", zones[1]),
            ],
            vec![],
        ),
    ];
    for (entries, positions) in cases {
        let case = format!("{entries:?}");
        let expected = ids_at(&allocated, &positions);
        let reported = described_ids(&router, &case, &filters(&entries)).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused, rather than read as selecting
/// nothing — and filter names are case-sensitive.
#[tokio::test]
async fn an_undocumented_filter_name_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    seed(&router).await;

    // Rows: a filter of another resource's describe, a misspelling, the right name in the
    // wrong case, the modelled member name, a bare `tag:` with no key, and an empty name.
    let cases = [
        "volume-id",
        "auto_placement",
        "Auto-Placement",
        "AvailabilityZone",
        "tag:",
        "",
    ];
    for name in cases {
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(name) || name.is_empty(),
            "case {name:?}: the refusal must name the filter, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every reported host exactly once, in
/// store order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_host_exactly_once_for_each_page_size() {
    let (router, _engine) = super::ec2_query::emulator();
    seed(&router).await;

    let whole = described_ids(&router, "every host", &[]).await;
    assert!(
        whole.len() > 5,
        "the allocated store must hold more hosts than the smallest page, got {whole:?}"
    );

    // Rows: the smallest page this operation serves, one that does not divide the set, one
    // larger than the set, and the largest page it serves.
    for max_results in [5, 6, 50, 500] {
        let case = format!("MaxResults={max_results}");
        let mut params = vec![param("MaxResults", max_results.to_string())];
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "hostId");
            assert!(
                page.len() <= max_results,
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                Some(token) => {
                    params = vec![
                        param("MaxResults", max_results.to_string()),
                        param("NextToken", token.clone()),
                    ];
                }
                None => break,
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: expected every host exactly once in store order, got {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, _engine) = super::ec2_query::emulator();
    let allocated = seed(&router).await;
    let expected = ids_at(&allocated, &[0, 1, 2, 3, 4, 5, 6]);

    let mut params = filters(&[("state", &[HOST_STATE_AVAILABLE])]);
    params.push(param("MaxResults", "5"));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "first page: got {status} with body {body}");
    let first = elements(&body, "hostId");
    assert_eq!(
        first,
        expected[..5].to_vec(),
        "first page: expected the first five available hosts, got {first:?}"
    );
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("first page: expected a NextToken in {body}"));

    let mut next = filters(&[("state", &[HOST_STATE_AVAILABLE])]);
    next.push(param("MaxResults", "5"));
    next.push(param("NextToken", token));
    let second = described_ids(&router, "second page", &next).await;
    assert_eq!(
        second,
        expected[5..].to_vec(),
        "second page: expected the remaining available hosts, got {second:?}"
    );
}

/// `MaxResults` is accepted at each published bound and refused one past it: "This value can
/// be between 5 and 500" (`DescribeHostsRequest$MaxResults`).
#[tokio::test]
async fn max_results_is_accepted_at_its_published_bounds_and_refused_past_them() {
    let (router, _engine) = super::ec2_query::emulator();
    seed(&router).await;

    let cases = [(0, 400), (4, 400), (5, 200), (500, 200), (501, 400)];
    for (max_results, expected_status) in cases {
        let (status, body) =
            describe(&router, &[param("MaxResults", max_results.to_string())]).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case MaxResults={max_results}: expected InvalidParameterValue, got {body}"
            );
        }
    }
}

/// "You cannot specify this parameter and the host IDs parameter in the same request"
/// (`DescribeHostsRequest$MaxResults`).
#[tokio::test]
async fn max_results_cannot_be_given_with_host_ids() {
    let (router, _engine) = super::ec2_query::emulator();
    let allocated = seed(&router).await;
    let known = ids_at(&allocated, &[0, 1]);

    // Rows: one id and two, each with a page size inside the accepted range.
    for ids in [&known[..1], &known[..2]] {
        let mut params = indexed("HostId", &borrowed(ids));
        params.push(param("MaxResults", "5"));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {ids:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterCombination",
            "case {ids:?}: expected InvalidParameterCombination, got {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`, rather
/// than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let allocated = seed(&router).await;
    let known = ids_at(&allocated, &[0]);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, that id hex-encoded, and a forged token
    // carrying the marker but not a matching checksum.
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        known[0].clone(),
        ::hex::encode(&known[0]),
        ::hex::encode(format!("ec2-1:0000000000000000:{}", known[0])),
    ];
    for token in cases {
        let (status, body) = describe(&router, &[param("NextToken", token.clone())]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}
