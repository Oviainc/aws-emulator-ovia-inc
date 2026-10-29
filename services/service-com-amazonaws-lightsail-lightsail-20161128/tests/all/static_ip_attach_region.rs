// SPDX-License-Identifier: CC0-1.0

//! `AttachStaticIp` across Regions — which pairings of a reservation and an
//! instance an attach can be made from at all — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The `Authorization` header carries only a credential SCOPE, which is
//! what names the Region a request is made in.
//!
//! The rule under test: a Lightsail static IP is reserved in ONE Region — the
//! modelled `StaticIp$location` is "the region and Availability Zone where the
//! static IP was created" — and an instance runs in one Availability Zone of one
//! Region, so an attach is made only between a reservation and an instance whose
//! records report the SAME `location.regionName`, and a pairing across two
//! Regions is refused with the modelled `OperationFailureException` while every
//! record stays exactly as it was. The Region is the whole of the rule: two
//! Availability Zones of one Region pair, so this is not the zone-level rule
//! `AttachDisk` applies.
//!
//! Which Region the attach CALL is made in decides nothing either way: both
//! halves are addressed by name and each record carries its own location, so the
//! rows attach from the reservation's Region, from the instance's Region, from a
//! third Region and from no credential scope at all.
//!
//! The HTTP harness, the address shape check and the settle poll are the ones this
//! test target already publishes; the attach suite's own success and refusal rules
//! are pinned in `static_ip_attach.rs`, which this suite carries no second copy
//! of.

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_modelled_ipv4, wait_for_state, Emulator};

/// The blueprint and bundle every instance in this suite is created from: a
/// published Linux plan (`src/state.rs`, `BUNDLE_SPECS`). Which plan it is changes
/// nothing about where an instance runs, so one is enough here.
const BLUEPRINT_ID: &str = "amazon_linux_2023";
/// See [`BLUEPRINT_ID`].
const BUNDLE_ID: &str = "nano_3_0";

/// The wire error Lightsail refuses a well-formed request its resources' state
/// does not allow with.
const OPERATION_FAILURE: &str = "OperationFailureException";

/// The Region a request that carries no credential scope is served in
/// (`crate::state::DEFAULT_REGION`), which is the Region such a request's
/// reservation is recorded in.
const DEFAULT_REGION: &str = "us-east-1";

/// One pairing of a reservation and an instance.
struct Pair {
    case: &'static str,
    /// The Region the reservation is allocated in, or `None` for an allocate that
    /// carries no credential scope at all.
    reservation_region: Option<&'static str>,
    /// The Region the reservation's own record reports — [`DEFAULT_REGION`] when
    /// the allocate named none.
    reported_reservation_region: &'static str,
    /// The zone the instance is created in, which is what decides its Region. Each
    /// one is a zone the Region catalog (`src/state.rs`, `REGION_SPECS`)
    /// publishes for its Region.
    availability_zone: &'static str,
    /// The Region the instance's own record reports.
    instance_region: &'static str,
    /// The Region the attach call itself is made in, which decides nothing.
    attach_region: Option<&'static str>,
}

/// The pairings an attach can be made from: the two records report the same
/// Region. The rows differ in whether the two sit in the same Availability Zone or
/// in two zones of one Region, in which Region that is, in whether either call
/// named it at all, and in the Region the attach is made in.
const SAME_REGION: &[Pair] = &[
    Pair {
        case: "one zone, named by neither call",
        reservation_region: None,
        reported_reservation_region: DEFAULT_REGION,
        availability_zone: "us-east-1a",
        instance_region: DEFAULT_REGION,
        attach_region: None,
    },
    Pair {
        case: "two zones of the default Region, attached from another Region",
        reservation_region: None,
        reported_reservation_region: DEFAULT_REGION,
        availability_zone: "us-east-1f",
        instance_region: DEFAULT_REGION,
        attach_region: Some("ap-south-1"),
    },
    Pair {
        case: "a named Region the instance is in a second zone of",
        reservation_region: Some("us-west-2"),
        reported_reservation_region: "us-west-2",
        availability_zone: "us-west-2d",
        instance_region: "us-west-2",
        attach_region: Some("us-west-2"),
    },
    Pair {
        case: "a Region on another continent, attached without a scope",
        reservation_region: Some("eu-north-1"),
        reported_reservation_region: "eu-north-1",
        availability_zone: "eu-north-1c",
        instance_region: "eu-north-1",
        attach_region: None,
    },
    Pair {
        case: "a two-zone Region, attached from a third Region",
        reservation_region: Some("ca-central-1"),
        reported_reservation_region: "ca-central-1",
        availability_zone: "ca-central-1b",
        instance_region: "ca-central-1",
        attach_region: Some("sa-east-1"),
    },
];

/// The pairings an attach cannot be made from: the two records report different
/// Regions. The rows differ in which half named its Region (including the pairing
/// a request with no credential scope makes), how far apart the two Regions are —
/// two Regions of one continent, two continents, and two Regions sharing a prefix
/// (`us-east-1` against `us-east-2`) — and the Region the attach is made in, which
/// in turn is the reservation's, the instance's, a third one and none.
const CROSS_REGION: &[Pair] = &[
    Pair {
        case: "a reservation in Europe against an instance in Ohio",
        reservation_region: Some("eu-west-1"),
        reported_reservation_region: "eu-west-1",
        availability_zone: "us-east-2a",
        instance_region: "us-east-2",
        attach_region: Some("eu-west-1"),
    },
    Pair {
        case: "a reservation the allocate named no Region for, against Oregon",
        reservation_region: None,
        reported_reservation_region: DEFAULT_REGION,
        availability_zone: "us-west-2b",
        instance_region: "us-west-2",
        attach_region: None,
    },
    Pair {
        case: "two Regions sharing a prefix, attached from the instance's",
        reservation_region: Some("us-east-2"),
        reported_reservation_region: "us-east-2",
        availability_zone: "us-east-1b",
        instance_region: DEFAULT_REGION,
        attach_region: Some(DEFAULT_REGION),
    },
    Pair {
        case: "two Regions of one continent, attached from a third",
        reservation_region: Some("eu-central-1"),
        reported_reservation_region: "eu-central-1",
        availability_zone: "eu-west-3a",
        instance_region: "eu-west-3",
        attach_region: Some("ap-east-1"),
    },
    Pair {
        case: "the Pacific against South America, attached from the reservation's",
        reservation_region: Some("ap-southeast-2"),
        reported_reservation_region: "ap-southeast-2",
        availability_zone: "sa-east-1c",
        instance_region: "sa-east-1",
        attach_region: Some("ap-southeast-2"),
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

/// Create the instance `name` in `zone` and wait for it to serve.
async fn create_instance(emulator: &Emulator, name: &str, zone: &str) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": zone,
                "blueprintId": BLUEPRINT_ID,
                "bundleId": BUNDLE_ID,
            }),
        )
        .await;
    wait_for_state(emulator, name, "running").await;
}

/// The stored static IP `name`, as `GetStaticIp` reports it.
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

/// Stand the pairing's two resources up, assert each reports the Region the row
/// says it is in, and answer the names they are held under.
async fn fixture(emulator: &Emulator, pair: &Pair) -> (String, String) {
    let case = pair.case;
    let reservation = "reservation";
    let host = "attach-host";
    allocate(emulator, pair.reservation_region, reservation).await;
    create_instance(emulator, host, pair.availability_zone).await;

    let reserved = static_ip(emulator, reservation).await;
    assert_eq!(
        reserved["location"]["regionName"].as_str(),
        Some(pair.reported_reservation_region),
        "case {case}: expected the reservation in {}, got {reserved:?}",
        pair.reported_reservation_region
    );
    let created = instance(emulator, host).await;
    assert_eq!(
        (
            created["location"]["regionName"].as_str(),
            created["location"]["availabilityZone"].as_str(),
        ),
        (Some(pair.instance_region), Some(pair.availability_zone)),
        "case {case}: expected the instance in {}, got {created:?}",
        pair.instance_region
    );
    (reservation.to_string(), host.to_string())
}

// ---------------------------------------------------------------------------
// What a cross-Region pairing does
// ---------------------------------------------------------------------------

/// RULE: a static IP is reserved in one Region and an instance runs in one, so an
/// attach naming a reservation and an instance whose records report different
/// Regions is refused with the modelled operation failure, and neither record
/// moves — the instance keeps the ordinary address it was reachable at and reports
/// no static one, and the reservation stays attached to nothing.
#[tokio::test]
async fn an_attach_across_regions_is_refused_and_attaches_nothing() {
    for pair in CROSS_REGION {
        let case = pair.case;
        let emulator = Emulator::new();
        let (reservation, host) = fixture(&emulator, pair).await;

        let before_reservation = static_ip(&emulator, &reservation).await;
        let before_instance = instance(&emulator, &host).await;

        let (status, body) = emulator
            .call_in(
                pair.attach_region,
                "AttachStaticIp",
                json!({"staticIpName": reservation, "instanceName": host}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str(), body["code"].as_str()),
            (400, OPERATION_FAILURE, None),
            "case {case}: expected the {OPERATION_FAILURE} refusal, got {status} {body:?}"
        );
        // The message names both records and the Region each of them is in, which
        // is what makes the refusal actionable.
        let message = body["message"].as_str().unwrap_or_default();
        for named in [
            reservation.as_str(),
            host.as_str(),
            pair.reported_reservation_region,
            pair.instance_region,
        ] {
            assert!(
                message.contains(named),
                "case {case}: expected the message to name {named}, got {message:?}"
            );
        }

        // Nothing was attached: both records read back exactly as they were.
        assert_eq!(
            static_ip(&emulator, &reservation).await,
            before_reservation,
            "case {case}: expected the reservation untouched, was {before_reservation:?}"
        );
        assert_eq!(
            instance(&emulator, &host).await,
            before_instance,
            "case {case}: expected the instance untouched, was {before_instance:?}"
        );
        let after_instance = instance(&emulator, &host).await;
        let public = after_instance["publicIpAddress"].as_str().unwrap_or_default();
        assert!(
            after_instance["isStaticIp"].as_bool() == Some(false)
                && is_modelled_ipv4(public)
                && Some(public) != before_reservation["ipAddress"].as_str(),
            "case {case}: expected the instance on its own ordinary address, got \
             {after_instance:?}"
        );
    }
}

/// RULE (the same one, on the pairings it admits): the Region is the whole of the
/// rule, so a reservation and an instance whose records report the same Region
/// attach — including when the instance is in another Availability Zone of that
/// Region — whatever Region the attach call itself is made in, and the instance
/// serves on the reserved address.
#[tokio::test]
async fn an_attach_within_one_region_is_made_whatever_zone_and_caller_region() {
    for pair in SAME_REGION {
        let case = pair.case;
        let emulator = Emulator::new();
        let (reservation, host) = fixture(&emulator, pair).await;

        let reserved_address = static_ip(&emulator, &reservation).await["ipAddress"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let ordinary_address = instance(&emulator, &host).await["publicIpAddress"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, body) = emulator
            .call_in(
                pair.attach_region,
                "AttachStaticIp",
                json!({"staticIpName": reservation, "instanceName": host}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the attach to be made, got {status} {body:?}"
        );

        let after_reservation = static_ip(&emulator, &reservation).await;
        assert_eq!(
            (
                after_reservation["isAttached"].as_bool(),
                after_reservation["attachedTo"].as_str(),
                after_reservation["ipAddress"].as_str(),
                after_reservation["location"]["regionName"].as_str(),
            ),
            (
                Some(true),
                Some(host.as_str()),
                Some(reserved_address.as_str()),
                Some(pair.reported_reservation_region),
            ),
            "case {case}: expected the reservation attached to {host}, got \
             {after_reservation:?}"
        );
        let after_instance = instance(&emulator, &host).await;
        assert_eq!(
            (
                after_instance["isStaticIp"].as_bool(),
                after_instance["publicIpAddress"].as_str(),
                after_instance["location"]["regionName"].as_str(),
            ),
            (
                Some(true),
                Some(reserved_address.as_str()),
                Some(pair.instance_region),
            ),
            "case {case}: expected the instance on the reserved address, got {after_instance:?}"
        );
        assert_ne!(
            reserved_address, ordinary_address,
            "case {case}: the instance gave up the ordinary address it was reachable at, got \
             {reserved_address:?}"
        );
    }
}
