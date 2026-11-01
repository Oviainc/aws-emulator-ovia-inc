// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyHosts`, driven over the wire the service actually speaks:
//! `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=ModifyHosts`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation APPLIES THE SETTINGS IT NAMES TO THE HOSTS IT
//! NAMES: every setting the request carries is stored on each named host the account still
//! holds and is reported by a later describe of it, every member the request leaves out is
//! left as it was, the changed ids come back in `Successful`, and a host the setting cannot
//! be applied to is reported in `Unsuccessful` with the code this service answers that cause
//! with — leaving the hosts beside it modified.
//!
//! Every host these tests modify was allocated through `AllocateHosts` over the same wire and
//! is read back through `DescribeHosts`, so the operations are checked against each other
//! rather than against a hand-written record — and each table row is a distinct input along
//! what the rule generalises over: which setting, which of its modelled values, how many
//! settings at once, which host, how many ids, and whether the id names a host the setting
//! can be applied to at all.

use std::sync::Arc;

use ec2::ops::types::{AUTO_PLACEMENT_VALUES, HOST_MAINTENANCE_VALUES, HOST_RECOVERY_VALUES};
use ec2::state::{
    ALLOWS_MULTIPLE_INSTANCE_TYPES_OFF, ALLOWS_MULTIPLE_INSTANCE_TYPES_ON, INVALID_HOST_CONFIGURATION,
    INVALID_HOST_ID_MALFORMED, INVALID_PARAMETER_VALUE, INVALID_REQUEST, MAX_RESOURCE_IDS,
    MISSING_PARAMETER, availability_zones_in_region, unresolvable_host_ids_message,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed, unescaped};

/// The Region every call here is signed for; the zones the hosts are allocated into are
/// derived from it.
const REGION: &str = "us-east-1";

/// One `(key, value)` request parameter.
fn param(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One setting a `ModifyHosts` call asks for, as the `(parameter, value)` pair it carries.
type Setting<'a> = (&'a str, &'a str);

/// One member a modification must change, as `(member, text)` — `None` for a member the
/// describe must then leave out.
type ExpectedChange = (&'static str, Option<&'static str>);

/// One row of [`the_settings_a_request_names_are_applied_and_nothing_else_moves`]: the case
/// name, which seeded host it modifies, the settings it asks for, and the members that must
/// change.
type ApplyCase<'a> = (&'a str, usize, &'a [Setting<'a>], &'a [ExpectedChange]);

/// One row of [`narrowing_then_widening_the_supported_instance_types_round_trips`]: the case
/// name, which seeded host it modifies, and the two modifications in order.
type RoundTripCase<'a> = (&'a str, usize, [Setting<'a>; 2]);

/// One row of
/// [`an_instance_family_the_host_is_not_configured_for_is_reported_unsuccessful`].
struct ForeignFamilyCase<'a> {
    /// What the row is, for every assertion message.
    case: &'a str,
    /// Which seeded host the modification names.
    target: usize,
    /// The settings the call asks for, one of which names another instance family.
    settings: &'a [Setting<'a>],
    /// The value that names that other family.
    value: &'a str,
    /// The member it arrived in, in its wire spelling.
    member: &'a str,
    /// The instance family the host is configured for.
    family: &'a str,
}

/// The name of the `index`th Availability Zone of the signed Region.
fn zone_name(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_name.clone()
}

/// How the hosts these tests modify are allocated. The rows differ along everything a
/// modification has to read off the host it is applied to: whether the host was allocated for
/// ONE instance type or for a whole instance family, which family that is, which zone it sits
/// in, and which of the settings this operation changes it already carries.
fn allocations() -> Vec<Vec<(String, String)>> {
    vec![
        // 0: one general-purpose instance type, every setting at its default.
        vec![
            param("InstanceType", "m5.large"),
            param("AvailabilityZone", zone_name(0)),
            param("Quantity", "1"),
            param("ClientToken", "modify-token-alpha"),
        ],
        // 1: a whole compute-optimized family, auto-placement already on.
        vec![
            param("InstanceFamily", "c5"),
            param("AvailabilityZone", zone_name(1)),
            param("Quantity", "1"),
            param("AutoPlacement", "on"),
        ],
        // 2: a Mac host — one instance type of an Apple-hardware family.
        vec![
            param("InstanceType", "mac1.metal"),
            param("AvailabilityZone", zone_name(0)),
            param("Quantity", "1"),
        ],
        // 3: a bare-metal host that already has host recovery and host maintenance on.
        vec![
            param("InstanceType", "r5.metal"),
            param("AvailabilityZone", zone_name(2)),
            param("Quantity", "1"),
            param("HostRecovery", "on"),
            param("HostMaintenance", "on"),
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

/// One `ModifyHosts` call naming `host_ids` and asking for `settings`, as its HTTP status and
/// XML body.
async fn modify(
    router: &MultiServiceRouter,
    host_ids: &[&str],
    settings: &[(&str, &str)],
) -> (u16, String) {
    let mut params = indexed("HostId", host_ids);
    for (name, value) in settings {
        params.push(param(name, *value));
    }
    super::ec2_query::call(router, REGION, "ModifyHosts", &params).await
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

/// Every member one host's report carries that a modification could touch, plus the ones it
/// must never touch, from the modelled `Host` shape
/// (`smithy_query(query="shape",shape="Host")`) and its `HostProperties`.
const REPORTED_MEMBERS: [&str; 13] = [
    "hostId",
    "availabilityZone",
    "availabilityZoneId",
    "state",
    "instanceType",
    "instanceFamily",
    "allowsMultipleInstanceTypes",
    "autoPlacement",
    "hostRecovery",
    "hostMaintenance",
    "amdSevSnp",
    "clientToken",
    "ownerId",
];

/// What `DescribeHosts` reports for one host, as `(member, text)` pairs — `None` for a member
/// the report leaves out.
///
/// ONE describe of ONE host, so the pairs an assertion compares cannot come from two different
/// reports of it.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    host_id: &str,
) -> Vec<(&'static str, Option<String>)> {
    let (status, body) =
        super::ec2_query::call(router, REGION, "DescribeHosts", &indexed("HostId", &[host_id]))
            .await;
    assert_eq!(
        status, 200,
        "case {case:?}: describing {host_id} expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, "hostId");
    assert_eq!(
        reported,
        vec![host_id.to_string()],
        "case {case:?}: expected the describe to report only {host_id}, got {reported:?}"
    );
    REPORTED_MEMBERS
        .into_iter()
        .map(|name| (name, element(&body, name)))
        .collect()
}

/// The reports of every seeded host, in allocation order.
async fn described_all(
    router: &MultiServiceRouter,
    case: &str,
    host_ids: &[String],
) -> Vec<Vec<(&'static str, Option<String>)>> {
    let mut reports = Vec::with_capacity(host_ids.len());
    for host_id in host_ids {
        reports.push(described(router, case, host_id).await);
    }
    reports
}

/// `report` with each `(member, value)` of `changes` overridden: what a host that took exactly
/// the modification under test must report.
fn with_changes(
    report: &[(&'static str, Option<String>)],
    changes: &[ExpectedChange],
) -> Vec<(&'static str, Option<String>)> {
    report
        .iter()
        .map(|(member, value)| {
            match changes.iter().find(|(changed, _)| changed == member) {
                Some((_, expected)) => (*member, expected.map(str::to_string)),
                None => (*member, value.clone()),
            }
        })
        .collect()
}

/// A well-formed Dedicated Host id no `AllocateHosts` call of these tests minted, in the long
/// (17-digit) and the short (8-digit) id form — both forms this service accepts
/// (`h-([0-9a-f]{8}|[0-9a-f]{17})`).
const UNKNOWN_LONG_ID: &str = "h-0fedcba987654321f";
const UNKNOWN_SHORT_ID: &str = "h-0fedcba9";

// ---------------------------------------------------------------------------
// What a modification does to the store
// ---------------------------------------------------------------------------

/// Whichever settings a call names are stored on the host it names, every other member of that
/// host is left as it was, and no other host moves — the answer is computed from the store, not
/// fixed.
#[tokio::test]
async fn the_settings_a_request_names_are_applied_and_nothing_else_moves() {
    // Rows: each setting on its own, in both directions, on hosts that already carry the
    // other value or none at all; several settings at once; and the two ways of saying which
    // instance types a host supports.
    let cases: [ApplyCase; 11] = [
        // Auto-placement moves both ways: on a host allocated with the default `off` and on
        // one allocated with `on`.
        (
            "auto-placement on",
            0,
            &[("AutoPlacement", "on")],
            &[("autoPlacement", Some("on"))],
        ),
        (
            "auto-placement off",
            1,
            &[("AutoPlacement", "off")],
            &[("autoPlacement", Some("off"))],
        ),
        (
            "auto-placement on again",
            1,
            &[("AutoPlacement", "on")],
            &[("autoPlacement", Some("on"))],
        ),
        // Host recovery, on a host that carries the default and on one that already has it on.
        (
            "host recovery on",
            0,
            &[("HostRecovery", "on")],
            &[("hostRecovery", Some("on"))],
        ),
        (
            "host recovery off",
            3,
            &[("HostRecovery", "off")],
            &[("hostRecovery", Some("off"))],
        ),
        // Host maintenance, on a host that was allocated without it at all and on one that
        // already has it on.
        (
            "host maintenance on",
            0,
            &[("HostMaintenance", "on")],
            &[("hostMaintenance", Some("on"))],
        ),
        (
            "host maintenance off",
            3,
            &[("HostMaintenance", "off")],
            &[("hostMaintenance", Some("off"))],
        ),
        // Three settings in one call, on the Mac host.
        (
            "every setting at once",
            2,
            &[
                ("AutoPlacement", "on"),
                ("HostRecovery", "on"),
                ("HostMaintenance", "on"),
            ],
            &[
                ("autoPlacement", Some("on")),
                ("hostRecovery", Some("on")),
                ("hostMaintenance", Some("on")),
            ],
        ),
        // "Specify this parameter to modify a Dedicated Host to support multiple instance
        // types within its current instance family" (`ModifyHostsRequest$InstanceFamily`):
        // the host stops reporting one instance type and allows the whole family.
        (
            "the whole instance family",
            0,
            &[("InstanceFamily", "m5")],
            &[
                ("instanceType", None),
                ("instanceFamily", Some("m5")),
                (
                    "allowsMultipleInstanceTypes",
                    Some(ALLOWS_MULTIPLE_INSTANCE_TYPES_ON),
                ),
            ],
        ),
        // "Specify this parameter to modify a Dedicated Host to support only a specific
        // instance type" (`$InstanceType`): the host that supported a whole family now
        // supports one type of it.
        (
            "one instance type of the family",
            1,
            &[("InstanceType", "c5.xlarge")],
            &[
                ("instanceType", Some("c5.xlarge")),
                ("instanceFamily", Some("c5")),
                (
                    "allowsMultipleInstanceTypes",
                    Some(ALLOWS_MULTIPLE_INSTANCE_TYPES_OFF),
                ),
            ],
        ),
        // The family a host already supports: the modification is served and changes nothing.
        (
            "the family the host already supports",
            1,
            &[("InstanceFamily", "c5")],
            &[],
        ),
    ];

    for (case, target, settings, changes) in cases {
        let (router, _engine, allocated) = seeded().await;
        let before = described_all(&router, case, &allocated).await;

        let (status, body) = modify(&router, &[allocated[target].as_str()], settings).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            successful(&body),
            vec![allocated[target].clone()],
            "case {case:?}: expected {:?} modified, got {body}",
            allocated[target]
        );
        assert!(
            unsuccessful(&body).is_empty(),
            "case {case:?}: expected no unsuccessful item, got {:?}",
            unsuccessful(&body)
        );

        for (position, host_id) in allocated.iter().enumerate() {
            let actual = described(&router, case, host_id).await;
            let expected = if position == target {
                with_changes(&before[position], changes)
            } else {
                before[position].clone()
            };
            assert_eq!(
                actual, expected,
                "case {case:?}: host {position} expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// Every value the model declares for each of the three on/off settings is stored and read
/// back, whatever the host already carried.
///
/// The values come from the modelled enums themselves
/// (`ec2::ops::types::AUTO_PLACEMENT_VALUES`, `HOST_RECOVERY_VALUES`,
/// `HOST_MAINTENANCE_VALUES`), so the catalog cannot drift from the model.
#[tokio::test]
async fn every_modelled_value_of_each_on_off_setting_is_applied() {
    let settings: [(&str, &[&str], &str); 3] = [
        ("AutoPlacement", AUTO_PLACEMENT_VALUES, "autoPlacement"),
        ("HostRecovery", HOST_RECOVERY_VALUES, "hostRecovery"),
        ("HostMaintenance", HOST_MAINTENANCE_VALUES, "hostMaintenance"),
    ];
    for (member, values, reported) in settings {
        for value in values {
            for target in 0..allocations().len() {
                let case = format!("{member}={value} on host {target}");
                let (router, _engine, allocated) = seeded().await;

                let (status, body) =
                    modify(&router, &[allocated[target].as_str()], &[(member, value)]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with body {body}"
                );
                assert_eq!(
                    successful(&body),
                    vec![allocated[target].clone()],
                    "case {case:?}: expected the host modified, got {body}"
                );

                let report = described(&router, &case, &allocated[target]).await;
                let actual = report
                    .iter()
                    .find(|(name, _)| *name == reported)
                    .and_then(|(_, text)| text.clone());
                assert_eq!(
                    actual.as_deref(),
                    Some(*value),
                    "case {case:?}: expected {reported} to be {value:?}, got {actual:?}"
                );
            }
        }
    }
}

/// Narrowing a host to one instance type and widening it back to its family round-trips: the
/// host reports exactly what it reported before the pair of calls.
#[tokio::test]
async fn narrowing_then_widening_the_supported_instance_types_round_trips() {
    // Rows: a host allocated for ONE type, widened then narrowed back, and a host allocated
    // for a whole FAMILY, narrowed then widened back.
    let cases: [RoundTripCase; 2] = [
        (
            "one type, widened and narrowed back",
            0,
            [("InstanceFamily", "m5"), ("InstanceType", "m5.large")],
        ),
        (
            "a family, narrowed and widened back",
            1,
            [("InstanceType", "c5.xlarge"), ("InstanceFamily", "c5")],
        ),
    ];
    for (case, target, steps) in cases {
        let (router, _engine, allocated) = seeded().await;
        let before = described(&router, case, &allocated[target]).await;

        for (member, value) in steps {
            let (status, body) =
                modify(&router, &[allocated[target].as_str()], &[(member, value)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {member}={value} expected HTTP 200, got {status} with body {body}"
            );
        }

        let after = described(&router, case, &allocated[target]).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the host back as it was {before:?}, got {after:?}"
        );
    }
}

/// An id the same request names twice names one host: it is modified once and reported once.
#[tokio::test]
async fn one_id_named_twice_is_modified_once_and_reported_once() {
    let (router, _engine, allocated) = seeded().await;
    let host_id = allocated[0].clone();

    let (status, body) = modify(
        &router,
        &[host_id.as_str(), host_id.as_str(), host_id.as_str()],
        &[("AutoPlacement", "on")],
    )
    .await;
    assert_eq!(
        status, 200,
        "an id named three times: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        successful(&body),
        vec![host_id.clone()],
        "an id named three times: expected it reported once, got {body}"
    );
    assert!(
        unsuccessful(&body).is_empty(),
        "an id named three times: expected no unsuccessful item, got {:?}",
        unsuccessful(&body)
    );
    let report = described(&router, "an id named three times", &host_id).await;
    let auto_placement = report
        .iter()
        .find(|(name, _)| *name == "autoPlacement")
        .and_then(|(_, text)| text.clone());
    assert_eq!(
        auto_placement.as_deref(),
        Some("on"),
        "an id named three times: expected autoPlacement on, got {auto_placement:?}"
    );
}

// ---------------------------------------------------------------------------
// The hosts a setting cannot be applied to
// ---------------------------------------------------------------------------

/// A host id this account holds no host for is reported in `Unsuccessful` and the hosts named
/// beside it are modified all the same.
#[tokio::test]
async fn an_id_naming_no_host_is_reported_unsuccessful() {
    // Rows: ids that are not host ids at all, in each way the id form can be wrong, and
    // well-formed ids of both lengths that name no host of this account.
    let cases = [
        "",
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
        let before = described_all(&router, unresolvable, &allocated).await;

        let (status, body) = modify(
            &router,
            &[held.as_str(), unresolvable],
            &[("AutoPlacement", "on")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {unresolvable:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            successful(&body),
            vec![held.clone()],
            "case {unresolvable:?}: expected the held host modified, got {body}"
        );
        assert_eq!(
            unsuccessful(&body),
            vec![(
                unresolvable.to_string(),
                INVALID_HOST_ID_MALFORMED.to_string(),
                unresolvable_host_ids_message([unresolvable]),
            )],
            "case {unresolvable:?}: expected the unresolvable id reported, got {body}"
        );

        for (position, host_id) in allocated.iter().enumerate() {
            let actual = described(&router, unresolvable, host_id).await;
            let expected = if position == 0 {
                with_changes(&before[position], &[("autoPlacement", Some("on"))])
            } else {
                before[position].clone()
            };
            assert_eq!(
                actual, expected,
                "case {unresolvable:?}: host {position} expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// A released host can no longer be modified: its id is reported in `Unsuccessful` and the
/// host keeps every setting it was released with, while the host named beside it is modified.
#[tokio::test]
async fn a_released_host_is_reported_unsuccessful() {
    let case = "a released host";
    let (router, _engine, allocated) = seeded().await;
    let released = allocated[2].clone();
    let held = allocated[0].clone();

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "ReleaseHosts",
        &indexed("HostId", &[released.as_str()]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the preparing release must succeed, got {status} with body {body}"
    );
    let before = described(&router, case, &released).await;

    let (status, body) = modify(
        &router,
        &[held.as_str(), released.as_str()],
        &[("AutoPlacement", "on"), ("HostRecovery", "on")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        successful(&body),
        vec![held.clone()],
        "case {case:?}: expected only the held host modified, got {body}"
    );
    assert_eq!(
        unsuccessful(&body),
        vec![(
            released.clone(),
            INVALID_HOST_ID_MALFORMED.to_string(),
            unresolvable_host_ids_message([released.as_str()]),
        )],
        "case {case:?}: expected the released host reported unsuccessful, got {body}"
    );

    let after = described(&router, case, &released).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected the released host unchanged {before:?}, got {after:?}"
    );
}

/// "Specify this parameter to modify a Dedicated Host to support multiple instance types
/// within its current instance family" (`ModifyHostsRequest$InstanceFamily`): a value naming
/// another instance family is reported in `Unsuccessful`, and the whole modification is left
/// unapplied to that host.
#[tokio::test]
async fn an_instance_family_the_host_is_not_configured_for_is_reported_unsuccessful() {
    // Rows: another family named through a single instance type and named outright, against a
    // general-purpose host and against a Mac host — both directions across the Mac boundary —
    // and one row that carries an on/off setting beside the foreign family.
    let cases: [ForeignFamilyCase; 5] = [
        ForeignFamilyCase {
            case: "another family by instance type",
            target: 0,
            settings: &[("InstanceType", "c5.xlarge")],
            value: "c5.xlarge",
            member: "instanceType",
            family: "m5",
        },
        ForeignFamilyCase {
            case: "another family outright",
            target: 0,
            settings: &[("InstanceFamily", "c5")],
            value: "c5",
            member: "instanceFamily",
            family: "m5",
        },
        ForeignFamilyCase {
            case: "a Mac family against a general-purpose host",
            target: 0,
            settings: &[("InstanceFamily", "mac1")],
            value: "mac1",
            member: "instanceFamily",
            family: "m5",
        },
        ForeignFamilyCase {
            case: "a general-purpose type against a Mac host",
            target: 2,
            settings: &[("InstanceType", "m5.large")],
            value: "m5.large",
            member: "instanceType",
            family: "mac1",
        },
        ForeignFamilyCase {
            case: "an on/off setting beside a foreign family",
            target: 3,
            settings: &[("AutoPlacement", "on"), ("InstanceFamily", "c5")],
            value: "c5",
            member: "instanceFamily",
            family: "r5",
        },
    ];
    for ForeignFamilyCase {
        case,
        target,
        settings,
        value,
        member,
        family,
    } in cases
    {
        let (router, _engine, allocated) = seeded().await;
        let before = described_all(&router, case, &allocated).await;

        let (status, body) = modify(&router, &[allocated[target].as_str()], settings).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert!(
            successful(&body).is_empty(),
            "case {case:?}: expected no host modified, got {body}"
        );
        let reported = unsuccessful(&body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly one unsuccessful item, got {reported:?}"
        );
        let (resource, code, message) = &reported[0];
        assert_eq!(
            resource, &allocated[target],
            "case {case:?}: expected the item to name the host, got {resource:?}"
        );
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected the code {INVALID_PARAMETER_VALUE}, got {code:?}"
        );
        for expected in [value, member, family] {
            assert!(
                message.contains(expected),
                "case {case:?}: expected the message to name {expected:?}, got {message:?}"
            );
        }

        for (position, host_id) in allocated.iter().enumerate() {
            let actual = described(&router, case, host_id).await;
            assert_eq!(
                actual, before[position],
                "case {case:?}: host {position} expected {:?}, got {actual:?}",
                before[position]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What the request itself is refused for
// ---------------------------------------------------------------------------

/// A request naming no setting at all asks for no change, so it is refused rather than
/// answered with a host that moved nothing.
#[tokio::test]
async fn a_request_naming_no_setting_is_refused() {
    let case = "no setting";
    let (router, _engine, allocated) = seeded().await;
    let before = described_all(&router, case, &allocated).await;

    let (status, body) = modify(&router, &[allocated[0].as_str()], &[]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        MISSING_PARAMETER,
        "case {case:?}: expected the code {MISSING_PARAMETER}, got {body}"
    );
    for named in ["autoPlacement", "hostRecovery", "hostMaintenance", "instanceType"] {
        assert!(
            unescaped(&element(&body, "Message").unwrap_or_default()).contains(named),
            "case {case:?}: the refusal must name {named:?}, got {body}"
        );
    }

    let after = described_all(&router, case, &allocated).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected every host unchanged {before:?}, got {after:?}"
    );
}

/// "You cannot specify InstanceType and InstanceFamily in the same request"
/// (`ModifyHostsRequest$InstanceType`): the pair is refused whatever the two values are, and
/// nothing is applied.
#[tokio::test]
async fn an_instance_type_and_an_instance_family_together_are_refused() {
    // Rows: two values of ONE family, two values of different families, and the pair beside an
    // on/off setting that would otherwise have been applied.
    let cases: [(&str, &[Setting]); 3] = [
        (
            "one family, both members",
            &[("InstanceType", "m5.large"), ("InstanceFamily", "m5")],
        ),
        (
            "two families",
            &[("InstanceType", "m5.large"), ("InstanceFamily", "c5")],
        ),
        (
            "the pair beside auto-placement",
            &[
                ("AutoPlacement", "on"),
                ("InstanceType", "m5.large"),
                ("InstanceFamily", "m5"),
            ],
        ),
    ];
    for (case, settings) in cases {
        let (router, _engine, allocated) = seeded().await;
        let before = described_all(&router, case, &allocated).await;

        let (status, body) = modify(&router, &[allocated[0].as_str()], settings).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case {case:?}: expected the code {INVALID_REQUEST}, got {body}"
        );

        let after = described_all(&router, case, &allocated).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected every host unchanged {before:?}, got {after:?}"
        );
    }
}

/// A value that names no hardware EC2 offers asks for a host CONFIGURATION EC2 cannot
/// provide, so it is refused before any host is read, with the configuration code and that
/// class's own sentence: the catalog both members are checked against is the modelled
/// `InstanceType` enum.
#[tokio::test]
async fn a_value_outside_the_modelled_catalog_is_refused() {
    // Rows: a size no family offers, a value that is not an instance type at all, a family no
    // instance type belongs to, and a whole instance type given where a family belongs.
    let cases: [(&str, &str, &str); 5] = [
        ("a size no family offers", "InstanceType", "m5.enormous"),
        ("not an instance type at all", "InstanceType", "not-a-type"),
        ("an instance family as a type", "InstanceType", "m5"),
        ("a family no instance type has", "InstanceFamily", "zz9"),
        ("an instance type as a family", "InstanceFamily", "m5.large"),
    ];
    for (case, member, value) in cases {
        let (router, _engine, allocated) = seeded().await;
        let before = described_all(&router, case, &allocated).await;

        let (status, body) = modify(&router, &[allocated[0].as_str()], &[(member, value)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_HOST_CONFIGURATION,
            "case {case:?}: expected the code {INVALID_HOST_CONFIGURATION}, got {body}"
        );
        let expected_message = if member == "InstanceType" {
            format!("The instance type '{value}' is not supported.")
        } else {
            format!("The instance family '{value}' is not supported.")
        };
        let answered = error_message(&body);
        assert_eq!(
            answered, expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {answered:?}"
        );

        let after = described_all(&router, case, &allocated).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected every host unchanged {before:?}, got {after:?}"
        );
    }
}

/// A value outside a modelled enum is refused by the modelled constraint, before the handler
/// runs — never stored as an on/off setting nothing understands.
#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused() {
    for (member, value) in [
        ("AutoPlacement", "maybe"),
        ("HostRecovery", "ON"),
        ("HostMaintenance", "enabled"),
    ] {
        let case = format!("{member}={value}");
        let (router, _engine, allocated) = seeded().await;
        let before = described_all(&router, &case, &allocated).await;

        let (status, body) = modify(&router, &[allocated[0].as_str()], &[(member, value)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );

        let after = described_all(&router, &case, &allocated).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected every host unchanged {before:?}, got {after:?}"
        );
    }
}

/// The ceiling this crate puts on a caller-chosen id list: a request naming more ids than it
/// allows is refused before any host is read, so nothing is applied to the hosts it named.
#[tokio::test]
async fn an_id_list_above_the_ceiling_is_refused() {
    let case = "an id list above the ceiling";
    let (router, _engine, allocated) = seeded().await;
    let before = described_all(&router, case, &allocated).await;

    // One id past the ceiling, a held host among them, so the refusal cannot be read as this
    // request naming no modifiable host. The filler ids are well formed (`h-` and 17
    // hexadecimal digits) so the ceiling is what refuses the request, not their shape.
    let mut named: Vec<String> = vec![allocated[0].clone()];
    while named.len() <= MAX_RESOURCE_IDS {
        named.push(format!("h-{:017x}", named.len()));
    }
    let ids: Vec<&str> = named.iter().map(String::as_str).collect();

    let (status, body) = modify(&router, &ids, &[("AutoPlacement", "on")]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: {} ids expected HTTP 400, got {status} with body {body}",
        ids.len()
    );
    assert_eq!(
        error_code(&body),
        INVALID_PARAMETER_VALUE,
        "case {case:?}: expected the code {INVALID_PARAMETER_VALUE}, got {body}"
    );

    let after = described_all(&router, case, &allocated).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected every host unchanged {before:?}, got {after:?}"
    );
}

/// "The IDs of the Dedicated Hosts to modify" (`ModifyHostsRequest$HostIds`) is a REQUIRED
/// member, so a request naming no host modifies nothing — never answered with two empty sets.
#[tokio::test]
async fn a_request_naming_no_host_is_refused() {
    let case = "no host id";
    let (router, _engine, allocated) = seeded().await;
    let before = described_all(&router, case, &allocated).await;

    let (status, body) = modify(&router, &[], &[("AutoPlacement", "on")]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert!(
        unescaped(&element(&body, "Message").unwrap_or_default()).contains("hostIds"),
        "case {case:?}: the refusal must name the required member, got {body}"
    );

    let after = described_all(&router, case, &allocated).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected every host unchanged {before:?}, got {after:?}"
    );
}
