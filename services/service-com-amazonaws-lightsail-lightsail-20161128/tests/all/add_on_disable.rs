// SPDX-License-Identifier: CC0-1.0

//! Disabling an add-on of a Lightsail resource, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `DisableAddOn` removes the add-on of the requested type
//! from the set of add-ons the named resource has ENABLED — the set an `addOns`
//! member reports — so a later read of that resource reports every other add-on
//! it still has, each with the parameters it was enabled with, and no longer
//! this one. It holds for every kind of resource that carries add-ons (a virtual
//! private server and a block storage disk alike), for every member of the
//! modelled `AddOnType` enum, and a type the resource has not enabled is
//! already disabled, so the call stands and removes nothing. The change is
//! logged in the account's operation log, naming the resource it acted on; a
//! name the account holds no add-on-carrying resource of is refused with the
//! service's `DoesNotExist` envelope.

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target already
// exposes: a sibling suite drives the service through the same front door rather than
// standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The zone every resource in this suite is created in; an add-on is not
/// regional, so one zone serves every case.
const ZONE: &str = "us-east-1a";

/// A Linux/Unix blueprint and the smallest plan: neither selects anything about
/// an add-on.
const BLUEPRINT: &str = "amazon_linux_2023";
/// See [`BLUEPRINT`].
const BUNDLE: &str = "nano_3_0";

/// The `ResourceType` a virtual private server reports, from the modelled
/// `ResourceType` enum.
const RESOURCE_TYPE_INSTANCE: &str = "Instance";
/// The `ResourceType` a block storage disk reports, from the modelled
/// `ResourceType` enum.
const RESOURCE_TYPE_DISK: &str = "Disk";

/// Which kind of add-on-carrying resource a row is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A virtual private server, which carries either add-on type.
    Instance,
    /// A block storage disk, which carries the automatic snapshot add-on.
    Disk,
}

impl Kind {
    /// The `ResourceType` this kind reports.
    fn resource_type(self) -> &'static str {
        match self {
            Self::Instance => RESOURCE_TYPE_INSTANCE,
            Self::Disk => RESOURCE_TYPE_DISK,
        }
    }
}

/// One case: a resource created with `enabled` add-ons, one `disable`d type, and
/// the add-on names the resource must report afterwards.
#[derive(Debug)]
struct Row {
    case: &'static str,
    kind: Kind,
    name: &'static str,
    /// The add-on types the create enables, in request order.
    enabled: &'static [&'static str],
    /// The `AddOnType` the `DisableAddOn` call names.
    disable: &'static str,
    /// The add-on names a read must report after the call, sorted.
    remaining: &'static [&'static str],
}

/// The two members of the modelled `AddOnType` enum
/// (`smithy_query(query="shape",shape="AddOnType")`): the automatic snapshot
/// add-on, which both kinds of resource carry, and the idle-stop add-on, which a
/// virtual computer carries.
const AUTO_SNAPSHOT: &str = "AutoSnapshot";
/// See [`AUTO_SNAPSHOT`].
const STOP_INSTANCE_ON_IDLE: &str = "StopInstanceOnIdle";

/// The class the rule spans: each kind of add-on-carrying resource, each member
/// of the modelled `AddOnType` enum, a resource carrying one add-on and one
/// carrying several, and a type the resource never had.
const ROWS: &[Row] = &[
    Row {
        case: "an instance's automatic snapshot add-on, with another add-on beside it",
        kind: Kind::Instance,
        name: "instance-both-auto",
        enabled: &[AUTO_SNAPSHOT, STOP_INSTANCE_ON_IDLE],
        disable: AUTO_SNAPSHOT,
        remaining: &[STOP_INSTANCE_ON_IDLE],
    },
    Row {
        case: "an instance's idle-stop add-on, with another add-on beside it",
        kind: Kind::Instance,
        name: "instance-both-idle",
        enabled: &[AUTO_SNAPSHOT, STOP_INSTANCE_ON_IDLE],
        disable: STOP_INSTANCE_ON_IDLE,
        remaining: &[AUTO_SNAPSHOT],
    },
    Row {
        case: "the only add-on an instance has",
        kind: Kind::Instance,
        name: "instance-only-auto",
        enabled: &[AUTO_SNAPSHOT],
        disable: AUTO_SNAPSHOT,
        remaining: &[],
    },
    Row {
        case: "a type an instance has not enabled, on an instance with none",
        kind: Kind::Instance,
        name: "instance-no-add-ons",
        enabled: &[],
        disable: AUTO_SNAPSHOT,
        remaining: &[],
    },
    Row {
        case: "a type an instance has not enabled, beside one it has",
        kind: Kind::Instance,
        name: "instance-other-type",
        enabled: &[AUTO_SNAPSHOT],
        disable: STOP_INSTANCE_ON_IDLE,
        remaining: &[AUTO_SNAPSHOT],
    },
    Row {
        case: "a disk's automatic snapshot add-on",
        kind: Kind::Disk,
        name: "disk-auto",
        enabled: &[AUTO_SNAPSHOT],
        disable: AUTO_SNAPSHOT,
        remaining: &[],
    },
    Row {
        case: "a type a disk cannot have, on a disk that has the other",
        kind: Kind::Disk,
        name: "disk-other-type",
        enabled: &[AUTO_SNAPSHOT],
        disable: STOP_INSTANCE_ON_IDLE,
        remaining: &[AUTO_SNAPSHOT],
    },
    Row {
        case: "a type a disk has not enabled, on a disk with none",
        kind: Kind::Disk,
        name: "disk-no-add-ons",
        enabled: &[],
        disable: AUTO_SNAPSHOT,
        remaining: &[],
    },
];

/// The hour the automatic snapshot add-on is enabled with, so a read can prove
/// the surviving add-on kept the parameters it was enabled with.
const SNAPSHOT_HOUR: &str = "06:00";
/// The idle threshold and duration the idle-stop add-on is enabled with, for the
/// same reason.
const IDLE_THRESHOLD: &str = "5";
/// See [`IDLE_THRESHOLD`].
const IDLE_DURATION: &str = "20";

/// The `addOns` request list that enables `types`.
fn add_on_requests(types: &[&str]) -> Value {
    Value::Array(
        types
            .iter()
            .map(|add_on_type| match *add_on_type {
                STOP_INSTANCE_ON_IDLE => json!({
                    "addOnType": STOP_INSTANCE_ON_IDLE,
                    "stopInstanceOnIdleRequest": {
                        "threshold": IDLE_THRESHOLD,
                        "duration": IDLE_DURATION,
                    },
                }),
                other => json!({
                    "addOnType": other,
                    "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": SNAPSHOT_HOUR},
                }),
            })
            .collect(),
    )
}

/// Create the resource `row` is about, with the add-ons it names enabled.
async fn create(emulator: &Emulator, row: &Row) {
    let mut body = match row.kind {
        Kind::Instance => json!({
            "instanceNames": [row.name],
            "availabilityZone": ZONE,
            "blueprintId": BLUEPRINT,
            "bundleId": BUNDLE,
        }),
        Kind::Disk => json!({
            "diskName": row.name,
            "availabilityZone": ZONE,
            "sizeInGb": 32,
        }),
    };
    if !row.enabled.is_empty() {
        body["addOns"] = add_on_requests(row.enabled);
    }
    let operation = match row.kind {
        Kind::Instance => "CreateInstances",
        Kind::Disk => "CreateDisk",
    };
    emulator.ok(operation, body).await;
}

/// The add-ons the resource `row` is about reports now, read back through the
/// resource's own reader.
async fn read_add_ons(emulator: &Emulator, row: &Row) -> Vec<Value> {
    let body = match row.kind {
        Kind::Instance => emulator
            .ok("GetInstance", json!({"instanceName": row.name}))
            .await["instance"]
            .clone(),
        Kind::Disk => emulator.ok("GetDisk", json!({"diskName": row.name})).await["disk"].clone(),
    };
    body["addOns"]
        .as_array()
        .unwrap_or_else(|| panic!("case {}: expected an addOns array, got {body:?}", row.case))
        .clone()
}

/// The `name` of each add-on in `add_ons`, sorted, so a comparison is about
/// which add-ons are enabled and not about their order.
fn names(add_ons: &[Value]) -> Vec<String> {
    let mut names: Vec<String> = add_ons
        .iter()
        .map(|add_on| add_on["name"].as_str().unwrap_or_default().to_string())
        .collect();
    names.sort();
    names
}

/// RULE: `DisableAddOn` removes the add-on of the requested type from the
/// add-ons the named resource has enabled, leaves every other add-on exactly as
/// it was, and removes nothing when the resource has not enabled that type.
#[tokio::test]
async fn disabling_an_add_on_removes_it_from_the_resources_enabled_add_ons() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        create(&emulator, row).await;

        let before = read_add_ons(&emulator, row).await;
        assert_eq!(
            names(&before).len(),
            row.enabled.len(),
            "case {case}: the create must enable every requested add-on, got {before:?}"
        );

        let answered = emulator
            .ok(
                "DisableAddOn",
                json!({"resourceName": row.name, "addOnType": row.disable}),
            )
            .await;

        // The call answers exactly one operation-log entry: it acted on the one
        // resource it named.
        let operations = answered["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {answered:?}"))
            .clone();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected one operation, got {operations:?}"
        );
        let operation = &operations[0];
        assert_eq!(
            (
                operation["operationType"].as_str(),
                operation["resourceName"].as_str(),
                operation["resourceType"].as_str(),
            ),
            (
                Some("DisableAddOn"),
                Some(row.name),
                Some(row.kind.resource_type()),
            ),
            "case {case}: the entry must name this call and this resource, got {operation:?}"
        );
        let id = operation["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&id),
            "case {case}: expected a minted operation id, got {id:?}"
        );

        // The add-on is gone when the call returns, so its entry is terminal.
        assert_eq!(
            (
                operation["status"].as_str(),
                operation["isTerminal"].as_bool()
            ),
            (Some("Succeeded"), Some(true)),
            "case {case}: a finished change is terminal, got {operation:?}"
        );

        // The entry is PERSISTED in the account's operation log, not just
        // answered: a later read of it reports the same entry.
        let logged = emulator.ok("GetOperation", json!({"operationId": id})).await;
        assert_eq!(
            logged["operation"]["resourceName"].as_str(),
            Some(row.name),
            "case {case}: the entry must be readable from the log, got {logged:?}"
        );

        let after = read_add_ons(&emulator, row).await;
        assert_eq!(
            names(&after),
            row.remaining
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case}: expected exactly the add-ons still enabled, got {after:?}"
        );

        // Every surviving add-on is UNTOUCHED: same status and same parameters
        // as the create enabled it with.
        for add_on in &after {
            assert_eq!(
                add_on["status"].as_str(),
                Some("Enabled"),
                "case {case}: a surviving add-on stays enabled, got {add_on:?}"
            );
            match add_on["name"].as_str() {
                Some(AUTO_SNAPSHOT) => assert_eq!(
                    add_on["snapshotTimeOfDay"].as_str(),
                    Some(SNAPSHOT_HOUR),
                    "case {case}: a surviving add-on keeps its parameters, got {add_on:?}"
                ),
                Some(STOP_INSTANCE_ON_IDLE) => assert_eq!(
                    (add_on["threshold"].as_str(), add_on["duration"].as_str()),
                    (Some(IDLE_THRESHOLD), Some(IDLE_DURATION)),
                    "case {case}: a surviving add-on keeps its parameters, got {add_on:?}"
                ),
                other => panic!("case {case}: unexpected add-on name {other:?}"),
            }
        }
    }
}

/// RULE: the add-on's absence is the whole effect, so disabling the same type
/// again changes nothing and still succeeds — and disabling one type never
/// reaches another resource's add-ons.
#[tokio::test]
async fn disabling_an_add_on_is_idempotent_and_reaches_only_the_named_resource() {
    let emulator = Emulator::new();
    let target = Row {
        case: "the resource the call names",
        kind: Kind::Instance,
        name: "idempotent-target",
        enabled: &[AUTO_SNAPSHOT, STOP_INSTANCE_ON_IDLE],
        disable: AUTO_SNAPSHOT,
        remaining: &[STOP_INSTANCE_ON_IDLE],
    };
    let bystander = Row {
        case: "a resource of the same name shape the call does not name",
        kind: Kind::Disk,
        name: "idempotent-bystander",
        enabled: &[AUTO_SNAPSHOT],
        disable: AUTO_SNAPSHOT,
        remaining: &[AUTO_SNAPSHOT],
    };
    create(&emulator, &target).await;
    create(&emulator, &bystander).await;

    for attempt in 1..=3 {
        emulator
            .ok(
                "DisableAddOn",
                json!({"resourceName": target.name, "addOnType": target.disable}),
            )
            .await;
        let after = read_add_ons(&emulator, &target).await;
        assert_eq!(
            names(&after),
            vec![STOP_INSTANCE_ON_IDLE.to_string()],
            "case attempt {attempt}: every call leaves the same add-ons enabled, got {after:?}"
        );
    }

    let bystanding = read_add_ons(&emulator, &bystander).await;
    assert_eq!(
        names(&bystanding),
        vec![AUTO_SNAPSHOT.to_string()],
        "case {}: another resource's add-ons are untouched, got {bystanding:?}",
        bystander.case
    );
}

/// RULE: a request this service cannot serve changes nothing — a name the
/// account holds no add-on-carrying resource of is the service's `DoesNotExist`
/// envelope, and an `addOnType` outside the modelled enum is refused by the
/// constraint walk.
#[tokio::test]
async fn an_unservable_request_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let row = Row {
        case: "a stored instance",
        kind: Kind::Instance,
        name: "refusal-instance",
        enabled: &[AUTO_SNAPSHOT],
        disable: AUTO_SNAPSHOT,
        remaining: &[],
    };
    create(&emulator, &row).await;

    // A resource type that carries no add-ons at all is as absent to this call
    // as a name nothing created: both are the `DoesNotExist` envelope.
    emulator
        .ok("AllocateStaticIp", json!({"staticIpName": "no-add-ons-here"}))
        .await;
    for (case, name) in [
        ("a name nothing created", "never-created"),
        ("a resource that carries no add-ons", "no-add-ons-here"),
    ] {
        let (status, body) = emulator
            .call(
                "DisableAddOn",
                json!({"resourceName": name, "addOnType": AUTO_SNAPSHOT}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str(), body["code"].as_str()),
            (400, "NotFoundException", Some("DoesNotExist")),
            "case {case}: expected the DoesNotExist envelope, got {status} {body:?}"
        );
    }

    // Outside the modelled `AddOnType` enum: refused as invalid input, never as
    // a disable that did nothing.
    let (status, body) = emulator
        .call(
            "DisableAddOn",
            json!({"resourceName": row.name, "addOnType": "TeaMaker"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case an unmodelled addOnType: expected a refusal, got {status} {body:?}"
    );

    let after = read_add_ons(&emulator, &row).await;
    assert_eq!(
        names(&after),
        vec![AUTO_SNAPSHOT.to_string()],
        "case a refused request: the add-ons are as they were, got {after:?}"
    );
}
