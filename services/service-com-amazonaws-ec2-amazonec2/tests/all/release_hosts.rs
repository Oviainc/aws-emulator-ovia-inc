// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ReleaseHosts`, driven over the wire the service actually speaks:
//! `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=ReleaseHosts`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation GIVES BACK THE HOSTS IT NAMES: every id of the
//! request that names a host the account still holds moves that host to the `released`
//! state with the call's instant as its release time and is reported in `Successful`, the
//! host's record is kept so a describe still reports it, and every other well-formed id is
//! reported in `Unsuccessful` with the code this service answers that cause with, leaving
//! the hosts beside it alone.
//!
//! Every host these tests release was allocated through `AllocateHosts` over the same wire
//! and is read back through `DescribeHosts`, so the operations are checked against each
//! other rather than against a hand-written record — and each table row is a distinct input
//! along what the rule generalises over: which ids, how many, which host state they are in,
//! and whether the id names a host at all.

use std::sync::Arc;

use ec2::ops::types::{ALLOCATION_STATE_VALUES, HostInstance};
use ec2::state::{
    DEDICATED_HOST_TAGGABLE_RESOURCE, HOST_STATE_AVAILABLE, HOST_STATE_RELEASED,
    HOST_STATE_RELEASED_PERMANENT_FAILURE, INCORRECT_STATE, INVALID_HOST_ID_MALFORMED,
    availability_zones_in_region,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, elements, indexed, unescaped};

/// The Region every call here is signed for; the zones the hosts are allocated into are
/// derived from it.
const REGION: &str = "us-east-1";

/// The code an `Unsuccessful` item carries for a Dedicated Host id this operation could not
/// resolve: the one code this service answers an unresolvable host id with
/// (`ec2::state::INVALID_HOST_ID_MALFORMED`, what `DescribeHosts` refuses the same id with).
const UNRESOLVABLE_HOST_ID_CODE: &str = INVALID_HOST_ID_MALFORMED;

/// One `(key, value)` request parameter.
fn param(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The name of the `index`th Availability Zone of the signed Region.
fn zone_name(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_name.clone()
}

/// How the hosts these tests release are allocated. The rows differ along everything a
/// release has to be indifferent to: whether the host was allocated for ONE instance type
/// or for a whole instance family, which zone it sits in, and whether it carries tags and a
/// client token.
fn allocations() -> Vec<Vec<(String, String)>> {
    vec![
        // One instance type, a client token and two tags.
        vec![
            param("InstanceType", "m5.large"),
            param("AvailabilityZone", zone_name(0)),
            param("Quantity", "1"),
            param("ClientToken", "release-token-alpha"),
            param("TagSpecification.1.ResourceType", DEDICATED_HOST_TAGGABLE_RESOURCE),
            param("TagSpecification.1.Tag.1.Key", "Name"),
            param("TagSpecification.1.Tag.1.Value", "prod-a"),
            param("TagSpecification.1.Tag.2.Key", "Team"),
            param("TagSpecification.1.Tag.2.Value", "payments"),
        ],
        // A whole instance family, auto-placement on, no tags.
        vec![
            param("InstanceFamily", "m5"),
            param("AvailabilityZone", zone_name(0)),
            param("Quantity", "1"),
            param("AutoPlacement", "on"),
        ],
        // Another zone and another instance type.
        vec![
            param("InstanceType", "c5.xlarge"),
            param("AvailabilityZone", zone_name(1)),
            param("Quantity", "1"),
        ],
        // A bare-metal host in a third zone, with host recovery and maintenance on.
        vec![
            param("InstanceType", "r5.metal"),
            param("AvailabilityZone", zone_name(2)),
            param("Quantity", "1"),
            param("HostRecovery", "on"),
            param("HostMaintenance", "on"),
        ],
        // A Mac host, allocated for one instance type in the first zone.
        vec![
            param("InstanceType", "mac1.metal"),
            param("AvailabilityZone", zone_name(0)),
            param("Quantity", "1"),
            param("ClientToken", "release-token-beta"),
        ],
    ]
}

/// An emulator whose store holds the hosts of [`allocations`], allocated over the wire, with
/// the engine behind it and their ids in allocation order.
async fn seeded() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Vec<String>) {
    let (router, engine) = super::ec2_query::emulator();
    let mut ids = Vec::new();
    for (position, params) in allocations().into_iter().enumerate() {
        let (status, body) = super::ec2_query::call(&router, REGION, "AllocateHosts", &params).await;
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
    (router, engine, ids)
}

/// One `ReleaseHosts` call naming `host_ids`, as its HTTP status and XML body.
async fn release(router: &MultiServiceRouter, host_ids: &[String]) -> (u16, String) {
    let named: Vec<&str> = host_ids.iter().map(String::as_str).collect();
    super::ec2_query::call(router, REGION, "ReleaseHosts", &indexed("HostId", &named)).await
}

/// The ids a response reports in `Successful`, in the order it reported them.
fn successful(body: &str) -> Vec<String> {
    element(body, "successful")
        .map(|set| elements(&set, "item"))
        .unwrap_or_default()
}

/// The `(resourceId, code, message)` of every `Unsuccessful` item, in the order reported.
///
/// An item of a structure list reaches the wire as `<item xmlns="…">`, so the items are read
/// member by member — one `resourceId`, one `code` and one `message` per item, in item order
/// — rather than by slicing on the item element itself.
fn unsuccessful(body: &str) -> Vec<(String, String, String)> {
    let Some(set) = element(body, "unsuccessful") else {
        return Vec::new();
    };
    let resources = elements(&set, "resourceId");
    let codes = elements(&set, "code");
    let messages = elements(&set, "message");
    assert!(
        resources.len() == codes.len() && codes.len() == messages.len(),
        "every unsuccessful item must carry a resourceId, a code and a message, got {set}"
    );
    resources
        .into_iter()
        .zip(codes)
        .zip(messages)
        .map(|((resource, code), message)| (resource, code, unescaped(&message)))
        .collect()
}

/// The `(state, releaseTime)` `DescribeHosts` reports for one host, or `None` when the
/// describe reports no host for the id at all.
async fn described(router: &MultiServiceRouter, host_id: &str) -> Option<(String, Option<String>)> {
    let (status, body) =
        super::ec2_query::call(router, REGION, "DescribeHosts", &indexed("HostId", &[host_id]))
            .await;
    assert_eq!(
        status, 200,
        "describing {host_id:?}: expected HTTP 200, got {status} with body {body}"
    );
    // The describe names ONE host, so the response carries one `hostId` and the members
    // below are that host's — read off the body rather than out of a nested element, which
    // the tag items would close early.
    element(&body, "hostId")?;
    Some((
        element(&body, "state").unwrap_or_default(),
        element(&body, "releaseTime"),
    ))
}

/// A well-formed Dedicated Host id no `AllocateHosts` call of these tests minted, in the
/// long (17-digit) id form.
const UNKNOWN_LONG_ID: &str = "h-0fedcba987654321f";

/// The same, in the short (8-digit) id form — both forms this service accepts
/// (`h-([0-9a-f]{8}|[0-9a-f]{17})`).
const UNKNOWN_SHORT_ID: &str = "h-0fedcba9";

// ---------------------------------------------------------------------------
// What a release does to the store
// ---------------------------------------------------------------------------

/// Whichever subset of the account's hosts a call names, exactly those hosts are released
/// and every other host is left as it was — the answer is computed from the store, not
/// fixed.
#[tokio::test]
async fn exactly_the_named_hosts_are_released() {
    // Rows: one host, a pair, the whole set, and the same host named twice (one id names one
    // host, so the repeat releases it once and is reported once).
    let cases: [(&str, &[usize], &[usize]); 4] = [
        ("one host", &[0], &[0]),
        ("a pair", &[1, 3], &[1, 3]),
        ("every host", &[0, 1, 2, 3, 4], &[0, 1, 2, 3, 4]),
        ("one host named twice", &[2, 2], &[2]),
    ];
    for (case, named, expected) in cases {
        let (router, _engine, allocated) = seeded().await;
        let requested: Vec<String> = named.iter().map(|at| allocated[*at].clone()).collect();
        let expected_ids: Vec<String> = expected.iter().map(|at| allocated[*at].clone()).collect();

        let (status, body) = release(&router, &requested).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            successful(&body),
            expected_ids,
            "case {case:?}: expected {expected_ids:?} released, got {body}"
        );
        assert!(
            unsuccessful(&body).is_empty(),
            "case {case:?}: expected no unsuccessful item, got {:?}",
            unsuccessful(&body)
        );

        for (position, host_id) in allocated.iter().enumerate() {
            let released = expected.contains(&position);
            let (state, release_time) = described(&router, host_id)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: host {position} must still be described"));
            let expected_state = if released {
                HOST_STATE_RELEASED
            } else {
                HOST_STATE_AVAILABLE
            };
            assert_eq!(
                state, expected_state,
                "case {case:?}: host {position} expected state {expected_state:?}, got {state:?}"
            );
            assert_eq!(
                release_time.is_some(),
                released,
                "case {case:?}: host {position} expected a releaseTime only once released, \
                 got {release_time:?}"
            );
        }
    }
}

/// A released host keeps every member it was allocated with and is still reported by
/// `DescribeHosts` — "released hosts still appear in a DescribeHosts response" — with a
/// release time no earlier than its allocation time.
#[tokio::test]
async fn a_released_host_is_still_described_with_everything_it_was_allocated_with() {
    let (router, _engine, allocated) = seeded().await;
    let host_id = allocated[0].clone();

    let (status, body) = release(&router, ::std::slice::from_ref(&host_id)).await;
    assert_eq!(
        status, 200,
        "releasing {host_id:?}: expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "DescribeHosts",
        &indexed("HostId", &[host_id.as_str()]),
    )
    .await;
    assert_eq!(
        status, 200,
        "describing the released host: expected HTTP 200, got {status} with body {body}"
    );
    for (name, expected) in [
        ("hostId", host_id.clone()),
        ("state", HOST_STATE_RELEASED.to_string()),
        ("availabilityZone", zone_name(0)),
        ("instanceType", "m5.large".to_string()),
        ("clientToken", "release-token-alpha".to_string()),
    ] {
        assert_eq!(
            element(&body, name).as_deref(),
            Some(expected.as_str()),
            "the released host: expected {name} to be {expected:?}, got {body}"
        );
    }
    assert_eq!(
        elements(&body, "key"),
        vec!["Name".to_string(), "Team".to_string()],
        "the released host: expected both tags to survive the release, got {body}"
    );

    let allocation_time = element(&body, "allocationTime")
        .and_then(|at| as_timestamp(&at))
        .unwrap_or_else(|| panic!("the released host: expected an allocationTime, got {body}"));
    let release_time = element(&body, "releaseTime")
        .and_then(|at| as_timestamp(&at))
        .unwrap_or_else(|| panic!("the released host: expected a releaseTime, got {body}"));
    assert!(
        release_time >= allocation_time,
        "the released host: expected a releaseTime at or after the allocationTime \
         {allocation_time}, got {release_time}"
    );
}

/// Releasing one host twice moves it once: the second call reports it unsuccessful and the
/// release time the first call recorded does not change.
#[tokio::test]
async fn releasing_a_host_twice_moves_it_once() {
    let (router, _engine, allocated) = seeded().await;
    let host_id = allocated[4].clone();

    let (status, body) = release(&router, ::std::slice::from_ref(&host_id)).await;
    assert_eq!(
        status, 200,
        "the first release: expected HTTP 200, got {status} with body {body}"
    );
    let first = described(&router, &host_id)
        .await
        .unwrap_or_else(|| panic!("the first release: the host must still be described"));

    let (status, body) = release(&router, ::std::slice::from_ref(&host_id)).await;
    assert_eq!(
        status, 200,
        "the second release: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        successful(&body).is_empty(),
        "the second release: expected nothing released, got {body}"
    );
    assert_eq!(
        unsuccessful(&body)
            .iter()
            .map(|(resource, code, _)| (resource.clone(), code.clone()))
            .collect::<Vec<_>>(),
        vec![(host_id.clone(), UNRESOLVABLE_HOST_ID_CODE.to_string())],
        "the second release: expected {host_id:?} reported as no longer releasable, got {body}"
    );

    let second = described(&router, &host_id)
        .await
        .unwrap_or_else(|| panic!("the second release: the host must still be described"));
    assert_eq!(
        second, first,
        "the second release: expected the host unchanged, got {second:?} after {first:?}"
    );
}

// ---------------------------------------------------------------------------
// What is reported per id
// ---------------------------------------------------------------------------

/// Which seeded host, or which literal id, one row of the table below names.
#[derive(Clone, Copy, Debug)]
enum Named {
    /// The `n`th host of [`allocations`], in allocation order.
    Seeded(usize),
    /// An id written out, which no host of the store carries.
    Literal(&'static str),
}

impl Named {
    /// This entry as the id a request carries.
    fn resolve(self, allocated: &[String]) -> String {
        match self {
            Named::Seeded(at) => allocated[at].clone(),
            Named::Literal(id) => id.to_string(),
        }
    }
}

/// One row of the per-id table below: the case name, the seeded hosts released BEFORE the
/// call under test, the ids that call names, and the whole answer expected, in order — a row
/// whose code is `None` is a release.
type PerIdCase = (
    &'static str,
    &'static [usize],
    &'static [Named],
    &'static [(Named, Option<&'static str>)],
);

/// Each named host is judged on its own: a host the account holds is released, and every
/// other well-formed id is reported in `Unsuccessful` with the code this service answers
/// that cause with — one bad id never refuses the hosts beside it.
#[tokio::test]
async fn each_named_id_is_judged_on_its_own() {
    // Rows: an id the store holds no host for, in both id forms; a host released before the
    // call; and each of those mixed with hosts that do release, so a failure is shown not to
    // stop the batch. `expected` is the whole answer, in order: `None` is a release.
    let cases: [PerIdCase; 6] = [
        (
            "an unknown long id",
            &[],
            &[Named::Literal(UNKNOWN_LONG_ID)],
            &[(
                Named::Literal(UNKNOWN_LONG_ID),
                Some(UNRESOLVABLE_HOST_ID_CODE),
            )],
        ),
        (
            "an unknown short id",
            &[],
            &[Named::Literal(UNKNOWN_SHORT_ID)],
            &[(
                Named::Literal(UNKNOWN_SHORT_ID),
                Some(UNRESOLVABLE_HOST_ID_CODE),
            )],
        ),
        (
            "a host already released",
            &[1],
            &[Named::Seeded(1)],
            &[(Named::Seeded(1), Some(UNRESOLVABLE_HOST_ID_CODE))],
        ),
        (
            "a held host beside an unknown id",
            &[],
            &[Named::Seeded(0), Named::Literal(UNKNOWN_LONG_ID)],
            &[
                (Named::Seeded(0), None),
                (
                    Named::Literal(UNKNOWN_LONG_ID),
                    Some(UNRESOLVABLE_HOST_ID_CODE),
                ),
            ],
        ),
        (
            "an unknown id before two held hosts",
            &[],
            &[
                Named::Literal(UNKNOWN_SHORT_ID),
                Named::Seeded(2),
                Named::Seeded(3),
            ],
            &[
                (
                    Named::Literal(UNKNOWN_SHORT_ID),
                    Some(UNRESOLVABLE_HOST_ID_CODE),
                ),
                (Named::Seeded(2), None),
                (Named::Seeded(3), None),
            ],
        ),
        (
            "a released host beside a held one",
            &[4],
            &[Named::Seeded(4), Named::Seeded(0)],
            &[
                (Named::Seeded(4), Some(UNRESOLVABLE_HOST_ID_CODE)),
                (Named::Seeded(0), None),
            ],
        ),
    ];

    for (case, pre_released, named, expected) in cases {
        let (router, _engine, allocated) = seeded().await;
        if !pre_released.is_empty() {
            let ids: Vec<String> = pre_released.iter().map(|at| allocated[*at].clone()).collect();
            let (status, body) = release(&router, &ids).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the preparing release must succeed, got {status} with body {body}"
            );
        }

        let requested: Vec<String> = named.iter().map(|entry| entry.resolve(&allocated)).collect();
        let (status, body) = release(&router, &requested).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let expected_successful: Vec<String> = expected
            .iter()
            .filter(|(_, code)| code.is_none())
            .map(|(entry, _)| entry.resolve(&allocated))
            .collect();
        let expected_unsuccessful: Vec<(String, String)> = expected
            .iter()
            .filter_map(|(entry, code)| {
                code.map(|code| (entry.resolve(&allocated), code.to_string()))
            })
            .collect();
        assert_eq!(
            successful(&body),
            expected_successful,
            "case {case:?}: expected {expected_successful:?} released, got {body}"
        );
        let reported: Vec<(String, String)> = unsuccessful(&body)
            .into_iter()
            .map(|(resource, code, message)| {
                assert!(
                    message.contains(&resource),
                    "case {case:?}: an unsuccessful item must name its host, got {message:?}"
                );
                (resource, code)
            })
            .collect();
        assert_eq!(
            reported, expected_unsuccessful,
            "case {case:?}: expected {expected_unsuccessful:?} reported unsuccessful, got {body}"
        );

        // The hosts the call did NOT release are untouched, whatever the row asked for.
        for (position, host_id) in allocated.iter().enumerate() {
            let should_be_released = pre_released.contains(&position)
                || expected_successful.contains(host_id);
            let (state, _) = described(&router, host_id)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: host {position} must still be described"));
            let expected_state = if should_be_released {
                HOST_STATE_RELEASED
            } else {
                HOST_STATE_AVAILABLE
            };
            assert_eq!(
                state, expected_state,
                "case {case:?}: host {position} expected state {expected_state:?}, got {state:?}"
            );
        }
    }
}

/// "You must stop or terminate all instances on a host before it can be released": a host
/// still running instances is reported in `Unsuccessful` and stays as it was, however many
/// instances it runs.
///
/// No operation of this service launches onto a Dedicated Host yet, so the instances are put
/// on the stored host directly, through the same engine the service reads.
#[tokio::test]
async fn a_host_still_running_instances_is_not_released() {
    // Rows: one instance on the host and two, against a host with no instance at all.
    for running in [1usize, 2] {
        let case = format!("{running} instance(s)");
        let (router, engine, allocated) = seeded().await;
        let occupied = allocated[2].clone();
        let free = allocated[3].clone();
        {
            let state = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
            let mut store = state.lock();
            let host = store
                .dedicated_hosts
                .get_mut(&occupied)
                .unwrap_or_else(|| panic!("case {case:?}: host {occupied:?} must be stored"));
            host.instances = Some(
                (0..running)
                    .map(|index| HostInstance {
                        instance_id: Some(format!("i-000000000000000{index}a")),
                        instance_type: Some("c5.xlarge".to_string()),
                        owner_id: Some(engine::emulator_account_id().to_string()),
                    })
                    .collect(),
            );
        }

        let (status, body) = release(&router, &[occupied.clone(), free.clone()]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            successful(&body),
            vec![free.clone()],
            "case {case:?}: expected only the empty host released, got {body}"
        );
        assert_eq!(
            unsuccessful(&body)
                .iter()
                .map(|(resource, code, _)| (resource.clone(), code.clone()))
                .collect::<Vec<_>>(),
            vec![(occupied.clone(), INCORRECT_STATE.to_string())],
            "case {case:?}: expected the occupied host reported unsuccessful, got {body}"
        );
        let (state, release_time) = described(&router, &occupied)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the occupied host must still be described"));
        assert_eq!(
            state, HOST_STATE_AVAILABLE,
            "case {case:?}: the occupied host must stay as it was, got {state:?}"
        );
        assert!(
            release_time.is_none(),
            "case {case:?}: the occupied host must carry no releaseTime, got {release_time:?}"
        );
    }
}

/// The states a release reads and writes are members of the modelled `AllocationState` enum
/// (`ec2::ops::types::ALLOCATION_STATE_VALUES`, the generated catalog), so neither is a
/// spelling the model does not publish.
#[test]
fn the_released_states_are_modelled_members() {
    for state in [
        HOST_STATE_AVAILABLE,
        HOST_STATE_RELEASED,
        HOST_STATE_RELEASED_PERMANENT_FAILURE,
    ] {
        assert!(
            ALLOCATION_STATE_VALUES.contains(&state),
            "case {state:?}: expected a member of {ALLOCATION_STATE_VALUES:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What an id this account holds no host for is reported as
// ---------------------------------------------------------------------------

/// An id the account holds no host for is reported in `Unsuccessful` as the id it is —
/// whatever its shape — while the hosts named beside it still release: to EC2 a host id it
/// never issued is not valid rather than merely absent, and this operation reports that per
/// id instead of refusing the whole batch.
#[tokio::test]
async fn an_id_naming_no_host_of_the_account_is_reported_per_id() {
    // Rows: free text, the prefix alone, the prefix with non-hexadecimal digits, one digit
    // short of each accepted length, one digit past the long form, an uppercase suffix and
    // another resource's id — every way of missing `h-([0-9a-f]{8}|[0-9a-f]{17})` — then the
    // two well-formed id lengths this service does issue, for hosts this account has none
    // of.
    let cases = [
        "not-an-id",
        "h-",
        "h-zzzzzzzz",
        "h-0123456",
        "h-0123456789abcdef",
        "h-0123456789abcdef01",
        "h-0123456789ABCDEF0",
        "i-0123456789abcdef0",
        UNKNOWN_SHORT_ID,
        UNKNOWN_LONG_ID,
    ];
    for unresolvable in cases {
        let (router, _engine, allocated) = seeded().await;
        let held = allocated[0].clone();

        let (status, body) = release(&router, &[held.clone(), unresolvable.to_string()]).await;
        assert_eq!(
            status, 200,
            "case {unresolvable:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            successful(&body),
            vec![held.clone()],
            "case {unresolvable:?}: expected the held host released, got {body}"
        );
        assert_eq!(
            unsuccessful(&body),
            vec![(
                unresolvable.to_string(),
                UNRESOLVABLE_HOST_ID_CODE.to_string(),
                // The wording this service answers an unresolvable Dedicated Host id with,
                // naming the id the request carried.
                format!("The specified Dedicated host IDs ['{unresolvable}'] are not valid.")
            )],
            "case {unresolvable:?}: expected the id reported unsuccessful, got {body}"
        );
        let (state, _) = described(&router, &held)
            .await
            .unwrap_or_else(|| panic!("case {unresolvable:?}: the host must still be described"));
        assert_eq!(
            state, HOST_STATE_RELEASED,
            "case {unresolvable:?}: the held host must still have been released, got {state:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the request itself is refused for
// ---------------------------------------------------------------------------

/// "The IDs of the Dedicated Hosts to release" (`ReleaseHostsRequest$HostIds`) is a REQUIRED
/// member, so a request naming no host is refused by the modelled constraint and releases
/// nothing — never answered with two empty sets.
#[tokio::test]
async fn a_request_naming_no_host_is_refused() {
    let (router, _engine, allocated) = seeded().await;

    let (status, body) = release(&router, &[]).await;
    assert_eq!(
        status, 400,
        "no host id: expected HTTP 400, got {status} with body {body}"
    );
    assert!(
        unescaped(&element(&body, "Message").unwrap_or_default()).contains("hostIds"),
        "no host id: the refusal must name the required member, got {body}"
    );
    let (state, _) = described(&router, &allocated[0])
        .await
        .unwrap_or_else(|| panic!("no host id: the host must still be described"));
    assert_eq!(
        state, HOST_STATE_AVAILABLE,
        "no host id: a refused request must release nothing, got {state:?}"
    );
}
