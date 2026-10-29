// SPDX-License-Identifier: CC0-1.0

//! `DetachStaticIp` — taking a reserved address off the instance holding it —
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The `Authorization` header carries only a credential SCOPE, which is
//! what names the Region a request is made in.
//!
//! The rule under test: a detach releases the ATTACHMENT the named resource holds
//! and nothing else — "detaches a static IP from the Amazon Lightsail instance to
//! which it is attached" — so the address stays reserved for the account under the
//! same name, and the instance it came off reports `isStaticIp` false at an
//! ordinary public IPv4 address, because an attached static IP is the only thing
//! that pins one ("to use the same IP address after stopping and starting an
//! instance, create a static IP address and attach it to the instance", the
//! modelled `StartInstance` documentation). It answers ONE terminal
//! `DetachStaticIp` operation-log entry naming the static IP, because the modelled
//! `StaticIp` carries no lifecycle state for the clock to settle. A static IP
//! attached to nothing is already in the state the request asks for, so the call
//! releases nothing and is not refused — this operation's documentation states no
//! precondition on the attachment, and the modelled `StaticIp` carries no state a
//! precondition could be about, unlike `DetachDisk`, whose documentation states
//! one — while a name the account does not hold detaches nothing and is refused
//! with the service's `DoesNotExist` envelope.
//!
//! The rows span what the rule generalises over: the form and length of the name
//! the modelled `ResourceName` pattern `^\w[\w\-]*\w$` admits, the Region the
//! reservation and the instance are in (including a request that carries no
//! credential scope at all), which addresses the instance has at all (its modelled
//! `ipAddressType`), the instance's own lifecycle state when the address comes off
//! it, and how many times the same static IP is detached.
//!
//! The HTTP harness, the address and id shape checks, the settle poll, the
//! attachment fixture and the checked page walk are the ones this test target
//! already publishes: this suite carries no second copy of any of them.

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state::STATIC_IP_PAGE_SIZE;

use crate::instance_lifecycle::{
    error_code, is_modelled_ipv4, is_uuid_shaped, wait_for_state, Emulator,
};
use crate::instance_start_addresses::attach_static_ip;
use crate::paging::{names, walk_pages};

/// The blueprint and bundle every instance in this suite is created from: a
/// published Linux plan (`src/state.rs`, `BUNDLE_SPECS`). Which plan it is changes
/// nothing about what a detach releases, so one is enough here.
const BLUEPRINT_ID: &str = "amazon_linux_2023";
/// See [`BLUEPRINT_ID`].
const BUNDLE_ID: &str = "nano_3_0";

/// The wire error a Lightsail request for a resource the account does not hold is
/// refused with, and the `code` its body carries.
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// A static IP no case ever detaches, allocated beside the one under test so every
/// assertion is about the name the request carried rather than about an account
/// holding one reservation.
const KEPT: &str = "kept-static-ip";

/// A name of 64 characters, all admitted by the modelled `ResourceName` pattern.
const LONG_NAME: &str = "static-ip-with-a-very-long-but-entirely-well-formed-name-abcdefg";

/// Every member of the modelled `StaticIp` that a detach does not touch: the
/// reservation is kept, so only `attachedTo` and `isAttached` may differ across
/// one.
const RESERVATION_MEMBERS: &[&str] = &[
    "name",
    "arn",
    "supportCode",
    "createdAt",
    "location",
    "resourceType",
    "ipAddress",
];

/// Every member of the modelled `Instance` that a detach does not touch: a static
/// IP reserves ONE public IPv4 address, so it pinned nothing else.
const UNTOUCHED_INSTANCE_MEMBERS: &[&str] = &[
    "name",
    "arn",
    "supportCode",
    "createdAt",
    "location",
    "ipAddressType",
    "blueprintId",
    "bundleId",
    "privateIpAddress",
    "ipv6Addresses",
    "hardware",
    "networking",
    "state",
];

/// One detach case.
struct Row {
    case: &'static str,
    /// The static IP the case detaches.
    static_ip_name: &'static str,
    /// The instance it is attached to when it is detached.
    instance_name: &'static str,
    /// The zone the instance is created in, which is what decides its Region.
    availability_zone: &'static str,
    /// The Region every call of the row is made in, or `None` for requests that
    /// carry no credential scope at all.
    region: Option<&'static str>,
    /// The Region the reservation and the operation-log entry must report —
    /// `us-east-1` (`crate::state::DEFAULT_REGION`) for a request that names
    /// none.
    reported_region: &'static str,
    /// What the create sends as `ipAddressType`; `None` sends none at all, which
    /// the service defaults to `dualstack`.
    ip_address_type: Option<&'static str>,
    /// Whether that address type gives the instance an IPv6 address, from the
    /// modelled `IpAddressType` enum (`ipv4` is IPv4 only, `dualstack` is both).
    has_ipv6: bool,
    /// Whether the instance is STOPPED when the address comes off it: this
    /// operation's documentation states no precondition on the instance, so its
    /// state must select nothing about the detach.
    stopped: bool,
}

/// The table. The modelled `StaticIp$name` example (`StaticIP-Ohio-EXAMPLE`) is
/// the first row.
const ROWS: &[Row] = &[
    Row {
        case: "the modelled name example, a dualstack instance, in no named region",
        static_ip_name: "StaticIP-Ohio-EXAMPLE",
        instance_name: "front-host",
        availability_zone: "us-east-1a",
        region: None,
        reported_region: "us-east-1",
        ip_address_type: None,
        has_ipv6: true,
        stopped: false,
    },
    Row {
        case: "an underscored name, an IPv4-only instance",
        static_ip_name: "static_ip_two",
        instance_name: "web_host_1",
        availability_zone: "us-west-2a",
        region: Some("us-west-2"),
        reported_region: "us-west-2",
        ip_address_type: Some("ipv4"),
        has_ipv6: false,
        stopped: false,
    },
    Row {
        case: "the shortest name the modelled pattern admits, a STOPPED instance",
        static_ip_name: "ip",
        instance_name: "edge-host-2",
        availability_zone: "eu-west-1b",
        region: Some("eu-west-1"),
        reported_region: "eu-west-1",
        ip_address_type: None,
        has_ipv6: true,
        stopped: true,
    },
    Row {
        case: "a digit-led mixed name, a dualstack instance on another continent",
        static_ip_name: "0a-static_ip-3",
        instance_name: "host_3",
        availability_zone: "ap-southeast-2b",
        region: Some("ap-southeast-2"),
        reported_region: "ap-southeast-2",
        ip_address_type: None,
        has_ipv6: true,
        stopped: false,
    },
    Row {
        case: "a long name, an IPv4-only STOPPED instance",
        static_ip_name: LONG_NAME,
        instance_name: "south-host",
        availability_zone: "sa-east-1c",
        region: Some("sa-east-1"),
        reported_region: "sa-east-1",
        ip_address_type: Some("ipv4"),
        has_ipv6: false,
        stopped: true,
    },
];

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Allocate the static IP `name` in `region`.
async fn allocate(emulator: &Emulator, region: Option<&str>, name: &str) {
    emulator
        .ok_at(None, region, "AllocateStaticIp", json!({"staticIpName": name}))
        .await;
}

/// Detach the static IP `name` in `region`, answering the `operations` the call
/// reported.
async fn detach(emulator: &Emulator, case: &str, region: Option<&str>, name: &str) -> Vec<Value> {
    let body = emulator
        .ok_at(None, region, "DetachStaticIp", json!({"staticIpName": name}))
        .await;
    body["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {body:?}"))
        .clone()
}

/// The stored static IP `name`, as `GetStaticIp` reports it. Requires it to be
/// held.
async fn static_ip(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetStaticIp", json!({"staticIpName": name}))
        .await["staticIp"]
        .clone()
}

/// The stored instance `name`, as `GetInstance` reports it.
async fn instance(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetInstance", json!({"instanceName": name}))
        .await["instance"]
        .clone()
}

/// Every static IP name the account holds, walking every page of `GetStaticIps`.
async fn held_names(emulator: &Emulator, case: &str) -> Vec<String> {
    let (records, _) =
        walk_pages(emulator, case, None, "GetStaticIps", "staticIps", STATIC_IP_PAGE_SIZE).await;
    names(&records)
}

/// Every operation-log entry the store holds against `resource_name`, as
/// `GetOperationsForResource` reports it.
async fn logged_operations(emulator: &Emulator, case: &str, resource_name: &str) -> Vec<Value> {
    let body = emulator
        .ok(
            "GetOperationsForResource",
            json!({"resourceName": resource_name}),
        )
        .await;
    body["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {body:?}"))
        .clone()
}

/// Create the row's instance, wait until it is serving, and stop it where the row
/// detaches from a stopped instance.
async fn create_instance(emulator: &Emulator, row: &Row) {
    let mut body = json!({
        "instanceNames": [row.instance_name],
        "availabilityZone": row.availability_zone,
        "blueprintId": BLUEPRINT_ID,
        "bundleId": BUNDLE_ID,
    });
    if let Some(ip_address_type) = row.ip_address_type {
        body["ipAddressType"] = json!(ip_address_type);
    }
    emulator.ok("CreateInstances", body).await;
    wait_for_state(emulator, row.instance_name, "running").await;
    if row.stopped {
        emulator
            .ok(
                "StopInstance",
                json!({"instanceName": row.instance_name, "force": true}),
            )
            .await;
        wait_for_state(emulator, row.instance_name, "stopped").await;
    }
}

/// Assert `entries` is the one terminal `DetachStaticIp` log entry a detach
/// answers, naming `name` in `region`.
fn assert_one_terminal_entry(case: &str, entries: &[Value], name: &str, region: &str) -> String {
    assert_eq!(
        entries.len(),
        1,
        "case {case}: expected exactly one operation, got {entries:?}"
    );
    let entry = &entries[0];
    assert_eq!(
        (
            entry["operationType"].as_str(),
            entry["resourceType"].as_str(),
            entry["resourceName"].as_str(),
            entry["status"].as_str(),
            entry["isTerminal"].as_bool(),
            entry["location"]["regionName"].as_str(),
        ),
        (
            Some("DetachStaticIp"),
            Some("StaticIp"),
            Some(name),
            Some("Succeeded"),
            Some(true),
            Some(region),
        ),
        "case {case}: expected a succeeded DetachStaticIp entry, got {entry:?}"
    );
    let id = entry["id"].as_str().unwrap_or_default().to_string();
    assert!(
        is_uuid_shaped(&id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    id
}

/// Assert `after` is `before` with the attachment released and nothing else about
/// the reservation changed.
fn assert_reservation_kept_unattached(case: &str, before: &Value, after: &Value) {
    assert_eq!(
        (after["isAttached"].as_bool(), after["attachedTo"].as_str()),
        (Some(false), None),
        "case {case}: expected an unattached reservation, got {after:?}"
    );
    for member in RESERVATION_MEMBERS {
        assert_eq!(
            after[member], before[member],
            "case {case}: a detach must leave {member} alone, expected {:?}, got {:?}",
            before[member], after[member]
        );
    }
}

/// Assert `status`/`body` is the refusal of a request naming a static IP the
/// account does not hold.
fn assert_does_not_exist(case: &str, name: &str, status: u16, body: &Value) {
    assert_eq!(
        (status, error_code(body).as_str(), body["code"].as_str()),
        (400, NOT_FOUND, Some(DOES_NOT_EXIST)),
        "case {case}: expected the DoesNotExist envelope for {name}, got {status} {body:?}"
    );
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("StaticIp") && message.contains(name)),
        "case {case}: expected a message naming the resource type and {name}, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// The detach
// ---------------------------------------------------------------------------

/// RULE: a detach releases the attachment and keeps the reservation — the address
/// is still held under the same name, every other member of the record is
/// untouched, the instance reports `isStaticIp` false at an ordinary public IPv4
/// address no resource in the store has held, and the call answers one terminal
/// `DetachStaticIp` entry naming the static IP in the Region it was reserved in.
#[tokio::test]
async fn detaching_a_static_ip_releases_the_attachment_and_keeps_the_reservation() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        allocate(&emulator, None, KEPT).await;
        allocate(&emulator, row.region, row.static_ip_name).await;
        create_instance(&emulator, row).await;

        let created = instance(&emulator, row.instance_name).await;
        let created_address = created["publicIpAddress"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let reserved = attach_static_ip(&emulator, row.static_ip_name, row.instance_name).await;

        // What a client sees while the attachment stands.
        let attached_ip = static_ip(&emulator, row.static_ip_name).await;
        let attached_instance = instance(&emulator, row.instance_name).await;
        assert_eq!(
            (
                attached_ip["isAttached"].as_bool(),
                attached_ip["attachedTo"].as_str(),
                attached_ip["ipAddress"].as_str(),
                attached_instance["isStaticIp"].as_bool(),
                attached_instance["publicIpAddress"].as_str(),
            ),
            (
                Some(true),
                Some(row.instance_name),
                Some(reserved.as_str()),
                Some(true),
                Some(reserved.as_str()),
            ),
            "case {case}: expected the instance on the reserved address, got {attached_ip:?} \
             and {attached_instance:?}"
        );

        let entries = detach(&emulator, case, row.region, row.static_ip_name).await;
        let id = assert_one_terminal_entry(case, &entries, row.static_ip_name, row.reported_region);

        // The reservation is kept, the attachment is gone.
        let detached_ip = static_ip(&emulator, row.static_ip_name).await;
        assert_reservation_kept_unattached(case, &attached_ip, &detached_ip);
        let held = held_names(&emulator, case).await;
        assert!(
            held.contains(&row.static_ip_name.to_string()) && held.contains(&KEPT.to_string()),
            "case {case}: expected both reservations still held, got {held:?}"
        );

        // The instance fell back to an ordinary public IPv4 address — one the
        // store has never handed out — and says so.
        let detached_instance = instance(&emulator, row.instance_name).await;
        assert_eq!(
            detached_instance["isStaticIp"].as_bool(),
            Some(false),
            "case {case}: no static IP is attached any more, got {detached_instance:?}"
        );
        let public = detached_instance["publicIpAddress"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_ipv4(&public),
            "case {case}: expected an ordinary public IPv4 address, got {public:?}"
        );
        for (what, spent) in [
            ("the detached reservation", &reserved),
            ("the address it had before the attachment", &created_address),
        ] {
            assert_ne!(
                &public, spent,
                "case {case}: expected an address the store has never handed out, got {what} \
                 {public:?}"
            );
        }
        for member in UNTOUCHED_INSTANCE_MEMBERS {
            assert_eq!(
                detached_instance[member], attached_instance[member],
                "case {case}: a detach must leave {member} alone, expected {:?}, got {:?}",
                attached_instance[member], detached_instance[member]
            );
        }
        let ipv6 = detached_instance["ipv6Addresses"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            !ipv6.is_empty(),
            row.has_ipv6,
            "case {case}: expected the address type's IPv6 addressing to be untouched, got \
             {ipv6:?}"
        );

        // The entry the call answered is the one the store holds against the
        // static IP, read back through the log.
        let logged = logged_operations(&emulator, case, row.static_ip_name).await;
        assert!(
            logged
                .iter()
                .any(|entry| entry["id"].as_str() == Some(id.as_str())
                    && entry["operationType"].as_str() == Some("DetachStaticIp")),
            "case {case}: expected the detach entry {id} in the static IP's log, got {logged:?}"
        );
    }
}

/// RULE (the same one, applied twice): a static IP attached to nothing is already
/// in the state the request asks for, so detaching it again releases nothing and
/// is not refused — the reservation, the instance it came off and every other
/// record read back exactly as the first detach left them.
#[tokio::test]
async fn detaching_a_static_ip_twice_releases_nothing_the_second_time() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        allocate(&emulator, row.region, row.static_ip_name).await;
        create_instance(&emulator, row).await;
        attach_static_ip(&emulator, row.static_ip_name, row.instance_name).await;

        detach(&emulator, case, row.region, row.static_ip_name).await;
        let once_ip = static_ip(&emulator, row.static_ip_name).await;
        let once_instance = instance(&emulator, row.instance_name).await;

        let entries = detach(&emulator, case, row.region, row.static_ip_name).await;
        assert_one_terminal_entry(case, &entries, row.static_ip_name, row.reported_region);

        let twice_ip = static_ip(&emulator, row.static_ip_name).await;
        let twice_instance = instance(&emulator, row.instance_name).await;
        assert_eq!(
            twice_ip, once_ip,
            "case {case}: a second detach releases nothing, expected {once_ip:?}, got \
             {twice_ip:?}"
        );
        assert_eq!(
            twice_instance, once_instance,
            "case {case}: a second detach must leave the instance alone, expected \
             {once_instance:?}, got {twice_instance:?}"
        );
    }
}

/// RULE (the same one, on a reservation that never held an instance): a static IP
/// attached to nothing at all is detached without a refusal and without a change —
/// the operation's documentation states no precondition on the attachment, and the
/// modelled `StaticIp` carries no lifecycle state a precondition could be about.
#[tokio::test]
async fn detaching_a_static_ip_that_was_never_attached_changes_nothing() {
    // The rows differ along what the rule generalises over: the form of the name
    // and the Region the reservation was made in.
    let cases: &[(&str, &str, Option<&str>, &str)] = &[
        (
            "a never attached reservation in no named region",
            "never-attached",
            None,
            "us-east-1",
        ),
        (
            "a never attached reservation in another region",
            "never_attached_2",
            Some("eu-central-1"),
            "eu-central-1",
        ),
        (
            "the shortest name the modelled pattern admits",
            "zz",
            Some("ap-northeast-1"),
            "ap-northeast-1",
        ),
    ];
    for (case, name, region, reported_region) in cases {
        let emulator = Emulator::new();
        allocate(&emulator, None, KEPT).await;
        allocate(&emulator, *region, name).await;
        let before = static_ip(&emulator, name).await;
        let before_kept = static_ip(&emulator, KEPT).await;

        let entries = detach(&emulator, case, *region, name).await;
        assert_one_terminal_entry(case, &entries, name, reported_region);

        let after = static_ip(&emulator, name).await;
        assert_eq!(
            after, before,
            "case {case}: a detach of an unattached reservation changes nothing, expected \
             {before:?}, got {after:?}"
        );
        assert_eq!(
            static_ip(&emulator, KEPT).await,
            before_kept,
            "case {case}: every other reservation must be untouched"
        );
    }
}

/// RULE: a well-formed name the account does not hold detaches nothing — the
/// `DoesNotExist` refusal — and leaves every record the account does hold exactly
/// as it was, the attachment one of them included.
#[tokio::test]
async fn a_name_the_account_does_not_hold_detaches_nothing() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    allocate(&emulator, None, KEPT).await;
    allocate(&emulator, None, row.static_ip_name).await;
    create_instance(&emulator, row).await;
    attach_static_ip(&emulator, row.static_ip_name, row.instance_name).await;

    let before_kept = static_ip(&emulator, KEPT).await;
    let before_attached = static_ip(&emulator, row.static_ip_name).await;
    let before_instance = instance(&emulator, row.instance_name).await;

    // The rows differ along what makes the name unheld: never allocated, a
    // difference in case only, the shortest and a long well-formed name, and the
    // name of a resource of another kind the account does hold.
    let cases: &[(&str, &str)] = &[
        ("a name nothing ever allocated", "no-such-static-ip"),
        ("a name differing only in case", "Kept-Static-Ip"),
        ("the shortest well-formed name", "zz"),
        ("a long well-formed name", LONG_NAME),
        ("the name of the instance holding the address", row.instance_name),
    ];
    for (case, name) in cases {
        let (status, body) = emulator
            .call("DetachStaticIp", json!({"staticIpName": name}))
            .await;
        assert_does_not_exist(case, name, status, &body);
    }

    assert_eq!(
        static_ip(&emulator, KEPT).await,
        before_kept,
        "case the refused detaches: the unattached reservation must be untouched"
    );
    assert_eq!(
        static_ip(&emulator, row.static_ip_name).await,
        before_attached,
        "case the refused detaches: the attachment must be untouched"
    );
    assert_eq!(
        instance(&emulator, row.instance_name).await,
        before_instance,
        "case the refused detaches: the instance must be untouched"
    );
}

/// RULE (the same one, over a set): a detach releases the ONE attachment it names
/// — every other static IP the account holds, and every attachment another
/// instance holds, is left exactly as it was.
#[tokio::test]
async fn detaching_one_static_ip_leaves_every_other_attachment_alone() {
    let emulator = Emulator::new();
    // Two instances, each holding a static IP, so nothing but the name tells the
    // two attachments apart; and a third reservation held by nothing at all.
    let detached = &ROWS[0];
    let kept = &ROWS[1];
    for row in [detached, kept] {
        allocate(&emulator, row.region, row.static_ip_name).await;
        create_instance(&emulator, row).await;
        attach_static_ip(&emulator, row.static_ip_name, row.instance_name).await;
    }
    allocate(&emulator, None, KEPT).await;

    let before_kept_ip = static_ip(&emulator, kept.static_ip_name).await;
    let before_kept_instance = instance(&emulator, kept.instance_name).await;
    let before_spare = static_ip(&emulator, KEPT).await;

    detach(&emulator, "one of three", None, detached.static_ip_name).await;

    let held = held_names(&emulator, "one of three").await;
    assert_eq!(
        held.len(),
        3,
        "case one of three: a detach releases no reservation, got {held:?}"
    );
    assert_eq!(
        static_ip(&emulator, kept.static_ip_name).await,
        before_kept_ip,
        "case one of three: expected the other attachment untouched"
    );
    assert_eq!(
        static_ip(&emulator, KEPT).await,
        before_spare,
        "case one of three: expected the unattached reservation untouched"
    );
    assert_eq!(
        instance(&emulator, kept.instance_name).await,
        before_kept_instance,
        "case one of three: expected the other holder's record untouched"
    );
}
