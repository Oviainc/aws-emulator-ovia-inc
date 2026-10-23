// SPDX-License-Identifier: CC0-1.0

//! The Lightsail static IP — allocate and read — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The `Authorization` header carries only a credential SCOPE, which is
//! what names the Region a request is made in.
//!
//! The rule under test: `AllocateStaticIp` reserves ONE static IP record per
//! `staticIpName` the request names, in the Region the request is made in — the
//! name kept verbatim, every server-assigned member (ARN, support code, creation
//! time, location, resource type, the reserved address) computed from the request
//! plus the store's own allocators, the address unique to that record and stable
//! across every later read, and the static IP attached to nothing until
//! something attaches it — so a later `GetStaticIp` reports exactly that record;
//! the allocation answers one TERMINAL `AllocateStaticIp` operation-log entry
//! naming it, because the modelled `StaticIp` carries no lifecycle state; a name
//! the account already holds is refused and changes nothing; and a name that was
//! never allocated is refused with the service's `DoesNotExist` envelope.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state::MAX_STATIC_IPS;

use crate::instance_lifecycle::{error_code, is_modelled_ipv4, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The region a request that carries no SigV4 credential scope is served in
/// (`crate::state::DEFAULT_REGION`).
const DEFAULT_REGION: &str = "us-east-1";

/// Every member the modelled `StaticIp` structure declares, in wire spelling
/// (`smithy_query(query="shape",shape="StaticIp")`).
///
/// `attachedTo` is the one member a static IP attached to nothing does not
/// report, so it is deliberately absent from this set.
const STATIC_IP_MEMBERS: &[&str] = &[
    "arn",
    "createdAt",
    "ipAddress",
    "isAttached",
    "location",
    "name",
    "resourceType",
    "supportCode",
];

/// One allocation case: the name the request carries and the Region it is made
/// in.
///
/// The rows differ along everything the rule generalises over: the form and
/// length of the name the modelled `ResourceName` pattern `^\w[\w\-]*\w$` admits
/// (hyphens, underscores, a leading digit, the shortest name the pattern allows,
/// a long one) and the Region the call is made in — including a request that
/// carries no credential scope at all.
struct Row {
    case: &'static str,
    name: &'static str,
    /// The region the request is made in, or `None` for a request that carries
    /// no credential scope at all.
    region: Option<&'static str>,
    /// The region the static IP must report — [`DEFAULT_REGION`] for a request
    /// that names none.
    reported_region: &'static str,
}

/// A name of 64 characters, all admitted by the modelled `ResourceName` pattern.
const LONG_NAME: &str =
    "static-ip-with-a-very-long-but-entirely-well-formed-name-abcdefg";

/// The table. The modelled `StaticIp$name` example (`StaticIP-Ohio-EXAMPLE`) is
/// the first row.
const ROWS: &[Row] = &[
    Row {
        case: "the modelled name example, in no named region",
        name: "StaticIP-Ohio-EXAMPLE",
        region: None,
        reported_region: DEFAULT_REGION,
    },
    Row {
        case: "an underscored name, in another region",
        name: "static_ip_two",
        region: Some("us-west-2"),
        reported_region: "us-west-2",
    },
    Row {
        case: "the shortest name the modelled pattern admits",
        name: "ip",
        region: Some("eu-west-1"),
        reported_region: "eu-west-1",
    },
    Row {
        case: "a digit-led mixed name",
        name: "0a-static_ip-3",
        region: Some("ap-southeast-2"),
        reported_region: "ap-southeast-2",
    },
    Row {
        case: "a long name, in a region on another continent",
        name: LONG_NAME,
        region: Some("sa-east-1"),
        reported_region: "sa-east-1",
    },
];

/// Allocate the static IP `name` in `region`, answering the `operations` the call
/// reported.
async fn allocate(emulator: &Emulator, region: Option<&str>, name: &str) -> Vec<Value> {
    let body = emulator
        .ok_at(None, region, "AllocateStaticIp", json!({"staticIpName": name}))
        .await;
    body["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case allocating {name}: expected an operations array, got {body:?}"))
        .clone()
}

/// The stored static IP `name`, as `GetStaticIp` reports it when read in
/// `region`.
async fn read(emulator: &Emulator, region: Option<&str>, name: &str) -> Value {
    emulator
        .ok_at(None, region, "GetStaticIp", json!({"staticIpName": name}))
        .await["staticIp"]
        .clone()
}

/// RULE: every member `GetStaticIp` reports comes from the one record
/// `AllocateStaticIp` stored — the caller's name verbatim, the Region of the
/// request, and every server-assigned member computed from them plus the store's
/// own allocators.
#[tokio::test]
async fn a_static_ip_reads_back_as_it_was_allocated() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut addresses: BTreeSet<String> = BTreeSet::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let name = row.name;

        // The allocation answers ONE operation-log entry — it allocates exactly
        // one static IP — and it is already terminal, because a reserved address
        // is reportable the moment the call returns.
        let operations = allocate(&emulator, row.region, name).await;
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {operations:?}"
        );
        let operation = operations[0].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("AllocateStaticIp"),
            "case {case}: expected the AllocateStaticIp operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("StaticIp"),
            "case {case}: expected the StaticIp resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(name),
            "case {case}: expected the operation to name the static IP, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Succeeded"),
            "case {case}: expected a Succeeded operation, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(true),
            "case {case}: a static IP has no lifecycle state to wait on, so the \
             allocation is terminal, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(row.reported_region),
            "case {case}: expected the operation in the region of the request, got \
             {operation:?}"
        );
        let operation_id = operation["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&operation_id),
            "case {case}: expected a uuid-shaped operation id, got {operation_id:?}"
        );
        assert!(
            operation_ids.insert(operation_id.clone()),
            "case {case}: expected a fresh operation id, got the already-used \
             {operation_id:?}"
        );

        // The stored record, read back through the implemented reader.
        let static_ip = read(&emulator, row.region, name).await;
        assert_eq!(
            static_ip["name"].as_str(),
            Some(name),
            "case {case}: expected the static IP name the request carried, got {static_ip:?}"
        );
        assert_eq!(
            static_ip["resourceType"].as_str(),
            Some("StaticIp"),
            "case {case}: expected the StaticIp resource type, got {static_ip:?}"
        );
        assert_eq!(
            static_ip["location"]["regionName"].as_str(),
            Some(row.reported_region),
            "case {case}: expected the region of the request, got {:?}",
            static_ip["location"]
        );
        assert!(
            static_ip["location"]["availabilityZone"].is_null(),
            "case {case}: a static IP is reserved by the region, so it must report no \
             availability zone, got {:?}",
            static_ip["location"]
        );

        // The reserved address: in the modelled `IpAddress` form, and held by
        // this static IP alone.
        let address = static_ip["ipAddress"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_ipv4(&address),
            "case {case}: expected a modelled IpAddress, got {address:?}"
        );
        assert!(
            addresses.insert(address.clone()),
            "case {case}: expected a fresh address, got the already-used {address:?}"
        );

        // Nothing has attached it yet.
        assert_eq!(
            static_ip["isAttached"].as_bool(),
            Some(false),
            "case {case}: a newly allocated static IP is attached to nothing, got \
             {static_ip:?}"
        );
        assert!(
            static_ip["attachedTo"].is_null(),
            "case {case}: a newly allocated static IP is attached to nothing, got {:?}",
            static_ip["attachedTo"]
        );

        // The ARN carries the Region of the request and the canonical emulator
        // account, and ends in a uuid no other static IP is using; the support
        // code names the same id.
        let arn = static_ip["arn"].as_str().unwrap_or_default().to_string();
        let prefix = format!(
            "arn:aws:lightsail:{}:{ACCOUNT}:StaticIp/",
            row.reported_region
        );
        let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case}: expected an ARN starting '{prefix}', got {arn:?}")
        });
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped ARN id, got {id:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: expected a fresh ARN, got the already-used {arn:?}"
        );
        assert_eq!(
            static_ip["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: expected the account/resource-id support code, got {:?}",
            static_ip["supportCode"]
        );
        assert!(
            static_ip["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a creation timestamp, got {:?}",
            static_ip["createdAt"]
        );

        // Every member the model declares for an unattached static IP is
        // reported, and nothing beyond them.
        let reported: BTreeSet<&str> = static_ip
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: expected a static IP object, got {static_ip:?}"))
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(member, _)| member.as_str())
            .collect();
        let want: BTreeSet<&str> = STATIC_IP_MEMBERS.iter().copied().collect();
        assert_eq!(
            reported, want,
            "case {case}: expected exactly the modelled members, got {static_ip:?}"
        );
    }
}

/// RULE: a read reports the record and never changes it, and each allocation is
/// its own record — so every static IP still reads back with the same address it
/// was reserved, however many others were allocated after it and however many
/// times it is read.
#[tokio::test]
async fn an_address_is_static_across_reads_and_other_allocations() {
    let emulator = Emulator::new();
    let mut allocated: Vec<(&Row, Value)> = Vec::new();
    for row in ROWS {
        allocate(&emulator, row.region, row.name).await;
        allocated.push((row, read(&emulator, row.region, row.name).await));
    }

    for (row, first) in &allocated {
        let case = row.case;
        // Read again — in a region that did not allocate it, because a static IP
        // is addressed by name — and twice more, to pin that reading settles
        // nothing.
        let again = read(&emulator, None, row.name).await;
        let third = read(&emulator, row.region, row.name).await;
        assert_eq!(
            &again, first,
            "case {case}: expected the same record on a later read, got {again:?}"
        );
        assert_eq!(
            third, again,
            "case {case}: expected the same record on a third read, got {third:?}"
        );
    }
}

/// RULE: a static IP name addresses ONE static IP, so allocating a name the
/// account already holds is refused with the modelled `InvalidInputException`
/// (400) and changes nothing — in particular, it does not re-draw the address the
/// first allocation reserved.
#[tokio::test]
async fn an_existing_name_cannot_be_allocated_again() {
    let emulator = Emulator::new();
    for row in ROWS {
        let case = row.case;
        allocate(&emulator, row.region, row.name).await;
        let before = read(&emulator, row.region, row.name).await;

        let (status, refused) = emulator
            .call_in(row.region, "AllocateStaticIp", json!({"staticIpName": row.name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 for a name already allocated, got {status} \
             {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidInputException",
            "case {case}: expected InvalidInputException, got {refused:?}"
        );

        let after = read(&emulator, row.region, row.name).await;
        assert_eq!(
            after, before,
            "case {case}: the refused allocation must change nothing, got {after:?}"
        );
    }
}

/// RULE: a well-formed name that names no static IP of this account is refused
/// with Lightsail's `DoesNotExist` envelope — HTTP 400, `NotFoundException` on
/// the wire, `code` `DoesNotExist` — never a zeroed 200.
#[tokio::test]
async fn an_unallocated_name_does_not_exist() {
    let emulator = Emulator::new();
    // One allocated static IP, so the refusals below are about the name asked
    // for and not about an empty store.
    allocate(&emulator, None, "allocated-static-ip").await;

    let cases: &[(&str, &str)] = &[
        ("a name nothing ever allocated", "no-such-static-ip"),
        ("the shortest well-formed name", "zz"),
        ("a name that differs only in case", "Allocated-Static-Ip"),
        ("a long well-formed name", LONG_NAME),
    ];
    for (case, name) in cases {
        let (status, body) = emulator
            .call("GetStaticIp", json!({"staticIpName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the DoesNotExist status, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected NotFoundException on the wire, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist code, got {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("StaticIp") && message.contains(name)),
            "case {case}: expected a message naming the resource type and the name, got \
             {body:?}"
        );
    }
}

/// RULE: the store's own bound on how many static IPs it holds is enforced
/// before the first write, so the allocation past it is refused with the modelled
/// `InvalidInputException` and the ones before it are untouched.
///
/// The bound is the emulator's resource guard
/// (`crate::state::MAX_STATIC_IPS`), not an AWS quota, so the test reads it from
/// the crate rather than hard-coding a number.
#[tokio::test]
async fn the_store_bound_refuses_one_allocation_too_many() {
    let emulator = Emulator::new();
    for index in 0..MAX_STATIC_IPS {
        allocate(&emulator, None, &format!("static-ip-{index}")).await;
    }

    let (status, refused) = emulator
        .call(
            "AllocateStaticIp",
            json!({"staticIpName": "static-ip-past-the-bound"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case one past the store bound: expected a 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidInputException",
        "case one past the store bound: expected InvalidInputException, got {refused:?}"
    );
    let (status, body) = emulator
        .call(
            "GetStaticIp",
            json!({"staticIpName": "static-ip-past-the-bound"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case one past the store bound: the refused allocation must store nothing, got \
         {status} {body:?}"
    );
    let first = read(&emulator, None, "static-ip-0").await;
    assert_eq!(
        first["name"].as_str(),
        Some("static-ip-0"),
        "case one past the store bound: the static IPs already held must be untouched, \
         got {first:?}"
    );
}
