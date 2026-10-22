// SPDX-License-Identifier: CC0-1.0

//! Lightsail's rejection envelope for a request that names something that does
//! not exist, driven over the wire through the same front door a client uses.
//!
//! The rule under test: when Lightsail cannot find what a request names — a
//! resource of the account, or an entry of a published catalog — it answers
//! **HTTP 400** with the wire error `NotFoundException`, the body member `code`
//! set to `DoesNotExist`, and a `message` built from the request: the resource's
//! `ResourceType` and the identifier the request carried (`The <ResourceType>
//! does not exist: <name>`) for an account resource, and the rejected id for a
//! blueprint the catalog does not hold. The model declares `@httpError(404)` on
//! `NotFoundException`, but a Lightsail SDK client maps the failure from the
//! body's `code`, and the recorded endpoint answers 400 — so 400 is the wire
//! status, and it is the same envelope on every path, reporting or mutating.
//!
//! So every case here is a TABLE: one row per operation that resolves an
//! identifier, crossed with names that differ in length, case, digits and
//! separators; one row per blueprint the catalog holds (every active entry must
//! create, every inactive one must be refused); and one row per form an
//! unresolvable blueprint id can take. Refusals are also asserted to store
//! nothing, and reads to store nothing either.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    blueprint_spec, BLUEPRINTS, BLUEPRINT_TYPE_APP, BLUEPRINT_TYPE_OS,
};

use crate::instance_lifecycle::{error_code, Emulator};

/// The body member `code` Lightsail sets on a rejection whose cause is that what
/// the request names does not exist.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The wire error code (awsJson `__type`) of that rejection: the modelled shape's
/// own name.
const NOT_FOUND: &str = "NotFoundException";

/// Instance operations that resolve `instanceName` against the store — every
/// implemented one, reporting and mutating alike.
const INSTANCE_OPERATIONS: &[&str] = &[
    "GetInstance",
    "GetInstanceState",
    "StartInstance",
    "StopInstance",
    "DeleteInstance",
];

/// Names nothing created, differing along what the rule generalises over: the
/// length, the case, the digits and the separators. Every one satisfies the
/// modelled `ResourceName` pattern `^\w[\w\-]*\w$`, so the centralized walk
/// passes it through to the handler. The issue's own recorded name is one row,
/// not the case.
const MISSING_NAMES: &[&str] = &[
    "ab",
    "never-created",
    "Probe_7",
    "sim2real-probe-lightsail-instance-missing-f0ca242d",
    "a-name-with-many-hyphen-separated-words-and-digits-0123456789",
];

/// Blueprint ids this service publishes, written out independently of the
/// service's own `BLUEPRINTS` table so these rows pin the catalog rather than
/// restating whatever the code happens to hold.
///
/// SOURCE: the Lightsail instance-image catalog (what `GetBlueprints` answers
/// from) — an operating-system image per supported release, and an application
/// stack per packaged app. None of these is the id the issue cited.
const PUBLISHED_BLUEPRINT_IDS: &[(&str, &str)] = &[
    ("amazon_linux_2", BLUEPRINT_TYPE_OS),
    ("almalinux_9", BLUEPRINT_TYPE_OS),
    ("freebsd_14", BLUEPRINT_TYPE_OS),
    ("ubuntu_24_04", BLUEPRINT_TYPE_OS),
    ("windows_server_2016", BLUEPRINT_TYPE_OS),
    ("nodejs", BLUEPRINT_TYPE_APP),
    ("wordpress_multisite", BLUEPRINT_TYPE_APP),
];

/// The `CreateInstances` body for one instance on `blueprint_id`.
fn create_body(name: &str, blueprint_id: &str) -> Value {
    json!({
        "instanceNames": [name],
        "availabilityZone": "us-east-1a",
        "blueprintId": blueprint_id,
        "bundleId": "nano_3_0",
    })
}

/// Assert `body`/`status` is Lightsail's `DoesNotExist` envelope carrying
/// `message`.
fn assert_does_not_exist(case: &str, status: u16, body: &Value, message: &str) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        NOT_FOUND,
        "case {case}: expected the wire error {NOT_FOUND}, got {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some(DOES_NOT_EXIST),
        "case {case}: expected the body code {DOES_NOT_EXIST}, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Account resources
// ---------------------------------------------------------------------------

/// RULE: every operation that resolves an identifier against the store answers
/// the same `DoesNotExist` envelope when nothing of that name is stored, with a
/// message naming the resource's `ResourceType` and the requested identifier.
#[tokio::test]
async fn every_unresolvable_identifier_answers_the_does_not_exist_envelope() {
    let emulator = Emulator::new();

    // One row per (operation, identifier member, resource type), crossed with
    // the names above.
    let paths: &[(&str, &str, &str)] = &[
        ("GetInstance", "instanceName", "Instance"),
        ("GetInstanceState", "instanceName", "Instance"),
        ("StartInstance", "instanceName", "Instance"),
        ("StopInstance", "instanceName", "Instance"),
        ("DeleteInstance", "instanceName", "Instance"),
    ];

    for (operation, member, resource_type) in paths {
        for name in MISSING_NAMES {
            let case = format!("{operation} on {name:?}");
            let (status, body) = emulator.call(operation, json!({*member: name})).await;
            assert_does_not_exist(
                &case,
                status,
                &body,
                &format!("The {resource_type} does not exist: {name}"),
            );
        }
    }

    // `GetContainerServices` resolves a `ContainerServiceName`, whose modelled
    // pattern admits only lower-case names, and answers the same envelope for its
    // own resource type.
    for name in ["ab", "never-created", "a-service-with-digits-0123456789"] {
        let case = format!("GetContainerServices on {name:?}");
        let (status, body) = emulator
            .call("GetContainerServices", json!({"serviceName": name}))
            .await;
        assert_does_not_exist(
            &case,
            status,
            &body,
            &format!("The ContainerService does not exist: {name}"),
        );
    }

    // `GetDomain` resolves a DNS name rather than a `ResourceName`, and answers
    // the same envelope for its own resource type.
    for name in ["absent.example", "missing.test", "deep.sub.absent.example"] {
        let case = format!("GetDomain on {name:?}");
        let (status, body) = emulator.call("GetDomain", json!({"domainName": name})).await;
        assert_does_not_exist(
            &case,
            status,
            &body,
            &format!("The Domain does not exist: {name}"),
        );
    }

    // Reporting an absent resource stores nothing: the store is still empty.
    let listed = emulator.ok("GetInstances", json!({})).await;
    assert_eq!(
        listed["instances"].as_array().map(Vec::len),
        Some(0),
        "case after every unresolvable read: the store must still be empty, got {listed:?}"
    );
}

/// RULE: a resource that existed and was deleted is refused exactly like one that
/// never existed — the envelope is a function of what the store holds NOW.
#[tokio::test]
async fn a_deleted_resource_is_refused_like_one_that_never_existed() {
    let emulator = Emulator::new();
    let name = "short-lived";
    emulator
        .ok("CreateInstances", create_body(name, "amazon_linux_2023"))
        .await;
    emulator
        .ok("DeleteInstance", json!({"instanceName": name}))
        .await;

    for operation in INSTANCE_OPERATIONS {
        let case = format!("{operation} after the delete");
        let (status, body) = emulator.call(operation, json!({"instanceName": name})).await;
        assert_does_not_exist(
            &case,
            status,
            &body,
            &format!("The Instance does not exist: {name}"),
        );
    }
}

// ---------------------------------------------------------------------------
// The blueprint catalog
// ---------------------------------------------------------------------------

/// RULE: `blueprintId` is resolved against the published blueprint catalog, so an
/// id the catalog does not hold — or holds only as an inactive (retired) image —
/// is refused with the `DoesNotExist` envelope naming the rejected blueprint, and
/// creates nothing.
#[tokio::test]
async fn a_blueprint_outside_the_catalog_is_refused_and_creates_nothing() {
    // Rows: the issue's own id, a plausible id of a published family at a release
    // that does not exist, the wrong case, the wrong separator, a bare family
    // name, and the ids the catalog holds as inactive.
    let mut rejected: Vec<&str> = vec![
        "nonexistent-blueprint-999",
        "amazon_linux_9999",
        "WORDPRESS",
        "wordpress-multisite",
        "windows_server",
    ];
    rejected.extend(
        BLUEPRINTS
            .iter()
            .filter(|spec| !spec.is_active)
            .map(|spec| spec.blueprint_id),
    );
    assert!(
        rejected.len() > 5,
        "case the inactive rows: the catalog must hold at least one inactive blueprint, \
         got {rejected:?}"
    );

    for blueprint_id in rejected {
        let emulator = Emulator::new();
        let case = format!("CreateInstances on blueprintId {blueprint_id:?}");
        let (status, body) = emulator
            .call("CreateInstances", create_body("refused", blueprint_id))
            .await;
        assert_does_not_exist(
            &case,
            status,
            &body,
            &format!("The specified blueprint does not exist or is deprecated: {blueprint_id}"),
        );

        // A refused create stores nothing, so the name it named still does not
        // exist.
        let (read_status, read_body) = emulator
            .call("GetInstance", json!({"instanceName": "refused"}))
            .await;
        assert_does_not_exist(
            &format!("{case}, reading back"),
            read_status,
            &read_body,
            "The Instance does not exist: refused",
        );
        let listed = emulator.ok("GetInstances", json!({})).await;
        assert_eq!(
            listed["instances"].as_array().map(Vec::len),
            Some(0),
            "case {case}: a refused create must store nothing, got {listed:?}"
        );
    }
}

/// RULE: every ACTIVE entry of the catalog creates an instance, and that instance
/// reports the blueprint the request named — the catalog is what the validation
/// resolves against, not a hand-picked few ids.
#[tokio::test]
async fn every_active_catalog_blueprint_creates_an_instance_that_reports_it() {
    let emulator = Emulator::new();
    let active: Vec<&str> = BLUEPRINTS
        .iter()
        .filter(|spec| spec.is_active)
        .map(|spec| spec.blueprint_id)
        .collect();
    assert!(
        active.len() > 10,
        "case the catalog: expected the published blueprints, got {active:?}"
    );

    for blueprint_id in active {
        let name = format!("bp-{blueprint_id}");
        let case = format!("creating on blueprintId {blueprint_id:?}");
        let (status, body) = emulator
            .call("CreateInstances", create_body(&name, blueprint_id))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: an active blueprint must create, got {status} {body:?}"
        );

        let read = emulator.ok("GetInstance", json!({"instanceName": name})).await;
        assert_eq!(
            read["instance"]["blueprintId"].as_str(),
            Some(blueprint_id),
            "case {case}: the instance must report the requested blueprint, got {read:?}"
        );
    }
}

/// RULE: the catalog holds each published id once, with the type the image is —
/// `os` for an operating-system image, `app` for an application stack.
#[tokio::test]
async fn the_catalog_holds_every_published_blueprint_once() {
    for (blueprint_id, blueprint_type) in PUBLISHED_BLUEPRINT_IDS {
        let spec = blueprint_spec(blueprint_id).unwrap_or_else(|| {
            panic!("case {blueprint_id:?}: the published catalog must hold this blueprint")
        });
        assert_eq!(
            spec.blueprint_type, *blueprint_type,
            "case {blueprint_id:?}: expected type {blueprint_type}, got {:?}",
            spec.blueprint_type
        );
        assert!(
            spec.is_active,
            "case {blueprint_id:?}: a published release must be active, got {spec:?}"
        );
        assert_eq!(
            BLUEPRINTS
                .iter()
                .filter(|entry| entry.blueprint_id == *blueprint_id)
                .count(),
            1,
            "case {blueprint_id:?}: the catalog must hold it exactly once"
        );
    }

    for spec in BLUEPRINTS {
        assert!(
            spec.blueprint_type == BLUEPRINT_TYPE_OS || spec.blueprint_type == BLUEPRINT_TYPE_APP,
            "case {:?}: the type must be a modelled BlueprintType value, got {spec:?}",
            spec.blueprint_id
        );
    }
}
