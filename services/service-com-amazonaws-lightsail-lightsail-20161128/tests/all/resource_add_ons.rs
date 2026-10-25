// SPDX-License-Identifier: CC0-1.0

//! Enabling and modifying a Lightsail resource's add-ons, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: an add-on is PERSISTED on the one resource the request
//! names, keyed by its type — so `EnableAddOn` stores the add-on its
//! `addOnRequest` describes on that resource with the parameters the request
//! carried (and the parameter the service picks for itself where the request
//! named none), REPLACES the add-on of the same type the resource already
//! carried rather than adding a second one beside it, leaves every other add-on
//! alone, and answers the one operation-log entry the change appended. Every
//! later read of the resource (`GetInstance`, `GetDisk`) reports exactly the
//! add-ons the store holds. An add-on a resource cannot carry is refused with
//! the modelled `InvalidInputException` and stores nothing; a name that is not
//! an add-on-carrying resource of this account is refused with the service's
//! `DoesNotExist` envelope.

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target
// already exposes, and the modelled `TimeOfDay` check a sibling suite exposes:
// this suite drives the service through the same front door and reads a value
// against the same pattern rather than keeping second copies of either.
use crate::disk_block_storage::is_modelled_time_of_day;
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The zone every resource in this suite is created in; an add-on is not
/// regional, so one zone serves every case.
const ZONE: &str = "us-east-1a";

/// A Linux/Unix blueprint and the smallest plan: neither selects anything about
/// an add-on.
const BLUEPRINT: &str = "amazon_linux_2023";
/// See [`BLUEPRINT`].
const BUNDLE: &str = "nano_3_0";

/// The size of every block storage disk in this suite: the smallest Lightsail
/// publishes (8 GB), since the size selects nothing about an add-on.
const DISK_SIZE_IN_GB: i64 = 8;

/// A resource kind an add-on can be enabled on. The two the model gives an
/// `addOns` member, and the pair the modelled `AutoSnapshotAddOnRequest`
/// documentation names ("an Amazon Lightsail instance or disk").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Instance,
    Disk,
}

impl Kind {
    /// The modelled `ResourceType` value this kind reports, and that the
    /// operation-log entry of a change to it carries.
    fn resource_type(self) -> &'static str {
        match self {
            Self::Instance => "Instance",
            Self::Disk => "Disk",
        }
    }
}

/// Create the resource `name` of `kind`, carrying `add_ons` at creation time.
async fn create(emulator: &Emulator, kind: Kind, name: &str, add_ons: &[Value]) {
    let mut body = match kind {
        Kind::Instance => json!({
            "instanceNames": [name],
            "availabilityZone": ZONE,
            "blueprintId": BLUEPRINT,
            "bundleId": BUNDLE,
        }),
        Kind::Disk => json!({
            "diskName": name,
            "availabilityZone": ZONE,
            "sizeInGb": DISK_SIZE_IN_GB,
        }),
    };
    if !add_ons.is_empty() {
        body["addOns"] = json!(add_ons);
    }
    let operation = match kind {
        Kind::Instance => "CreateInstances",
        Kind::Disk => "CreateDisk",
    };
    emulator.ok(operation, body).await;
}

/// The `addOns` a read of the resource `name` of `kind` reports, in the order it
/// reports them.
async fn add_ons(emulator: &Emulator, kind: Kind, name: &str) -> Vec<Value> {
    let resource = match kind {
        Kind::Instance => {
            emulator
                .ok("GetInstance", json!({"instanceName": name}))
                .await["instance"]
                .clone()
        }
        Kind::Disk => emulator.ok("GetDisk", json!({"diskName": name})).await["disk"].clone(),
    };
    resource["addOns"].as_array().cloned().unwrap_or_default()
}

/// The add-on named `add_on_name` among `reported`, or `None` when the resource
/// carries none of that type.
fn add_on_named<'a>(reported: &'a [Value], add_on_name: &str) -> Option<&'a Value> {
    reported
        .iter()
        .find(|add_on| add_on["name"].as_str() == Some(add_on_name))
}

/// Enable `request` on the resource `name`, requiring success, and answer the
/// one operation-log entry the call reports.
async fn enable(emulator: &Emulator, case: &str, name: &str, request: Value) -> Value {
    let body = emulator
        .ok(
            "EnableAddOn",
            json!({"resourceName": name, "addOnRequest": request}),
        )
        .await;
    let operations = body["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an operations array, got {body:?}"))
        .clone();
    assert_eq!(
        operations.len(),
        1,
        "case {case:?}: EnableAddOn acts on exactly one resource, got {operations:?}"
    );
    operations[0].clone()
}

/// Assert everything the rule says about the operation-log entry an
/// `EnableAddOn` call answers with: it names the resource it changed, it is the
/// modelled operation type, and the change is already done.
fn assert_operation(case: &str, operation: &Value, kind: Kind, name: &str) {
    assert_eq!(
        operation["operationType"].as_str(),
        Some("EnableAddOn"),
        "case {case:?}: expected the EnableAddOn operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(name),
        "case {case:?}: expected the operation to name {name}, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some(kind.resource_type()),
        "case {case:?}: expected the resource's own type, got {operation:?}"
    );
    assert_eq!(
        operation["status"].as_str(),
        Some("Succeeded"),
        "case {case:?}: the add-on is enabled when the call answers, got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case {case:?}: a finished operation is terminal, got {operation:?}"
    );
    assert!(
        is_uuid_shaped(operation["id"].as_str().unwrap_or_default()),
        "case {case:?}: expected a uuid-shaped operation id, got {operation:?}"
    );
    assert!(
        operation["createdAt"].is_number(),
        "case {case:?}: expected a creation timestamp, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["availabilityZone"].as_str(),
        Some(ZONE),
        "case {case:?}: expected the resource's own location, got {operation:?}"
    );
}

// ---------------------------------------------------------------------------
// The enable table
// ---------------------------------------------------------------------------

/// What the `snapshotTimeOfDay` of the stored add-on must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WantTime {
    /// The hour the request named, verbatim.
    Exact(&'static str),
    /// The request named none, so the service picks one: any value the modelled
    /// `TimeOfDay` pattern admits, on the hourly increment the modelled
    /// `AutoSnapshotAddOnRequest$snapshotTimeOfDay` documentation requires
    /// ("Must be in `HH:00` format, and in an hourly increment").
    ServiceChosen,
    /// This add-on has no snapshot hour at all.
    Absent,
}

/// One `EnableAddOn` request and the add-on the resource must then carry.
///
/// The rows differ along everything the rule generalises over: the kind of
/// resource the add-on is enabled on, which modelled `AddOnType` it is, whether
/// the request carries the add-on's own parameter object at all, whether that
/// object carries its members, and — for the automatic snapshot add-on — the
/// hour itself, including each published bound of the modelled `TimeOfDay`.
struct Row {
    case: &'static str,
    kind: Kind,
    /// The resource this row creates and enables its add-on on; distinct per
    /// row, so one row cannot see another's resource.
    name: &'static str,
    /// The `addOnRequest` the request carries, verbatim.
    request: Value,
    /// The `name` the stored add-on must report, which is its type.
    want_name: &'static str,
    want_time: WantTime,
    want_threshold: Option<&'static str>,
    want_duration: Option<&'static str>,
}

/// The table.
///
/// The two add-on types are the modelled `AddOnType` enum's members
/// (`AutoSnapshot`, `StopInstanceOnIdle`); `00:00` and `23:00` are the first and
/// last hour the modelled `TimeOfDay` pattern
/// `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$` admits on an hourly increment.
fn enable_rows() -> Vec<Row> {
    vec![
        Row {
            case: "AutoSnapshot on an instance at a named hour",
            kind: Kind::Instance,
            name: "add-on-instance-named-hour",
            request: json!({
                "addOnType": "AutoSnapshot",
                "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": "06:00"},
            }),
            want_name: "AutoSnapshot",
            want_time: WantTime::Exact("06:00"),
            want_threshold: None,
            want_duration: None,
        },
        Row {
            case: "AutoSnapshot on an instance at the first hour of the day",
            kind: Kind::Instance,
            name: "add-on-instance-first-hour",
            request: json!({
                "addOnType": "AutoSnapshot",
                "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": "00:00"},
            }),
            want_name: "AutoSnapshot",
            want_time: WantTime::Exact("00:00"),
            want_threshold: None,
            want_duration: None,
        },
        Row {
            case: "AutoSnapshot on an instance at the last hour of the day",
            kind: Kind::Instance,
            name: "add-on-instance-last-hour",
            request: json!({
                "addOnType": "AutoSnapshot",
                "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": "23:00"},
            }),
            want_name: "AutoSnapshot",
            want_time: WantTime::Exact("23:00"),
            want_threshold: None,
            want_duration: None,
        },
        Row {
            case: "AutoSnapshot on an instance with no parameter object",
            kind: Kind::Instance,
            name: "add-on-instance-no-parameters",
            request: json!({"addOnType": "AutoSnapshot"}),
            want_name: "AutoSnapshot",
            want_time: WantTime::ServiceChosen,
            want_threshold: None,
            want_duration: None,
        },
        Row {
            case: "AutoSnapshot on an instance with an empty parameter object",
            kind: Kind::Instance,
            name: "add-on-instance-empty-parameters",
            request: json!({"addOnType": "AutoSnapshot", "autoSnapshotAddOnRequest": {}}),
            want_name: "AutoSnapshot",
            want_time: WantTime::ServiceChosen,
            want_threshold: None,
            want_duration: None,
        },
        Row {
            case: "AutoSnapshot on a disk at a named hour",
            kind: Kind::Disk,
            name: "add-on-disk-named-hour",
            request: json!({
                "addOnType": "AutoSnapshot",
                "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": "13:00"},
            }),
            want_name: "AutoSnapshot",
            want_time: WantTime::Exact("13:00"),
            want_threshold: None,
            want_duration: None,
        },
        Row {
            case: "AutoSnapshot on a disk with no parameter object",
            kind: Kind::Disk,
            name: "add-on-disk-no-parameters",
            request: json!({"addOnType": "AutoSnapshot"}),
            want_name: "AutoSnapshot",
            want_time: WantTime::ServiceChosen,
            want_threshold: None,
            want_duration: None,
        },
        Row {
            case: "StopInstanceOnIdle on an instance with both parameters",
            kind: Kind::Instance,
            name: "add-on-instance-idle-stop",
            request: json!({
                "addOnType": "StopInstanceOnIdle",
                "stopInstanceOnIdleRequest": {"threshold": "20", "duration": "30"},
            }),
            want_name: "StopInstanceOnIdle",
            want_time: WantTime::Absent,
            want_threshold: Some("20"),
            want_duration: Some("30"),
        },
        Row {
            case: "StopInstanceOnIdle on an instance with no parameter object",
            kind: Kind::Instance,
            name: "add-on-instance-idle-stop-bare",
            request: json!({"addOnType": "StopInstanceOnIdle"}),
            want_name: "StopInstanceOnIdle",
            want_time: WantTime::Absent,
            want_threshold: None,
            want_duration: None,
        },
    ]
}

/// Every row of the table: the add-on the request describes is stored on the
/// resource it names, and every later read reports it.
#[tokio::test]
async fn enable_add_on_stores_the_add_on_the_request_describes() {
    let emulator = Emulator::new();
    for row in enable_rows() {
        let case = row.case;
        create(&emulator, row.kind, row.name, &[]).await;
        let before = add_ons(&emulator, row.kind, row.name).await;
        assert!(
            before.is_empty(),
            "case {case:?}: a resource created with no add-ons carries none, got {before:?}"
        );

        let operation = enable(&emulator, case, row.name, row.request.clone()).await;
        assert_operation(case, &operation, row.kind, row.name);

        // The operation-log entry the call reported is the one the log holds.
        let logged = emulator
            .ok(
                "GetOperation",
                json!({"operationId": operation["id"].as_str().unwrap_or_default()}),
            )
            .await["operation"]
            .clone();
        assert_eq!(
            logged, operation,
            "case {case:?}: expected GetOperation to report the same entry {operation:?}, \
             got {logged:?}"
        );

        let reported = add_ons(&emulator, row.kind, row.name).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly the one enabled add-on, got {reported:?}"
        );
        let add_on = &reported[0];
        assert_eq!(
            add_on["name"].as_str(),
            Some(row.want_name),
            "case {case:?}: an add-on is named for its type, got {add_on:?}"
        );
        assert_eq!(
            add_on["status"].as_str(),
            Some("Enabled"),
            "case {case:?}: an enabled add-on reports Enabled, got {add_on:?}"
        );
        match row.want_time {
            WantTime::Exact(want) => assert_eq!(
                add_on["snapshotTimeOfDay"].as_str(),
                Some(want),
                "case {case:?}: expected the hour the request named ({want}), got {add_on:?}"
            ),
            WantTime::ServiceChosen => {
                let chosen = add_on["snapshotTimeOfDay"].as_str().unwrap_or_default();
                assert!(
                    is_modelled_time_of_day(chosen) && chosen.ends_with(":00"),
                    "case {case:?}: expected an hourly HH:00 snapshot time, got {add_on:?}"
                );
            }
            WantTime::Absent => assert!(
                add_on["snapshotTimeOfDay"].is_null(),
                "case {case:?}: an add-on that takes no snapshot has no snapshot hour, \
                 got {add_on:?}"
            ),
        }
        assert_eq!(
            add_on["threshold"].as_str(),
            row.want_threshold,
            "case {case:?}: expected threshold {:?}, got {add_on:?}",
            row.want_threshold
        );
        assert_eq!(
            add_on["duration"].as_str(),
            row.want_duration,
            "case {case:?}: expected duration {:?}, got {add_on:?}",
            row.want_duration
        );

        // Reading is not enabling: a second read reports exactly the same
        // add-on, hour included.
        let again = add_ons(&emulator, row.kind, row.name).await;
        assert_eq!(
            again, reported,
            "case {case:?}: expected every read to report {reported:?}, got {again:?}"
        );
    }
}

/// An add-on is keyed by its type: enabling one the resource already carries
/// modifies THAT one — whichever call enabled it first — and leaves every other
/// add-on it carries alone.
#[tokio::test]
async fn enable_add_on_modifies_the_add_on_of_the_same_type() {
    let emulator = Emulator::new();
    let name = "add-on-modify-instance";
    // The instance is created with the automatic snapshot add-on already
    // enabled, so the first `EnableAddOn` below MODIFIES what another call
    // stored rather than one of its own.
    create(
        &emulator,
        Kind::Instance,
        name,
        &[json!({
            "addOnType": "AutoSnapshot",
            "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": "05:00"},
        })],
    )
    .await;
    let created = add_ons(&emulator, Kind::Instance, name).await;
    assert_eq!(
        add_on_named(&created, "AutoSnapshot").map(|add_on| add_on["snapshotTimeOfDay"].clone()),
        Some(json!("05:00")),
        "case \"created with an add-on\": expected the hour the create named, got {created:?}"
    );

    for (case, hour) in [
        ("modify the add-on the create enabled", "17:00"),
        ("modify the add-on a previous EnableAddOn enabled", "08:00"),
    ] {
        let operation = enable(
            &emulator,
            case,
            name,
            json!({
                "addOnType": "AutoSnapshot",
                "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": hour},
            }),
        )
        .await;
        assert_operation(case, &operation, Kind::Instance, name);
        let reported = add_ons(&emulator, Kind::Instance, name).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: a resource carries one add-on per type, got {reported:?}"
        );
        assert_eq!(
            reported[0]["snapshotTimeOfDay"].as_str(),
            Some(hour),
            "case {case:?}: expected the modified hour {hour}, got {reported:?}"
        );
    }

    // A DIFFERENT type is a different add-on: it joins the one already enabled
    // instead of replacing it.
    let case = "enable a second add-on type";
    enable(
        &emulator,
        case,
        name,
        json!({
            "addOnType": "StopInstanceOnIdle",
            "stopInstanceOnIdleRequest": {"threshold": "15", "duration": "45"},
        }),
    )
    .await;
    let reported = add_ons(&emulator, Kind::Instance, name).await;
    assert_eq!(
        reported.len(),
        2,
        "case {case:?}: expected both add-ons to be enabled, got {reported:?}"
    );
    let auto = add_on_named(&reported, "AutoSnapshot")
        .unwrap_or_else(|| panic!("case {case:?}: expected the AutoSnapshot add-on in {reported:?}"));
    assert_eq!(
        auto["snapshotTimeOfDay"].as_str(),
        Some("08:00"),
        "case {case:?}: the other add-on is left as it was, got {auto:?}"
    );
    let idle = add_on_named(&reported, "StopInstanceOnIdle").unwrap_or_else(|| {
        panic!("case {case:?}: expected the StopInstanceOnIdle add-on in {reported:?}")
    });
    assert_eq!(
        (idle["threshold"].as_str(), idle["duration"].as_str()),
        (Some("15"), Some("45")),
        "case {case:?}: expected the parameters the request carried, got {idle:?}"
    );
}

/// An add-on a resource cannot carry is refused, and stores nothing: of the two
/// modelled `AddOnType` members, only the automatic snapshot one describes a
/// block storage disk — the other stops an idle virtual computer, which a disk
/// is not.
#[tokio::test]
async fn enable_add_on_refuses_an_add_on_the_resource_cannot_carry() {
    let emulator = Emulator::new();
    let name = "add-on-disk-refusal";
    create(
        &emulator,
        Kind::Disk,
        name,
        &[json!({
            "addOnType": "AutoSnapshot",
            "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": "04:00"},
        })],
    )
    .await;

    for case in [
        "the idle-stop add-on with its parameters",
        "the idle-stop add-on with no parameters",
    ] {
        let mut request = json!({"addOnType": "StopInstanceOnIdle"});
        if case.ends_with("with its parameters") {
            request["stopInstanceOnIdleRequest"] = json!({"threshold": "20", "duration": "30"});
        }
        let (status, body) = emulator
            .call(
                "EnableAddOn",
                json!({"resourceName": name, "addOnRequest": request}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled InvalidInputException status, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case:?}: expected the modelled InvalidInputException, got {body:?}"
        );
        // A refusal built from the request names the member that is wrong and
        // the value that broke it. The rules are shared with the create calls,
        // so the sentence is theirs — but the member path is this request's own
        // (`addOnRequest.addOnType`), never the create calls' `addOns[]`.
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("addOnRequest.addOnType")
                && message.contains("StopInstanceOnIdle")
                && !message.contains("addOns[]"),
            "case {case:?}: expected the message to name addOnRequest.addOnType and the add-on \
             asked for, got {body:?}"
        );

        // The refusal changed nothing: the disk still carries exactly the
        // add-on it was created with.
        let reported = add_ons(&emulator, Kind::Disk, name).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: a refused request stores nothing, got {reported:?}"
        );
        assert_eq!(
            (
                reported[0]["name"].as_str(),
                reported[0]["snapshotTimeOfDay"].as_str()
            ),
            (Some("AutoSnapshot"), Some("04:00")),
            "case {case:?}: expected the disk's own add-on untouched, got {reported:?}"
        );
    }
}

/// A `resourceName` that is not an add-on-carrying resource of this account is
/// refused with the service's `DoesNotExist` envelope: a name nothing was ever
/// created under, a resource of a kind that carries no add-ons, and a name whose
/// resource has since been deleted.
#[tokio::test]
async fn enable_add_on_refuses_a_name_that_carries_no_add_ons() {
    let emulator = Emulator::new();

    // A resource of a kind with no `addOns` member at all.
    emulator
        .ok("AllocateStaticIp", json!({"staticIpName": "add-on-static-ip"}))
        .await;
    // A resource that existed and does not any more.
    create(&emulator, Kind::Instance, "add-on-deleted-instance", &[]).await;
    emulator
        .ok(
            "DeleteInstance",
            json!({"instanceName": "add-on-deleted-instance"}),
        )
        .await;

    for (case, name) in [
        ("a name nothing was created under", "add-on-no-such-resource"),
        ("a static IP, which carries no add-ons", "add-on-static-ip"),
        ("an instance that has been deleted", "add-on-deleted-instance"),
    ] {
        let (status, body) = emulator
            .call(
                "EnableAddOn",
                json!({
                    "resourceName": name,
                    "addOnRequest": {"addOnType": "AutoSnapshot"},
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the DoesNotExist status, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case:?}: expected the DoesNotExist code, got {body:?}"
        );
    }
}

/// A Lightsail name is unique per resource TYPE, so one name can be both an
/// instance's and a disk's. The shared resolver every add-on operation goes
/// through answers the INSTANCE first (it is the kind that can carry every
/// modelled add-on), and this pins it: the instance gets the add-on and the disk
/// of the same name keeps the add-ons it had.
#[tokio::test]
async fn enable_add_on_resolves_a_colliding_name_to_the_instance() {
    let emulator = Emulator::new();
    let name = "add-on-name-collision";
    create(&emulator, Kind::Instance, name, &[]).await;
    create(
        &emulator,
        Kind::Disk,
        name,
        &[json!({
            "addOnType": "AutoSnapshot",
            "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": "02:00"},
        })],
    )
    .await;

    let case = "one name held by both an instance and a disk";
    let operation = enable(
        &emulator,
        case,
        name,
        json!({
            "addOnType": "AutoSnapshot",
            "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": "09:00"},
        }),
    )
    .await;
    // The log entry says which resource was changed, so a caller can tell which
    // of the two the add-on landed on.
    assert_operation(case, &operation, Kind::Instance, name);

    let on_instance = add_ons(&emulator, Kind::Instance, name).await;
    assert_eq!(
        on_instance.len(),
        1,
        "case {case:?}: expected the instance to carry the enabled add-on, got {on_instance:?}"
    );
    assert_eq!(
        on_instance[0]["snapshotTimeOfDay"].as_str(),
        Some("09:00"),
        "case {case:?}: expected the hour the request named on the instance, got {on_instance:?}"
    );

    let on_disk = add_ons(&emulator, Kind::Disk, name).await;
    assert_eq!(
        on_disk.len(),
        1,
        "case {case:?}: expected the disk's own add-ons untouched, got {on_disk:?}"
    );
    assert_eq!(
        on_disk[0]["snapshotTimeOfDay"].as_str(),
        Some("02:00"),
        "case {case:?}: expected the hour the disk was created with, got {on_disk:?}"
    );
}
