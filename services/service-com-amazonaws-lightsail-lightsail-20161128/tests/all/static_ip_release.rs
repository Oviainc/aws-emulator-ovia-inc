// SPDX-License-Identifier: CC0-1.0

//! `ReleaseStaticIp` — giving up a static IP reservation — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The `Authorization` header carries only a credential SCOPE, which is
//! what names the Region a request is made in.
//!
//! The rule under test: a release "deletes a specific static IP from your
//! account" — the reservation the account held under that name is given up, so
//! every later read of that name is the service's `DoesNotExist` refusal and the
//! name is free to be reserved again, at an address the store has never handed
//! out — and it answers ONE terminal `ReleaseStaticIp` operation-log entry naming
//! it, because the modelled `StaticIp` carries no lifecycle state for the clock to
//! settle. Giving up the reservation gives up the attachment with it: an
//! attachment names "the resources to which" a resource "is attached" (the
//! modelled `StaticIp$attachedTo`), and an address the account no longer holds is
//! not one of them, so the instance that held it reports `isStaticIp` false and an
//! ordinary public IPv4 address the platform assigned it — "to use the same IP
//! address after stopping and starting an instance, create a static IP address and
//! attach it to the instance" (the modelled `StartInstance` documentation), so
//! only an attached static IP ever pinned that address. A name the account does
//! not hold releases nothing, and no other record is touched.
//!
//! The rows span what the rule generalises over: the form and length of the name
//! the modelled `ResourceName` pattern `^\w[\w\-]*\w$` admits, the Region the
//! reservation was made in (including a request that carries no credential scope
//! at all), and whether the static IP is attached to an instance when it is
//! released — and, where it is, which addresses that instance has at all (its
//! modelled `ipAddressType`).

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state::STATIC_IP_PAGE_SIZE;

// The harness, the address and id shape checks, the settle poll, the attachment
// fixture and the checked page walk this test target already publishes: this
// suite carries no second copy of any of them.
use crate::instance_lifecycle::{
    error_code, is_modelled_ipv4, is_uuid_shaped, wait_for_state, Emulator,
};
use crate::instance_start_addresses::attach_static_ip;
use crate::paging::{names, walk_pages};

/// The blueprint and bundle every instance in this suite is created from: a
/// published Linux plan (`src/state.rs`, `BUNDLE_SPECS`). Which plan it is changes
/// nothing about what a release gives up, so one is enough here.
const BLUEPRINT_ID: &str = "amazon_linux_2023";
/// See [`BLUEPRINT_ID`].
const BUNDLE_ID: &str = "nano_3_0";

/// The wire error a Lightsail request for a resource the account does not hold is
/// refused with, and the `code` its body carries.
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// A static IP no case ever releases, allocated beside the one under test so
/// every assertion is about the name the request carried rather than about an
/// empty account.
const KEPT: &str = "kept-static-ip";

/// A name of 64 characters, all admitted by the modelled `ResourceName` pattern.
const LONG_NAME: &str = "static-ip-with-a-very-long-but-entirely-well-formed-name-abcdefg";

/// The instance a row's static IP is attached to when it is released.
struct Attachment {
    instance_name: &'static str,
    /// The zone the instance is created in, which is what decides its Region.
    availability_zone: &'static str,
    /// What the create sends as `ipAddressType`; `None` sends none at all, which
    /// the service defaults to `dualstack`.
    ip_address_type: Option<&'static str>,
    /// Whether that address type gives the instance an IPv6 address, from the
    /// modelled `IpAddressType` enum (`ipv4` is IPv4 only, `dualstack` is both).
    has_ipv6: bool,
}

/// One release case.
struct Row {
    case: &'static str,
    name: &'static str,
    /// The Region the reservation is made and released in, or `None` for a
    /// request that carries no credential scope at all.
    region: Option<&'static str>,
    /// The Region the records and the operation-log entry must report —
    /// `us-east-1` (`crate::state::DEFAULT_REGION`) for a request that names
    /// none.
    reported_region: &'static str,
    /// The instance holding the static IP when it is released, when one does.
    attached: Option<Attachment>,
}

/// The table. The modelled `StaticIp$name` example (`StaticIP-Ohio-EXAMPLE`) is
/// the first row.
const ROWS: &[Row] = &[
    Row {
        case: "the modelled name example, unattached, in no named region",
        name: "StaticIP-Ohio-EXAMPLE",
        region: None,
        reported_region: "us-east-1",
        attached: None,
    },
    Row {
        case: "an underscored name, attached to a dualstack instance",
        name: "static_ip_two",
        region: Some("us-west-2"),
        reported_region: "us-west-2",
        attached: Some(Attachment {
            instance_name: "web_host_1",
            availability_zone: "us-west-2a",
            ip_address_type: None,
            has_ipv6: true,
        }),
    },
    Row {
        case: "the shortest name the modelled pattern admits, unattached",
        name: "ip",
        region: Some("eu-west-1"),
        reported_region: "eu-west-1",
        attached: None,
    },
    Row {
        case: "a digit-led mixed name, attached to an IPv4-only instance",
        name: "0a-static_ip-3",
        region: Some("ap-southeast-2"),
        reported_region: "ap-southeast-2",
        attached: Some(Attachment {
            instance_name: "edge-host-2",
            availability_zone: "ap-southeast-2b",
            ip_address_type: Some("ipv4"),
            has_ipv6: false,
        }),
    },
    Row {
        case: "a long name, unattached, in a region on another continent",
        name: LONG_NAME,
        region: Some("sa-east-1"),
        reported_region: "sa-east-1",
        attached: None,
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

/// Release the static IP `name` in `region`, answering the `operations` the call
/// reported.
async fn release(emulator: &Emulator, case: &str, region: Option<&str>, name: &str) -> Vec<Value> {
    let body = emulator
        .ok_at(None, region, "ReleaseStaticIp", json!({"staticIpName": name}))
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

/// Create the attachment's instance and wait until it is serving.
async fn create_running_instance(emulator: &Emulator, attachment: &Attachment) {
    let mut body = json!({
        "instanceNames": [attachment.instance_name],
        "availabilityZone": attachment.availability_zone,
        "blueprintId": BLUEPRINT_ID,
        "bundleId": BUNDLE_ID,
    });
    if let Some(ip_address_type) = attachment.ip_address_type {
        body["ipAddressType"] = json!(ip_address_type);
    }
    emulator.ok("CreateInstances", body).await;
    wait_for_state(emulator, attachment.instance_name, "running").await;
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
// The release
// ---------------------------------------------------------------------------

/// RULE: a release gives up the reservation the account held under that name —
/// every later read of it is the `DoesNotExist` refusal, the listing no longer
/// carries it, and the name is free to be reserved again at an address the store
/// has never handed out — and it answers one terminal `ReleaseStaticIp` entry
/// naming it in the Region it was reserved in.
#[tokio::test]
async fn releasing_a_static_ip_gives_up_the_reservation() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        allocate(&emulator, None, KEPT).await;
        allocate(&emulator, row.region, row.name).await;
        let before = static_ip(&emulator, row.name).await;
        let released_address = before["ipAddress"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if let Some(attachment) = &row.attached {
            create_running_instance(&emulator, attachment).await;
            let attached = attach_static_ip(&emulator, row.name, attachment.instance_name).await;
            assert_eq!(
                attached, released_address,
                "case {case}: the attachment must hold the reserved address, got {attached:?}"
            );
        }

        // The log: one entry, for the one static IP the call released, already
        // terminal because a reservation is given up as the call returns.
        let entries = release(&emulator, case, row.region, row.name).await;
        assert_eq!(
            entries.len(),
            1,
            "case {case}: expected exactly one operation, got {entries:?}"
        );
        let entry = entries[0].clone();
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
                Some("ReleaseStaticIp"),
                Some("StaticIp"),
                Some(row.name),
                Some("Succeeded"),
                Some(true),
                Some(row.reported_region),
            ),
            "case {case}: expected a succeeded ReleaseStaticIp entry, got {entry:?}"
        );
        let id = entry["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&id),
            "case {case}: expected a uuid-shaped operation id, got {id:?}"
        );

        // The reservation is gone from every read...
        let (status, body) = emulator
            .call("GetStaticIp", json!({"staticIpName": row.name}))
            .await;
        assert_does_not_exist(case, row.name, status, &body);
        let held = held_names(&emulator, case).await;
        assert_eq!(
            held,
            vec![KEPT.to_string()],
            "case {case}: expected only the static IP the case kept, got {held:?}"
        );

        // ...so a second release has nothing to release.
        let (status, body) = emulator
            .call_in(row.region, "ReleaseStaticIp", json!({"staticIpName": row.name}))
            .await;
        assert_does_not_exist(case, row.name, status, &body);

        // The name is free again, and what it names now is a NEW reservation:
        // the released address was given up, not handed back.
        allocate(&emulator, row.region, row.name).await;
        let after = static_ip(&emulator, row.name).await;
        let reserved = after["ipAddress"].as_str().unwrap_or_default().to_string();
        assert!(
            is_modelled_ipv4(&reserved),
            "case {case}: expected a modelled IpAddress, got {reserved:?}"
        );
        assert_ne!(
            reserved, released_address,
            "case {case}: expected an address the store has never handed out, got the \
             released {reserved:?}"
        );
        assert_ne!(
            after["arn"], before["arn"],
            "case {case}: expected a new record, got the released ARN {:?}",
            after["arn"]
        );
        assert_eq!(
            (
                after["isAttached"].as_bool(),
                after["attachedTo"].as_str(),
                after["location"]["regionName"].as_str(),
            ),
            (Some(false), None, Some(row.reported_region)),
            "case {case}: expected a fresh unattached reservation, got {after:?}"
        );
    }
}

/// RULE (the attachment half): an address the account no longer holds is not one
/// of the resources an instance is attached to, so releasing an attached static IP
/// leaves the instance reporting `isStaticIp` false at an ordinary public IPv4
/// address — one no resource in this store has held — while the private address,
/// the IPv6 addresses and every other member of the record stay where they are.
#[tokio::test]
async fn releasing_an_attached_static_ip_leaves_the_instance_on_an_ordinary_address() {
    for row in ROWS {
        let Some(attachment) = &row.attached else {
            continue;
        };
        let case = row.case;
        let emulator = Emulator::new();
        create_running_instance(&emulator, attachment).await;
        let created = instance(&emulator, attachment.instance_name).await;
        let created_address = created["publicIpAddress"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        allocate(&emulator, row.region, row.name).await;
        let static_address = attach_static_ip(&emulator, row.name, attachment.instance_name).await;

        // What a client sees while the attachment stands: the instance is
        // reachable at the reserved address, and says so.
        let attached = instance(&emulator, attachment.instance_name).await;
        assert_eq!(
            (
                attached["isStaticIp"].as_bool(),
                attached["publicIpAddress"].as_str(),
            ),
            (Some(true), Some(static_address.as_str())),
            "case {case}: expected the instance on the reserved address, got {attached:?}"
        );

        release(&emulator, case, row.region, row.name).await;

        let after = instance(&emulator, attachment.instance_name).await;
        assert_eq!(
            after["isStaticIp"].as_bool(),
            Some(false),
            "case {case}: no static IP is held any more, got {after:?}"
        );
        let public = after["publicIpAddress"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_ipv4(&public),
            "case {case}: expected an ordinary public IPv4 address, got {public:?}"
        );
        for (what, spent) in [
            ("the released reservation", &static_address),
            ("the address it had before the attachment", &created_address),
        ] {
            assert_ne!(
                &public, spent,
                "case {case}: expected an address the store has never handed out, got {what} \
                 {public:?}"
            );
        }
        // Everything a release does not re-provision reads back identical: a
        // static IP reserves ONE public IPv4 address, so it pinned nothing else.
        for member in [
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
        ] {
            assert_eq!(
                after[member], attached[member],
                "case {case}: a release must leave {member} alone, expected {:?}, got {:?}",
                attached[member], after[member]
            );
        }
        let ipv6 = after["ipv6Addresses"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            !ipv6.is_empty(),
            attachment.has_ipv6,
            "case {case}: expected the address type's IPv6 addressing to be untouched, got \
             {ipv6:?}"
        );

        // And the reservation itself is gone.
        let (status, body) = emulator
            .call("GetStaticIp", json!({"staticIpName": row.name}))
            .await;
        assert_does_not_exist(case, row.name, status, &body);
    }
}

/// RULE: a well-formed name the account does not hold releases nothing — the
/// `DoesNotExist` refusal — and leaves every record the account does hold exactly
/// as it was, the attachment one of them included.
#[tokio::test]
async fn a_name_the_account_does_not_hold_releases_nothing() {
    let emulator = Emulator::new();
    let holder = "holder-host";
    let attachment = Attachment {
        instance_name: holder,
        availability_zone: "us-east-1a",
        ip_address_type: None,
        has_ipv6: true,
    };
    create_running_instance(&emulator, &attachment).await;
    allocate(&emulator, None, KEPT).await;
    allocate(&emulator, None, "attached-static-ip").await;
    attach_static_ip(&emulator, "attached-static-ip", holder).await;

    let before_kept = static_ip(&emulator, KEPT).await;
    let before_attached = static_ip(&emulator, "attached-static-ip").await;
    let before_instance = instance(&emulator, holder).await;

    // The rows differ along what makes the name unheld: never allocated, a
    // difference in case only, the shortest and a long well-formed name.
    let cases: &[(&str, &str)] = &[
        ("a name nothing ever allocated", "no-such-static-ip"),
        ("a name differing only in case", "Kept-Static-Ip"),
        ("the shortest well-formed name", "zz"),
        ("a long well-formed name", LONG_NAME),
    ];
    for (case, name) in cases {
        let (status, body) = emulator
            .call("ReleaseStaticIp", json!({"staticIpName": name}))
            .await;
        assert_does_not_exist(case, name, status, &body);
    }

    let after_kept = static_ip(&emulator, KEPT).await;
    let after_attached = static_ip(&emulator, "attached-static-ip").await;
    let after_instance = instance(&emulator, holder).await;
    assert_eq!(
        after_kept, before_kept,
        "case the refused releases: the static IP the account holds must be untouched, got \
         {after_kept:?}"
    );
    assert_eq!(
        after_attached, before_attached,
        "case the refused releases: the attachment must be untouched, got {after_attached:?}"
    );
    assert_eq!(
        after_instance, before_instance,
        "case the refused releases: the instance must be untouched, got {after_instance:?}"
    );
}

/// RULE (the same one, over a set): a release gives up the ONE reservation it
/// names — every other static IP the account holds, and every attachment another
/// instance holds, is left exactly as it was.
#[tokio::test]
async fn releasing_one_static_ip_leaves_every_other_alone() {
    let emulator = Emulator::new();
    // Two instances, each holding a static IP, so nothing but the name tells the
    // two reservations apart.
    let doomed = Attachment {
        instance_name: "doomed-host",
        availability_zone: "us-east-1a",
        ip_address_type: None,
        has_ipv6: true,
    };
    let kept = Attachment {
        instance_name: "kept_host_1",
        availability_zone: "us-east-1a",
        ip_address_type: Some("ipv4"),
        has_ipv6: false,
    };
    for attachment in [&doomed, &kept] {
        create_running_instance(&emulator, attachment).await;
    }
    // A third reservation, held by nothing at all.
    for name in ["doomed-ip", "kept-ip", "spare-ip"] {
        allocate(&emulator, None, name).await;
    }
    attach_static_ip(&emulator, "doomed-ip", doomed.instance_name).await;
    let kept_address = attach_static_ip(&emulator, "kept-ip", kept.instance_name).await;

    let before_kept_ip = static_ip(&emulator, "kept-ip").await;
    let before_spare = static_ip(&emulator, "spare-ip").await;
    let before_kept_host = instance(&emulator, kept.instance_name).await;

    release(&emulator, "one of three", None, "doomed-ip").await;

    let held = held_names(&emulator, "one of three").await;
    assert_eq!(
        held,
        vec!["kept-ip".to_string(), "spare-ip".to_string()],
        "case one of three: expected the other two reservations in name order, got {held:?}"
    );
    assert_eq!(
        static_ip(&emulator, "kept-ip").await,
        before_kept_ip,
        "case one of three: expected the other attachment untouched"
    );
    assert_eq!(
        static_ip(&emulator, "spare-ip").await,
        before_spare,
        "case one of three: expected the unattached reservation untouched"
    );
    let kept_host = instance(&emulator, kept.instance_name).await;
    assert_eq!(
        (
            kept_host["isStaticIp"].as_bool(),
            kept_host["publicIpAddress"].as_str(),
        ),
        (Some(true), Some(kept_address.as_str())),
        "case one of three: expected the other holder still on its reserved address, got \
         {kept_host:?}"
    );
    assert_eq!(
        kept_host, before_kept_host,
        "case one of three: expected the other holder's record untouched, got {kept_host:?}"
    );
}
