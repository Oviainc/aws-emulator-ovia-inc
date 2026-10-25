// SPDX-License-Identifier: CC0-1.0

//! `StartInstance` — the public addresses Lightsail assigns an instance as it is
//! started, driven over the wire through the same front door a client uses.
//!
//! The rule under test: starting an instance that was not running assigns it NEW
//! public addressing — a freshly drawn public IPv4 address and, where its
//! `ipAddressType` gives it one, a freshly drawn IPv6 address — unless a static
//! IP is attached to it, whose reserved IPv4 address is kept. "When you start a
//! stopped instance, Lightsail assigns a new public IP address to the instance.
//! To use the same IP address after stopping and starting an instance, create a
//! static IP address and attach it to the instance" (the modelled
//! `StartInstance` and `StopInstance` documentation). Everything else the record
//! holds is untouched — the private address the instance keeps in the Region's
//! VPC above all — and a call that starts nothing (the instance is already
//! running, or already coming up) assigns nothing, just as a reboot
//! re-provisions nothing (`instance_reboot.rs`).
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over:
//! which addresses the instance has at all (its `ipAddressType`), how the stop it
//! is started from was made (`force`, so also whether the stop had settled), and
//! how many stop/start cycles it has been through — every address ever handed out
//! is distinct, so the rule is an invariant over cycles rather than one
//! before/after pair.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state as lightsail_state;

// The harness, the address shape checks and the settle-state poll are the ones
// `instance_lifecycle.rs` publishes to this test target, so this suite carries no
// second copy of any of them.
use crate::instance_lifecycle::{is_modelled_ipv4, is_modelled_ipv6, wait_for_state, Emulator};

/// One instance to cycle. The rows differ in the IP address type, which decides
/// which addresses the instance has at all, and in blueprint, bundle and zone so
/// no row is the recorded one.
#[derive(Debug)]
struct Row {
    case: &'static str,
    availability_zone: &'static str,
    blueprint_id: &'static str,
    bundle_id: &'static str,
    /// What the request sends; `None` sends no `ipAddressType` at all, which the
    /// service defaults to `dualstack`.
    ip_address_type: Option<&'static str>,
    /// Whether the instance has a public IPv4 address, and whether it has an IPv6
    /// address. From the modelled `IpAddressType` enum: `ipv4` is IPv4 only,
    /// `ipv6` is IPv6 only and `dualstack` is both.
    has_ipv4: bool,
    has_ipv6: bool,
}

/// The table every case here cycles.
const ROWS: &[Row] = &[
    Row {
        case: "no ipAddressType, which defaults to dualstack",
        availability_zone: "us-east-1a",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "micro_3_0",
        ip_address_type: None,
        has_ipv4: true,
        has_ipv6: true,
    },
    Row {
        case: "ipv4 only, so there is no IPv6 address to redraw",
        availability_zone: "us-west-2b",
        blueprint_id: "ubuntu_22_04",
        bundle_id: "nano_3_0",
        ip_address_type: Some("ipv4"),
        has_ipv4: true,
        has_ipv6: false,
    },
    Row {
        case: "ipv6 only, so there is no public IPv4 address at all",
        availability_zone: "ap-northeast-1c",
        blueprint_id: "windows_server_2022",
        bundle_id: "large_3_0",
        ip_address_type: Some("ipv6"),
        has_ipv4: false,
        has_ipv6: true,
    },
    Row {
        case: "dualstack asked for explicitly, application blueprint",
        availability_zone: "eu-west-3a",
        blueprint_id: "wordpress",
        bundle_id: "4xlarge_3_0",
        ip_address_type: Some("dualstack"),
        has_ipv4: true,
        has_ipv6: true,
    },
];

/// How many stop/start cycles each row goes through: enough that a redraw has to
/// be NEW every time rather than merely alternating between two addresses.
const CYCLES: usize = 3;

/// The `CreateInstances` body for `row`, naming one instance.
fn create_body(row: &Row, name: &str) -> Value {
    let mut body = json!({
        "instanceNames": [name],
        "availabilityZone": row.availability_zone,
        "blueprintId": row.blueprint_id,
        "bundleId": row.bundle_id,
    });
    if let Some(ip_address_type) = row.ip_address_type {
        body["ipAddressType"] = json!(ip_address_type);
    }
    body
}

/// The stored instance `name`, as `GetInstance` reports it.
async fn instance(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetInstance", json!({"instanceName": name})).await["instance"].clone()
}

/// Every public address this suite has seen reported, so a redraw can be asserted
/// to be an address never handed out before rather than merely different from the
/// one immediately before it.
#[derive(Debug, Default)]
struct SeenAddresses {
    ipv4: BTreeSet<String>,
    ipv6: BTreeSet<String>,
}

/// Assert the addresses `instance` reports are the ones its row's address type
/// gives it, are well formed, and have never been reported before — recording
/// them in `seen`.
fn assert_fresh_addresses(case: &str, instance: &Value, row: &Row, seen: &mut SeenAddresses) {
    match instance["publicIpAddress"].as_str() {
        Some(address) => {
            assert!(
                row.has_ipv4,
                "case {case}: an instance with ipAddressType {:?} has no public IPv4 \
                 address, got {address:?}",
                row.ip_address_type
            );
            assert!(
                is_modelled_ipv4(address),
                "case {case}: expected a dotted-quad public IPv4 address, got {address:?}"
            );
            assert!(
                seen.ipv4.insert(address.to_string()),
                "case {case}: expected a public IPv4 address never assigned before, got \
                 {address:?} again (already seen {:?})",
                seen.ipv4
            );
        }
        None => assert!(
            !row.has_ipv4,
            "case {case}: an instance with ipAddressType {:?} must report a public IPv4 \
             address, got {instance:?}",
            row.ip_address_type
        ),
    }

    let ipv6: Vec<String> = instance["ipv6Addresses"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ipv6.len(),
        usize::from(row.has_ipv6),
        "case {case}: an instance with ipAddressType {:?} reports exactly the IPv6 \
         addresses it has, got {ipv6:?}",
        row.ip_address_type
    );
    for address in ipv6 {
        assert!(
            is_modelled_ipv6(&address),
            "case {case}: expected a modelled IPv6 address, got {address:?}"
        );
        assert!(
            seen.ipv6.insert(address.clone()),
            "case {case}: expected an IPv6 address never assigned before, got {address:?} \
             again (already seen {:?})",
            seen.ipv6
        );
    }
}

/// Assert everything a start does NOT re-provision reads back identical.
fn assert_everything_else_unchanged(case: &str, before: &Value, after: &Value) {
    for member in [
        // The instance keeps its place in the Region's VPC across a stop.
        "privateIpAddress",
        "name",
        "arn",
        "supportCode",
        "createdAt",
        "location",
        "ipAddressType",
        "blueprintId",
        "bundleId",
        "sshKeyName",
        "hardware",
        "networking",
        "addOns",
        "tags",
        "isStaticIp",
    ] {
        assert_eq!(
            after[member], before[member],
            "case {case}: a start must leave {member} alone, expected {:?}, got {:?}",
            before[member], after[member]
        );
    }
    assert_eq!(
        after["isStaticIp"].as_bool(),
        Some(false),
        "case {case}: no static IP is attached, so the new address is not a static one, \
         got {after:?}"
    );
}

/// Attach the static IP `static_ip_name` to the instance `instance_name` in the
/// store, answering the reserved address.
///
/// `AttachStaticIp` is still a 501 stub, so the attachment the rule's exception is
/// about cannot be made through the API yet. It is made here where the store holds
/// it — on the STATIC IP record, which names the instance it is attached to —
/// through the same public state API a handler uses, so the exception is exercised
/// against the one persisted record rather than a copy. Writing what that
/// operation will write, so it stays correct once the stub is implemented.
fn attach_static_ip(emulator: &Emulator, static_ip_name: &str, instance_name: &str) -> String {
    lightsail_state::with_state(emulator.engine(), |store| {
        let static_ip = store
            .static_ips
            .get_mut(static_ip_name)
            .expect("the suite allocated this static IP through AllocateStaticIp");
        static_ip.attached_to = Some(instance_name.to_string());
        let address = static_ip.ip_address.clone();
        let instance = store
            .instance_mut(instance_name)
            .expect("the suite created this instance through CreateInstances");
        instance.is_static_ip = true;
        instance.public_ip_address = Some(address.clone());
        address
    })
    .expect("the suite's engine is the shared in-memory one")
}

/// Release the attachment made by [`attach_static_ip`], leaving the address
/// allocated to the account.
///
/// Only the attachment is released: what `DetachStaticIp` then does to the
/// instance's own address is that operation's business, and it is still a stub.
fn detach_static_ip(emulator: &Emulator, static_ip_name: &str, instance_name: &str) {
    lightsail_state::with_state(emulator.engine(), |store| {
        store
            .static_ips
            .get_mut(static_ip_name)
            .expect("the suite allocated this static IP through AllocateStaticIp")
            .attached_to = None;
        store
            .instance_mut(instance_name)
            .expect("the suite created this instance through CreateInstances")
            .is_static_ip = false;
    })
    .expect("the suite's engine is the shared in-memory one");
}

// ---------------------------------------------------------------------------
// The redraw
// ---------------------------------------------------------------------------

/// RULE: every start of an instance that was not running assigns it public
/// addressing drawn afresh — a public IPv4 address, and an IPv6 address where its
/// address type gives it one, neither of which was ever assigned before — while
/// the private address and every other member of the record stay where they are.
#[tokio::test]
async fn starting_a_stopped_instance_assigns_new_public_addresses() {
    for row in ROWS {
        for force in [false, true] {
            let case = format!("{}, force={force}", row.case);
            let emulator = Emulator::new();
            let name = "cycled";
            emulator
                .ok("CreateInstances", create_body(row, name))
                .await;
            wait_for_state(&emulator, name, "running").await;

            // The addresses the create assigned are the first ones handed out.
            let mut seen = SeenAddresses::default();
            let created = instance(&emulator, name).await;
            assert_fresh_addresses(&format!("{case}, as created"), &created, row, &mut seen);

            for cycle in 1..=CYCLES {
                let case = format!("{case}, cycle {cycle}");
                let before = instance(&emulator, name).await;

                let mut stop = json!({"instanceName": name});
                if force {
                    stop["force"] = json!(true);
                }
                emulator.ok("StopInstance", stop).await;
                wait_for_state(&emulator, name, "stopped").await;
                // A stop assigns nothing: the rule is about what STARTING the
                // instance assigns.
                let stopped = instance(&emulator, name).await;
                assert_eq!(
                    (
                        stopped["publicIpAddress"].clone(),
                        stopped["ipv6Addresses"].clone()
                    ),
                    (
                        before["publicIpAddress"].clone(),
                        before["ipv6Addresses"].clone()
                    ),
                    "case {case}: the stop must not redraw anything, got {stopped:?}"
                );

                emulator
                    .ok("StartInstance", json!({"instanceName": name}))
                    .await;
                // The assignment belongs to the START, not to the clock that
                // settles the instance into `running`: it is already reported
                // while the instance is still coming up, and it does not move
                // again as it settles.
                let coming_up = instance(&emulator, name).await;
                assert_fresh_addresses(&case, &coming_up, row, &mut seen);
                assert_everything_else_unchanged(&case, &before, &coming_up);

                wait_for_state(&emulator, name, "running").await;
                let running = instance(&emulator, name).await;
                assert_eq!(
                    (
                        running["publicIpAddress"].clone(),
                        running["ipv6Addresses"].clone()
                    ),
                    (
                        coming_up["publicIpAddress"].clone(),
                        coming_up["ipv6Addresses"].clone()
                    ),
                    "case {case}: the addresses the start assigned must not move as the \
                     instance settles, got {running:?}"
                );
            }

            // One distinct address per assignment: the create, then one per start.
            if row.has_ipv4 {
                assert_eq!(
                    seen.ipv4.len(),
                    CYCLES + 1,
                    "case {case}: expected a distinct public IPv4 address for the create \
                     and for each of the {CYCLES} starts, got {:?}",
                    seen.ipv4
                );
            }
            if row.has_ipv6 {
                assert_eq!(
                    seen.ipv6.len(),
                    CYCLES + 1,
                    "case {case}: expected a distinct IPv6 address for the create and for \
                     each of the {CYCLES} starts, got {:?}",
                    seen.ipv6
                );
            }
        }
    }
}

/// RULE: an instance that is on its way DOWN but has not settled yet is still
/// started from a state in which it was not running, so the start assigns it new
/// public addressing too.
#[tokio::test]
async fn starting_an_instance_that_is_still_stopping_assigns_new_addresses() {
    let row = &ROWS[3];
    let emulator = Emulator::new();
    let name = "turned-around";
    emulator
        .ok("CreateInstances", create_body(row, name))
        .await;
    wait_for_state(&emulator, name, "running").await;

    let mut seen = SeenAddresses::default();
    let before = instance(&emulator, name).await;
    assert_fresh_addresses("still stopping, as created", &before, row, &mut seen);

    // No wait and no `force`: the instance is `stopping` for as long as the clock
    // owes it `stopped`.
    emulator
        .ok("StopInstance", json!({"instanceName": name}))
        .await;
    emulator
        .ok("StartInstance", json!({"instanceName": name}))
        .await;

    let after = instance(&emulator, name).await;
    assert_fresh_addresses("still stopping", &after, row, &mut seen);
    assert_everything_else_unchanged("still stopping", &before, &after);
    wait_for_state(&emulator, name, "running").await;
}

/// RULE: a start that starts nothing assigns nothing — an instance that is
/// already running, or already coming up, keeps the addresses it is serving on.
#[tokio::test]
async fn starting_an_instance_that_is_already_up_assigns_nothing() {
    for (case, already_running) in [("already running", true), ("still coming up", false)] {
        let emulator = Emulator::new();
        let name = "already-up";
        emulator
            .ok("CreateInstances", create_body(&ROWS[0], name))
            .await;
        if already_running {
            wait_for_state(&emulator, name, "running").await;
        }

        let before = instance(&emulator, name).await;
        emulator
            .ok("StartInstance", json!({"instanceName": name}))
            .await;
        let after = instance(&emulator, name).await;
        assert_eq!(
            (
                after["publicIpAddress"].clone(),
                after["ipv6Addresses"].clone()
            ),
            (
                before["publicIpAddress"].clone(),
                before["ipv6Addresses"].clone()
            ),
            "case {case}: starting it again must assign nothing, got {after:?}"
        );

        // Nor does the transition the clock owes it redraw anything as it settles.
        wait_for_state(&emulator, name, "running").await;
        let settled = instance(&emulator, name).await;
        assert_eq!(
            (
                settled["publicIpAddress"].clone(),
                settled["ipv6Addresses"].clone()
            ),
            (
                before["publicIpAddress"].clone(),
                before["ipv6Addresses"].clone()
            ),
            "case {case}: the clock must not redraw anything either, got {settled:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The static IP that keeps the address
// ---------------------------------------------------------------------------

/// RULE: the one exception — an instance a static IP is attached to keeps that
/// reserved public IPv4 address across every stop and start, which is the whole
/// reason a static IP exists; the IPv6 address is still drawn afresh, because a
/// static IP reserves one public IPv4 address and nothing else. Release the
/// attachment and the next start draws a public IPv4 address again.
#[tokio::test]
async fn an_instance_with_a_static_ip_attached_keeps_its_public_ipv4_address() {
    let row = &ROWS[3];
    let emulator = Emulator::new();
    let name = "pinned";
    emulator
        .ok("CreateInstances", create_body(row, name))
        .await;
    wait_for_state(&emulator, name, "running").await;
    emulator
        .ok("AllocateStaticIp", json!({"staticIpName": "reserved"}))
        .await;

    // The attachment is made in the store, because the operation that makes it is
    // still a stub — but this case does not depend on that staying true: an
    // emulator that already attaches it through the API answers 200, and the
    // store-side setup then re-applies exactly what that call made.
    let (status, body) = emulator
        .call(
            "AttachStaticIp",
            json!({"instanceName": name, "staticIpName": "reserved"}),
        )
        .await;
    assert!(
        matches!(status, 200 | 501),
        "case the static IP exception: expected AttachStaticIp either to attach the \
         address or to answer the scaffold's 501, got {status} {body:?}"
    );
    let reserved = attach_static_ip(&emulator, "reserved", name);

    // The record the cycles below are measured against: `publicIpAddress` is what
    // the attachment made it, so only what the STARTS do to it is asserted.
    let before = instance(&emulator, name).await;

    let mut seen_ipv6 = BTreeSet::new();
    seen_ipv6.insert(
        before["ipv6Addresses"][0]
            .as_str()
            .expect("a dualstack instance reports one IPv6 address")
            .to_string(),
    );
    for cycle in 1..=2 {
        let case = format!("the static IP exception, cycle {cycle}");
        emulator
            .ok("StopInstance", json!({"instanceName": name, "force": true}))
            .await;
        emulator
            .ok("StartInstance", json!({"instanceName": name}))
            .await;

        let after = instance(&emulator, name).await;
        assert_eq!(
            after["publicIpAddress"].as_str(),
            Some(reserved.as_str()),
            "case {case}: an attached static IP keeps the address across a stop and \
             start, got {after:?}"
        );
        assert_eq!(
            after["privateIpAddress"], before["privateIpAddress"],
            "case {case}: the private address is unchanged, got {after:?}"
        );
        let ipv6 = after["ipv6Addresses"][0]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_ipv6(&ipv6),
            "case {case}: expected a modelled IPv6 address, got {ipv6:?}"
        );
        assert!(
            seen_ipv6.insert(ipv6.clone()),
            "case {case}: a static IP reserves a public IPv4 address only, so the IPv6 \
             address is still drawn afresh, got {ipv6:?} again (already seen {seen_ipv6:?})"
        );
    }

    // Release the attachment: the exception no longer holds, so the next start
    // draws a public IPv4 address of its own again.
    detach_static_ip(&emulator, "reserved", name);
    emulator
        .ok("StopInstance", json!({"instanceName": name, "force": true}))
        .await;
    emulator
        .ok("StartInstance", json!({"instanceName": name}))
        .await;
    let after = instance(&emulator, name).await;
    let drawn = after["publicIpAddress"].as_str().unwrap_or_default();
    assert!(
        is_modelled_ipv4(drawn) && drawn != reserved,
        "case the static IP exception, once detached: expected a newly drawn public IPv4 \
         address, got {after:?}"
    );
    // The address stays ALLOCATED to the account: only the attachment was
    // released, so `GetStaticIp` still reports the reservation.
    let static_ip = emulator
        .ok("GetStaticIp", json!({"staticIpName": "reserved"}))
        .await;
    assert_eq!(
        static_ip["staticIp"]["ipAddress"].as_str(),
        Some(reserved.as_str()),
        "case the static IP exception, once detached: the reservation is unchanged, got \
         {static_ip:?}"
    );
}
