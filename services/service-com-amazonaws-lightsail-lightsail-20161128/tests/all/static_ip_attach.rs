// SPDX-License-Identifier: CC0-1.0

//! `AttachStaticIp` — putting a reserved address onto the instance that is to
//! serve on it — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The `Authorization` header carries only a credential SCOPE, which is
//! what names the Region a request is made in.
//!
//! The rule under test: an attach makes the ONE attachment it names — "attaches a
//! static IP address to a specific Amazon Lightsail instance" — so the named
//! reservation reports the instance it is attached to, that instance is reachable
//! at the reserved address and says `isStaticIp`, and nothing else about either
//! record moves; an attached static IP is the only thing that pins an instance's
//! public IPv4 address ("to use the same IP address after stopping and starting an
//! instance, create a static IP address and attach it to the instance", the
//! modelled `StartInstance` documentation), which is what the address surviving a
//! stop and a start is about (pinned in `instance_start_addresses.rs`). It answers
//! ONE terminal `AttachStaticIp` operation-log entry naming the static IP, because
//! the modelled `StaticIp` carries no lifecycle state for the clock to settle, and
//! attaching the address an instance already holds asks for the state the store is
//! already in, so it changes nothing and is not refused. A request that cannot be
//! served attaches NOTHING: a name the account holds no static IP or no instance
//! under is refused with the service's `DoesNotExist` envelope, and a reservation
//! or an instance that already holds another attachment, and an instance whose
//! `ipAddressType` gives it no public IPv4 address for a reservation to pin, are
//! each refused with the modelled `OperationFailureException`.
//!
//! The rows span what the rule generalises over: the form and length of the name
//! the modelled `ResourceName` pattern `^\w[\w\-]*\w$` admits, the Region the
//! reservation and the instance are in (including a request that carries no
//! credential scope at all), which addresses the instance has at all (its modelled
//! `ipAddressType`), the instance's own lifecycle state when the address goes onto
//! it, and how many times the same static IP is attached.
//!
//! The HTTP harness, the address and id shape checks and the settle poll are the
//! ones this test target already publishes: this suite carries no second copy of
//! any of them.

use serde_json::{json, Value};

use crate::instance_lifecycle::{
    error_code, is_modelled_ipv4, is_uuid_shaped, wait_for_state, Emulator,
};

/// The blueprint and bundle every instance in this suite is created from: a
/// published Linux plan (`src/state.rs`, `BUNDLE_SPECS`). Which plan it is changes
/// nothing about what an attach pins, so one is enough here.
const BLUEPRINT_ID: &str = "amazon_linux_2023";
/// See [`BLUEPRINT_ID`].
const BUNDLE_ID: &str = "nano_3_0";

/// The wire error a Lightsail request for a resource the account does not hold is
/// refused with, and the `code` its body carries.
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";
/// The wire error Lightsail refuses a well-formed request its resources' state
/// does not allow with.
const OPERATION_FAILURE: &str = "OperationFailureException";

/// A static IP no case ever attaches, allocated beside the one under test so every
/// assertion is about the name the request carried rather than about an account
/// holding one reservation.
const KEPT: &str = "kept-static-ip";

/// A name of 64 characters, all admitted by the modelled `ResourceName` pattern.
const LONG_NAME: &str = "static-ip-with-a-very-long-but-entirely-well-formed-name-abcdefg";

/// Every member of the modelled `StaticIp` an attach does not touch: the
/// reservation is kept as it was, the reserved address above all, so only
/// `attachedTo` and `isAttached` may differ across one.
const RESERVATION_MEMBERS: &[&str] = &[
    "name",
    "arn",
    "supportCode",
    "createdAt",
    "location",
    "resourceType",
    "ipAddress",
];

/// Every member of the modelled `Instance` an attach does not touch: a static IP
/// reserves ONE public IPv4 address, so it pins nothing else. The instance's own
/// `state` is left out because the clock settles it on its own schedule, which an
/// attach neither waits for nor moves.
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
];

/// What the instance's lifecycle state is when the address goes onto it. This
/// operation's documentation states no precondition on it — unlike `AttachDisk`,
/// which "attaches a block storage disk to a running or stopped" instance — so the
/// state must select nothing about the attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settle {
    /// Attached once the instance is serving.
    Running,
    /// Attached while the instance is stopped.
    Stopped,
    /// Attached while the instance is still coming up, so the clock still owes it
    /// `running`.
    ComingUp,
}

/// One attach case.
struct Row {
    case: &'static str,
    /// The static IP the case attaches.
    static_ip_name: &'static str,
    /// The instance it is attached to.
    instance_name: &'static str,
    /// The zone the instance is created in, which is what decides its Region.
    availability_zone: &'static str,
    /// The Region every call of the row is made in, or `None` for requests that
    /// carry no credential scope at all.
    region: Option<&'static str>,
    /// The Region the reservation and the operation-log entry must report —
    /// `us-east-1` (`crate::state::DEFAULT_REGION`) for a request that names none.
    reported_region: &'static str,
    /// What the create sends as `ipAddressType`; `None` sends none at all, which
    /// the service defaults to `dualstack`.
    ip_address_type: Option<&'static str>,
    /// Whether that address type gives the instance an IPv6 address, from the
    /// modelled `IpAddressType` enum (`ipv4` is IPv4 only, `dualstack` is both).
    has_ipv6: bool,
    /// The instance's lifecycle state when the address goes onto it.
    settle: Settle,
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
        settle: Settle::Running,
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
        settle: Settle::Running,
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
        settle: Settle::Stopped,
    },
    Row {
        case: "a digit-led mixed name, an instance that is still coming up",
        static_ip_name: "0a-static_ip-3",
        instance_name: "host_3",
        availability_zone: "ap-southeast-2b",
        region: Some("ap-southeast-2"),
        reported_region: "ap-southeast-2",
        ip_address_type: None,
        has_ipv6: true,
        settle: Settle::ComingUp,
    },
    Row {
        case: "a long name, an IPv4-only STOPPED instance on another continent",
        static_ip_name: LONG_NAME,
        instance_name: "south-host",
        availability_zone: "sa-east-1c",
        region: Some("sa-east-1"),
        reported_region: "sa-east-1",
        ip_address_type: Some("ipv4"),
        has_ipv6: false,
        settle: Settle::Stopped,
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

/// Attach the static IP `static_ip_name` to the instance `instance_name` in
/// `region`, answering the `operations` the call reported.
async fn attach(
    emulator: &Emulator,
    case: &str,
    region: Option<&str>,
    static_ip_name: &str,
    instance_name: &str,
) -> Vec<Value> {
    let body = emulator
        .ok_at(
            None,
            region,
            "AttachStaticIp",
            json!({"staticIpName": static_ip_name, "instanceName": instance_name}),
        )
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

/// Create the instance `name` in `zone` with `ip_address_type`, and bring it to
/// the lifecycle state `settle` names.
async fn create_instance(
    emulator: &Emulator,
    name: &str,
    zone: &str,
    ip_address_type: Option<&str>,
    settle: Settle,
) {
    let mut body = json!({
        "instanceNames": [name],
        "availabilityZone": zone,
        "blueprintId": BLUEPRINT_ID,
        "bundleId": BUNDLE_ID,
    });
    if let Some(ip_address_type) = ip_address_type {
        body["ipAddressType"] = json!(ip_address_type);
    }
    emulator.ok("CreateInstances", body).await;
    match settle {
        // The create leaves the instance coming up, so this row attaches the
        // address without waiting for anything.
        Settle::ComingUp => {}
        Settle::Running => {
            wait_for_state(emulator, name, "running").await;
        }
        Settle::Stopped => {
            wait_for_state(emulator, name, "running").await;
            emulator
                .ok("StopInstance", json!({"instanceName": name, "force": true}))
                .await;
            wait_for_state(emulator, name, "stopped").await;
        }
    }
}

/// Create the row's instance, in the state the row attaches the address in.
async fn create_row_instance(emulator: &Emulator, row: &Row) {
    create_instance(
        emulator,
        row.instance_name,
        row.availability_zone,
        row.ip_address_type,
        row.settle,
    )
    .await;
}

/// Assert `entries` is the one terminal `AttachStaticIp` log entry an attach
/// answers, naming `name` in `region`, and answer its id.
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
            Some("AttachStaticIp"),
            Some("StaticIp"),
            Some(name),
            Some("Succeeded"),
            Some(true),
            Some(region),
        ),
        "case {case}: expected a succeeded AttachStaticIp entry, got {entry:?}"
    );
    let id = entry["id"].as_str().unwrap_or_default().to_string();
    assert!(
        is_uuid_shaped(&id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    id
}

/// Assert `after` is `before` with the attachment to `instance_name` made and
/// nothing else about the reservation changed, and answer the reserved address.
fn assert_reservation_attached(
    case: &str,
    before: &Value,
    after: &Value,
    instance_name: &str,
) -> String {
    assert_eq!(
        (after["isAttached"].as_bool(), after["attachedTo"].as_str()),
        (Some(true), Some(instance_name)),
        "case {case}: expected the reservation attached to {instance_name}, got {after:?}"
    );
    for member in RESERVATION_MEMBERS {
        assert_eq!(
            after[member], before[member],
            "case {case}: an attach must leave {member} alone, expected {:?}, got {:?}",
            before[member], after[member]
        );
    }
    let address = after["ipAddress"].as_str().unwrap_or_default().to_string();
    assert!(
        is_modelled_ipv4(&address),
        "case {case}: expected the reserved dotted-quad address, got {address:?}"
    );
    address
}

/// Assert `after` is the instance `before` serving on `address` as a static one,
/// with everything an attach does not pin left alone.
fn assert_instance_pinned(case: &str, before: &Value, after: &Value, address: &str, row: &Row) {
    assert_eq!(
        (
            after["isStaticIp"].as_bool(),
            after["publicIpAddress"].as_str()
        ),
        (Some(true), Some(address)),
        "case {case}: expected the instance on the reserved address as a static one, got \
         {after:?}"
    );
    for member in UNTOUCHED_INSTANCE_MEMBERS {
        assert_eq!(
            after[member], before[member],
            "case {case}: an attach must leave {member} alone, expected {:?}, got {:?}",
            before[member], after[member]
        );
    }
    let ipv6 = after["ipv6Addresses"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        !ipv6.is_empty(),
        row.has_ipv6,
        "case {case}: expected the address type's IPv6 addressing to be untouched, got \
         {ipv6:?}"
    );
}

// ---------------------------------------------------------------------------
// The attach
// ---------------------------------------------------------------------------

/// RULE: an attach puts the reserved address onto the instance the request names —
/// the reservation reports the attachment and keeps every other member it has, the
/// instance serves on that address and reports `isStaticIp`, the address it was
/// reachable at before is given up, and the call answers one terminal
/// `AttachStaticIp` entry naming the static IP in the Region it was reserved in.
#[tokio::test]
async fn attaching_a_static_ip_puts_the_instance_on_the_reserved_address() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        allocate(&emulator, None, KEPT).await;
        allocate(&emulator, row.region, row.static_ip_name).await;
        create_row_instance(&emulator, row).await;

        let before_ip = static_ip(&emulator, row.static_ip_name).await;
        let before_instance = instance(&emulator, row.instance_name).await;
        let before_kept = static_ip(&emulator, KEPT).await;
        let created_address = before_instance["publicIpAddress"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            (
                before_ip["isAttached"].as_bool(),
                before_instance["isStaticIp"].as_bool()
            ),
            (Some(false), Some(false)),
            "case {case}: a reservation starts attached to nothing, got {before_ip:?} and \
             {before_instance:?}"
        );

        let entries = attach(
            &emulator,
            case,
            row.region,
            row.static_ip_name,
            row.instance_name,
        )
        .await;
        let id = assert_one_terminal_entry(case, &entries, row.static_ip_name, row.reported_region);

        // Read both records back through the readers a client has.
        let after_ip = static_ip(&emulator, row.static_ip_name).await;
        let address =
            assert_reservation_attached(case, &before_ip, &after_ip, row.instance_name);
        let after_instance = instance(&emulator, row.instance_name).await;
        assert_instance_pinned(case, &before_instance, &after_instance, &address, row);
        assert_ne!(
            address, created_address,
            "case {case}: the instance gave up the ordinary address it was reachable at, got \
             {address:?}"
        );
        assert_eq!(
            static_ip(&emulator, KEPT).await,
            before_kept,
            "case {case}: every other reservation must be untouched"
        );

        // The entry the call answered is the one the store holds against the
        // static IP, read back through the log.
        let logged = logged_operations(&emulator, case, row.static_ip_name).await;
        assert!(
            logged
                .iter()
                .any(|entry| entry["id"].as_str() == Some(id.as_str())
                    && entry["operationType"].as_str() == Some("AttachStaticIp")),
            "case {case}: expected the attach entry {id} in the static IP's log, got {logged:?}"
        );

        // An instance that was still coming up settles as it would have anyway:
        // the attach neither waited for the transition nor moved it.
        if row.settle == Settle::ComingUp {
            wait_for_state(&emulator, row.instance_name, "running").await;
            let settled = instance(&emulator, row.instance_name).await;
            assert_eq!(
                settled["publicIpAddress"].as_str(),
                Some(address.as_str()),
                "case {case}: the reserved address survives the transition the clock owed, \
                 got {settled:?}"
            );
        }
    }
}

/// RULE (the same one, applied twice): attaching the static IP an instance already
/// holds asks for the state the store is already in, so it changes nothing and is
/// not refused — and it answers its own terminal entry, because every mutating
/// call writes one.
#[tokio::test]
async fn attaching_the_same_static_ip_again_changes_nothing() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        allocate(&emulator, row.region, row.static_ip_name).await;
        create_row_instance(&emulator, row).await;

        let first = attach(
            &emulator,
            case,
            row.region,
            row.static_ip_name,
            row.instance_name,
        )
        .await;
        let first_id =
            assert_one_terminal_entry(case, &first, row.static_ip_name, row.reported_region);
        let once_ip = static_ip(&emulator, row.static_ip_name).await;
        let once_instance = instance(&emulator, row.instance_name).await;

        let second = attach(
            &emulator,
            case,
            row.region,
            row.static_ip_name,
            row.instance_name,
        )
        .await;
        let second_id =
            assert_one_terminal_entry(case, &second, row.static_ip_name, row.reported_region);
        assert_ne!(
            first_id, second_id,
            "case {case}: each call writes its own log entry, got {second_id} twice"
        );

        let twice_ip = static_ip(&emulator, row.static_ip_name).await;
        let twice_instance = instance(&emulator, row.instance_name).await;
        assert_eq!(
            twice_ip, once_ip,
            "case {case}: a second attach changes nothing, expected {once_ip:?}, got \
             {twice_ip:?}"
        );
        assert_eq!(
            twice_instance["publicIpAddress"], once_instance["publicIpAddress"],
            "case {case}: a second attach must leave the instance on the same address, \
             expected {:?}, got {:?}",
            once_instance["publicIpAddress"], twice_instance["publicIpAddress"]
        );
        assert_eq!(
            twice_instance["isStaticIp"].as_bool(),
            Some(true),
            "case {case}: the instance still holds a static address, got {twice_instance:?}"
        );
    }
}

/// RULE (the same one, over the detach that releases it): a reservation whose
/// attachment was released attaches again — to the instance it came off, or to
/// another one — at the same reserved address, because a release of the attachment
/// is not a release of the reservation.
#[tokio::test]
async fn a_detached_reservation_attaches_again_at_the_same_address() {
    let emulator = Emulator::new();
    let moved = "moved-ip";
    allocate(&emulator, None, moved).await;
    for name in ["first-holder", "second_holder_1"] {
        create_instance(&emulator, name, "us-east-1a", None, Settle::Running).await;
    }

    let reserved = static_ip(&emulator, moved).await["ipAddress"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The rows differ in the instance the address goes onto, so the reservation is
    // attached, released and attached again both to the same instance and to
    // another one.
    for (case, holder) in [
        ("onto the first instance", "first-holder"),
        ("back onto the same instance", "first-holder"),
        ("onto another instance", "second_holder_1"),
    ] {
        attach(&emulator, case, None, moved, holder).await;
        let attached = static_ip(&emulator, moved).await;
        assert_eq!(
            (
                attached["attachedTo"].as_str(),
                attached["ipAddress"].as_str()
            ),
            (Some(holder), Some(reserved.as_str())),
            "case {case}: expected the same reserved address on {holder}, got {attached:?}"
        );
        let holding = instance(&emulator, holder).await;
        assert_eq!(
            (
                holding["isStaticIp"].as_bool(),
                holding["publicIpAddress"].as_str()
            ),
            (Some(true), Some(reserved.as_str())),
            "case {case}: expected {holder} on the reserved address, got {holding:?}"
        );
        emulator
            .ok("DetachStaticIp", json!({"staticIpName": moved}))
            .await;
    }

    // Neither instance is left holding the address the reservation took back.
    for holder in ["first-holder", "second_holder_1"] {
        let record = instance(&emulator, holder).await;
        let public = record["publicIpAddress"].as_str().unwrap_or_default();
        assert!(
            record["isStaticIp"].as_bool() == Some(false)
                && is_modelled_ipv4(public)
                && public != reserved,
            "case the released reservation: expected {holder} back on an ordinary address, \
             got {record:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What an attach refuses
// ---------------------------------------------------------------------------

/// One refused attach.
struct Refusal {
    case: &'static str,
    static_ip_name: &'static str,
    instance_name: &'static str,
    /// The wire error the request is refused with.
    error: &'static str,
    /// The `code` the body carries, for the service's `DoesNotExist` envelope.
    body_code: Option<&'static str>,
    /// What the message has to name: the resources, and the state of them, that
    /// are in the way.
    names: &'static [&'static str],
}

/// RULE: an attach that cannot be served attaches NOTHING — a name the account
/// holds no static IP or no instance under is the `DoesNotExist` refusal, and a
/// reservation or an instance that already holds another attachment, and an
/// instance with no public IPv4 address for a reservation to pin, are each the
/// modelled operation-failure refusal. Every record the account holds is left
/// exactly as it was.
#[tokio::test]
async fn an_attach_that_cannot_be_served_attaches_nothing() {
    let emulator = Emulator::new();
    // Three instances: one already holding a static IP, one free, and one with no
    // public IPv4 address at all.
    for (name, zone, ip_address_type) in [
        ("holder-host", "us-east-1a", None),
        ("second-host", "us-west-2a", None),
        ("ipv6-host", "us-east-1a", Some("ipv6")),
    ] {
        create_instance(&emulator, name, zone, ip_address_type, Settle::Running).await;
    }
    allocate(&emulator, None, "held-ip").await;
    allocate(&emulator, Some("eu-west-1"), "spare-ip").await;
    attach(&emulator, "the fixture", None, "held-ip", "holder-host").await;

    let before_held = static_ip(&emulator, "held-ip").await;
    let before_spare = static_ip(&emulator, "spare-ip").await;
    let before_instances: Vec<(&str, Value)> = {
        let mut records = Vec::new();
        for name in ["holder-host", "second-host", "ipv6-host"] {
            records.push((name, instance(&emulator, name).await));
        }
        records
    };

    // The rows differ along what makes the attach unservable, and the resource the
    // refusal is about.
    let refusals: &[Refusal] = &[
        Refusal {
            case: "a static IP name nothing ever allocated",
            static_ip_name: "no-such-static-ip",
            instance_name: "second-host",
            error: NOT_FOUND,
            body_code: Some(DOES_NOT_EXIST),
            names: &["StaticIp", "no-such-static-ip"],
        },
        Refusal {
            case: "a static IP name differing only in case",
            static_ip_name: "Held-Ip",
            instance_name: "second-host",
            error: NOT_FOUND,
            body_code: Some(DOES_NOT_EXIST),
            names: &["StaticIp", "Held-Ip"],
        },
        Refusal {
            case: "neither name is held, and the static IP is the one acted on",
            static_ip_name: "zz",
            instance_name: "no-such-instance",
            error: NOT_FOUND,
            body_code: Some(DOES_NOT_EXIST),
            names: &["StaticIp", "zz"],
        },
        Refusal {
            case: "an instance name nothing ever created",
            static_ip_name: "spare-ip",
            instance_name: "no-such-instance",
            error: NOT_FOUND,
            body_code: Some(DOES_NOT_EXIST),
            names: &["Instance", "no-such-instance"],
        },
        Refusal {
            case: "the name of a static IP given as the instance",
            static_ip_name: "spare-ip",
            instance_name: "held-ip",
            error: NOT_FOUND,
            body_code: Some(DOES_NOT_EXIST),
            names: &["Instance", "held-ip"],
        },
        Refusal {
            case: "a static IP already attached to another instance",
            static_ip_name: "held-ip",
            instance_name: "second-host",
            error: OPERATION_FAILURE,
            body_code: None,
            names: &["held-ip", "holder-host"],
        },
        Refusal {
            case: "an instance that already holds another static IP",
            static_ip_name: "spare-ip",
            instance_name: "holder-host",
            error: OPERATION_FAILURE,
            body_code: None,
            names: &["holder-host", "held-ip"],
        },
        Refusal {
            case: "an instance with no public IPv4 address at all",
            static_ip_name: "spare-ip",
            instance_name: "ipv6-host",
            error: OPERATION_FAILURE,
            body_code: None,
            names: &["ipv6-host", "ipv6"],
        },
    ];

    for refusal in refusals {
        let case = refusal.case;
        let (status, body) = emulator
            .call(
                "AttachStaticIp",
                json!({
                    "staticIpName": refusal.static_ip_name,
                    "instanceName": refusal.instance_name,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str(), body["code"].as_str()),
            (400, refusal.error, refusal.body_code),
            "case {case}: expected the {} refusal, got {status} {body:?}",
            refusal.error
        );
        let message = body["message"].as_str().unwrap_or_default();
        for named in refusal.names {
            assert!(
                message.contains(named),
                "case {case}: expected the message to name {named}, got {message:?}"
            );
        }
    }

    // Nothing moved: not the attachment that was in the way, not either free
    // reservation, and not one instance.
    for (what, before, name) in [
        ("the attachment", &before_held, "held-ip"),
        ("the free reservation", &before_spare, "spare-ip"),
    ] {
        assert_eq!(
            &static_ip(&emulator, name).await,
            before,
            "case the refused attaches: expected {what} untouched"
        );
    }
    for (name, before) in &before_instances {
        assert_eq!(
            &instance(&emulator, name).await,
            before,
            "case the refused attaches: expected {name} untouched"
        );
    }
}
