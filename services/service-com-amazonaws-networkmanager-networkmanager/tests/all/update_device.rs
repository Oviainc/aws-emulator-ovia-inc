// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateDevice`, driven over HTTP through the real router.
//!
//! The rule this suite pins is the one `update_link` and `update_site` pin for their own
//! resource: an update of an existing resource writes ONLY the members the request carries
//! — an absent member leaves the stored value as it was, a supplied one replaces it, and an
//! empty string removes the information the member held ("To remove information for any of
//! the parameters, specify an empty string", `smithy_query query="doc"
//! shape="UpdateDevice"`) — and rewrites nothing else, so the device keeps its id, its ARN,
//! its `CreatedAt` and its tags, and the reconfigured device is answered in the
//! non-terminal `UPDATING` state the clock then settles.
//!
//! Addressing a device the store does not hold under the path's global network is the
//! missing-resource answer (404 `ResourceNotFoundException`) and stores nothing; a
//! descriptor past its documented maximum is refused (400 `ValidationException`) exactly as
//! `CreateDevice` refuses it; and the one REFERENCE the body carries, `SiteId`, must name a
//! site of that global network, so one that names nothing is the same field failure
//! `CreateDevice` answers.
//!
//! Because the write lands in the store rather than in the answer alone, every reader of
//! the record answers from it: a device moved to another site leaves the `siteId` listing it
//! was in and joins the one it now belongs to.
//!
//! Nothing reaches into `crate::state`: a device is only ever read back through
//! `GetDevices`. The harness is the one `global_network_lifecycle` defines.

use serde_json::{json, Value};

use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{
    error_code, expected_scoped_arn, ids_of, Emulator, PAST_SETTLE,
};
use crate::link_lifecycle::{create_link_ok, plain_link};

/// The members every seeded device starts with, so a row that supplies none of them has
/// values to leave alone.
const BASE_DESCRIPTION: &str = "baseline device";
/// See [`BASE_DESCRIPTION`].
const BASE_TYPE: &str = "router";
/// See [`BASE_DESCRIPTION`].
const BASE_VENDOR: &str = "AnyCompany";
/// See [`BASE_DESCRIPTION`].
const BASE_MODEL: &str = "AC-1000";
/// See [`BASE_DESCRIPTION`].
const BASE_SERIAL_NUMBER: &str = "SN-0001";

/// The documented maximum length, in characters, of a device's `Vendor`, `Model` and
/// `SerialNumber`: "Constraints: Maximum length of 128 characters" on each
/// (`smithy_query query="doc" shape="UpdateDeviceRequest" member="Vendor"`, the same
/// sentence for `Model` and `SerialNumber`).
const MAX_DESCRIPTOR_CHARS: usize = 128;

/// The modelled maximum of a `ConstrainedString`, which bounds `Description` and `Type`
/// (`smithy_query query="operation" operation="UpdateDevice"`: len 0..256).
const MAX_CONSTRAINED_CHARS: usize = 256;

/// The fixed text the conflict error class documents, which carries no identifying detail
/// of its own: "There was a conflict processing the request. Updating or deleting the
/// resource can cause an inconsistent state." (`smithy_query query="doc"
/// shape="ConflictException"`) — the device is named by `ResourceId` / `ResourceType`, the
/// same division the not-found message rule keeps.
const CONFLICT_MESSAGE: &str = "There was a conflict processing the request. Updating or \
    deleting the resource can cause an inconsistent state.";

/// A subnet ARN of the form `AWSLocation$SubnetArn` accepts: its modelled pattern is
/// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:subnet\/subnet-[0-9a-f]{8,17}$`
/// (`smithy_query shape="AWSLocation"`).
const SUBNET_ARN: &str = "arn:aws:ec2:us-west-2:123456789012:subnet/subnet-0123456789abcdef0";

/// The `AWSLocation` every seeded device starts with.
fn base_aws_location() -> Value {
    json!({ "Zone": "us-west-2a" })
}

/// The `Location` every seeded device starts with.
fn base_location() -> Value {
    json!({ "Address": "1 Main St", "Latitude": "47.6062", "Longitude": "-122.3321" })
}

/// The `Tags` every seeded device carries, which no update may rewrite.
fn base_tags() -> Value {
    json!([{ "Key": "env", "Value": "test" }])
}

/// The `CreateDevice` body every row of this suite is seeded from.
fn seeded_device(site: &str) -> Value {
    json!({
        "SiteId": site,
        "AWSLocation": base_aws_location(),
        "Location": base_location(),
        "Description": BASE_DESCRIPTION,
        "Type": BASE_TYPE,
        "Vendor": BASE_VENDOR,
        "Model": BASE_MODEL,
        "SerialNumber": BASE_SERIAL_NUMBER,
        "Tags": base_tags(),
    })
}

/// One row of the update table: the request body, and the members the stored record must
/// hold afterwards. `site` is `Value::Null` for a row that leaves the device unsited.
struct UpdateCase {
    case: &'static str,
    body: Value,
    aws_location: Value,
    location: Value,
    description: Value,
    device_type: Value,
    vendor: Value,
    model: Value,
    serial_number: Value,
    /// `None` means "whatever site the row was seeded at"; `Some(value)` is the `SiteId`
    /// the record must answer (`Value::Null` for an unsited device).
    site: Option<Value>,
}

/// The rows of the update table. They differ along WHICH mutable members the request
/// carries (every one, a single one, none at all), and along the form of the value: an
/// empty string (the documented removal), the published maximum lengths, a non-ASCII and
/// multi-line value, a structure carrying one member, one carrying none, and the
/// reference member `SiteId` both re-pointed and removed.
fn update_cases(other_site: &str) -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "every mutable member supplied",
            body: json!({
                "AWSLocation": { "Zone": "eu-west-1b", "SubnetArn": SUBNET_ARN },
                "Location": { "Address": "2 High St", "Latitude": "51.5072", "Longitude": "-0.1276" },
                "Description": "updated device",
                "Type": "switch",
                "Vendor": "Example Networks",
                "Model": "EX-2000",
                "SerialNumber": "SN-9999",
            }),
            aws_location: json!({ "Zone": "eu-west-1b", "SubnetArn": SUBNET_ARN }),
            location: json!({ "Address": "2 High St", "Latitude": "51.5072", "Longitude": "-0.1276" }),
            description: json!("updated device"),
            device_type: json!("switch"),
            vendor: json!("Example Networks"),
            model: json!("EX-2000"),
            serial_number: json!("SN-9999"),
            site: None,
        },
        UpdateCase {
            case: "Description only, so the other members are left alone",
            body: json!({ "Description": "description only" }),
            aws_location: base_aws_location(),
            location: base_location(),
            description: json!("description only"),
            device_type: json!(BASE_TYPE),
            vendor: json!(BASE_VENDOR),
            model: json!(BASE_MODEL),
            serial_number: json!(BASE_SERIAL_NUMBER),
            site: None,
        },
        UpdateCase {
            case: "no mutable member at all, so nothing changes",
            body: json!({}),
            aws_location: base_aws_location(),
            location: base_location(),
            description: json!(BASE_DESCRIPTION),
            device_type: json!(BASE_TYPE),
            vendor: json!(BASE_VENDOR),
            model: json!(BASE_MODEL),
            serial_number: json!(BASE_SERIAL_NUMBER),
            site: None,
        },
        UpdateCase {
            case: "empty strings remove the information the descriptors held",
            body: json!({
                "Description": "",
                "Type": "",
                "Vendor": "",
                "Model": "",
                "SerialNumber": "",
            }),
            aws_location: base_aws_location(),
            location: base_location(),
            description: json!(""),
            device_type: json!(""),
            vendor: json!(""),
            model: json!(""),
            serial_number: json!(""),
            site: None,
        },
        UpdateCase {
            case: "an AWSLocation carrying one member replaces the whole structure",
            body: json!({ "AWSLocation": { "SubnetArn": SUBNET_ARN } }),
            aws_location: json!({ "SubnetArn": SUBNET_ARN }),
            location: base_location(),
            description: json!(BASE_DESCRIPTION),
            device_type: json!(BASE_TYPE),
            vendor: json!(BASE_VENDOR),
            model: json!(BASE_MODEL),
            serial_number: json!(BASE_SERIAL_NUMBER),
            site: None,
        },
        UpdateCase {
            case: "an empty Location removes every member it held",
            body: json!({ "Location": {} }),
            aws_location: base_aws_location(),
            location: json!({}),
            description: json!(BASE_DESCRIPTION),
            device_type: json!(BASE_TYPE),
            vendor: json!(BASE_VENDOR),
            model: json!(BASE_MODEL),
            serial_number: json!(BASE_SERIAL_NUMBER),
            site: None,
        },
        UpdateCase {
            case: "descriptors at their published maximum lengths",
            body: json!({
                "Description": "d".repeat(MAX_CONSTRAINED_CHARS),
                "Type": "t".repeat(MAX_CONSTRAINED_CHARS),
                "Vendor": "v".repeat(MAX_DESCRIPTOR_CHARS),
                "Model": "m".repeat(MAX_DESCRIPTOR_CHARS),
                "SerialNumber": "s".repeat(MAX_DESCRIPTOR_CHARS),
            }),
            aws_location: base_aws_location(),
            location: base_location(),
            description: json!("d".repeat(MAX_CONSTRAINED_CHARS)),
            device_type: json!("t".repeat(MAX_CONSTRAINED_CHARS)),
            vendor: json!("v".repeat(MAX_DESCRIPTOR_CHARS)),
            model: json!("m".repeat(MAX_DESCRIPTOR_CHARS)),
            serial_number: json!("s".repeat(MAX_DESCRIPTOR_CHARS)),
            site: None,
        },
        UpdateCase {
            case: "multi-line and non-ascii values",
            body: json!({
                "Description": "line one\nline two \u{1f310}",
                "Type": "rüter",
                "Vendor": "Ünternet",
                "Model": "モデル-1",
                "SerialNumber": "SN-\u{1f310}",
            }),
            aws_location: base_aws_location(),
            location: base_location(),
            description: json!("line one\nline two \u{1f310}"),
            device_type: json!("rüter"),
            vendor: json!("Ünternet"),
            model: json!("モデル-1"),
            serial_number: json!("SN-\u{1f310}"),
            site: None,
        },
        UpdateCase {
            case: "SiteId re-pointed to another site of the same global network",
            body: json!({ "SiteId": other_site }),
            aws_location: base_aws_location(),
            location: base_location(),
            description: json!(BASE_DESCRIPTION),
            device_type: json!(BASE_TYPE),
            vendor: json!(BASE_VENDOR),
            model: json!(BASE_MODEL),
            serial_number: json!(BASE_SERIAL_NUMBER),
            site: Some(json!(other_site)),
        },
        UpdateCase {
            case: "an empty SiteId leaves the device unsited",
            body: json!({ "SiteId": "" }),
            aws_location: base_aws_location(),
            location: base_location(),
            description: json!(BASE_DESCRIPTION),
            device_type: json!(BASE_TYPE),
            vendor: json!(BASE_VENDOR),
            model: json!(BASE_MODEL),
            serial_number: json!(BASE_SERIAL_NUMBER),
            site: Some(Value::Null),
        },
    ]
}

/// `UpdateDevice` on one device of `network`.
async fn update_device(
    emulator: &Emulator,
    network: &str,
    device_id: &str,
    body: Value,
) -> (u16, Value) {
    emulator
        .patch(
            &format!("/global-networks/{network}/devices/{device_id}"),
            body,
        )
        .await
}

/// Read one device back through `GetDevices`, the implemented reader for this resource.
async fn read_device(emulator: &Emulator, network: &str, device_id: &str) -> Value {
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/devices?deviceIds={device_id}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetDevices({device_id}): expected 200, got {status} with {body}"
    );
    let devices = body["Devices"]
        .as_array()
        .unwrap_or_else(|| panic!("GetDevices({device_id}): no Devices array in {body}"));
    assert_eq!(
        devices.len(),
        1,
        "GetDevices({device_id}): expected exactly that device, got {body}"
    );
    devices[0].clone()
}

/// A `Device` without its `State`, so two readings of one record can be compared member
/// for member without racing the transition the clock owes.
fn without_state(device: &Value) -> Value {
    let mut copy = device.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("State");
    }
    copy
}

#[tokio::test]
async fn update_device_writes_only_the_supplied_members_and_round_trips_through_get_devices() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-device").await;
    let site = emulator.create_site(&network, "hq").await;
    let other_site = emulator.create_site(&network, "branch").await;
    let rows = update_cases(&other_site);

    // One identically seeded device per row, plus a sibling no row names.
    let mut devices = Vec::new();
    for _ in &rows {
        devices.push(create_device_ok(&emulator, &network, seeded_device(&site)).await);
    }
    let sibling = create_device_ok(&emulator, &network, seeded_device(&site)).await;

    // The PENDING -> AVAILABLE transition the create owes is past before the first update,
    // so an update that restarted that clock would be visible here.
    std::thread::sleep(PAST_SETTLE);

    for (row, device_id) in rows.iter().zip(&devices) {
        let case = row.case;
        let before = read_device(&emulator, &network, device_id).await;
        let created_at = before["CreatedAt"].clone();

        let (status, answer) = update_device(&emulator, &network, device_id, row.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {answer}"
        );
        let device = &answer["Device"];

        let expected_site = row.site.clone().unwrap_or_else(|| json!(site.clone()));
        for (member, want) in [
            ("AWSLocation", &row.aws_location),
            ("Location", &row.location),
            ("Description", &row.description),
            ("Type", &row.device_type),
            ("Vendor", &row.vendor),
            ("Model", &row.model),
            ("SerialNumber", &row.serial_number),
            ("SiteId", &expected_site),
        ] {
            assert_eq!(
                &device[member], want,
                "case {case}: expected {member} {want}, got {device}"
            );
        }

        // Everything the update does not take as input is the record it already was.
        assert_eq!(
            device["DeviceId"],
            json!(device_id),
            "case {case}: DeviceId must be the addressed device, got {device}"
        );
        assert_eq!(
            device["DeviceArn"],
            json!(expected_scoped_arn("device", &network, device_id)),
            "case {case}: the ARN must not be rewritten, got {device}"
        );
        assert_eq!(
            device["GlobalNetworkId"],
            json!(network.clone()),
            "case {case}: the global network back-reference must not be rewritten, got {device}"
        );
        assert_eq!(
            device["CreatedAt"], created_at,
            "case {case}: CreatedAt must stay the create's instant, got {device}"
        );
        assert_eq!(
            device["Tags"],
            base_tags(),
            "case {case}: the tags must not be rewritten, got {device}"
        );
        assert_eq!(
            device["State"],
            json!("UPDATING"),
            "case {case}: an update answers the device in its transient state, got {device}"
        );

        // The answer is the stored record: GetDevices reports the same device, member for
        // member. `State` is compared on its own, since the transition the update owes can
        // fall due between the two calls.
        let stored = read_device(&emulator, &network, device_id).await;
        assert_eq!(
            without_state(&stored),
            without_state(device),
            "case {case}: GetDevices must answer the updated record, got {stored}"
        );

        // Once that transition is due, the device is settled again — and the members the
        // update wrote are still what it wrote.
        std::thread::sleep(PAST_SETTLE);
        let settled = read_device(&emulator, &network, device_id).await;
        assert_eq!(
            settled["State"],
            json!("AVAILABLE"),
            "case {case}: the device must settle once the transition is due, got {settled}"
        );
        assert_eq!(
            settled["Description"], row.description,
            "case {case}: the written Description must survive the transition, got {settled}"
        );
        assert_eq!(
            settled["SiteId"], expected_site,
            "case {case}: the written SiteId must survive the transition, got {settled}"
        );

        // The sibling nobody addressed still holds what it was created with.
        let untouched = read_device(&emulator, &network, &sibling).await;
        assert_eq!(
            untouched["Description"],
            json!(BASE_DESCRIPTION),
            "case {case}: an update must not touch another device, got {untouched}"
        );
        assert_eq!(
            untouched["SiteId"],
            json!(site.clone()),
            "case {case}: an update must not touch another device, got {untouched}"
        );
    }
}

#[tokio::test]
async fn an_update_while_the_create_is_still_pending_answers_the_update_it_is_working_on() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-device").await;
    // No wait: the create's PENDING -> AVAILABLE transition is still outstanding here.
    let device = create_device_ok(&emulator, &network, json!({ "Description": "settling" })).await;
    let fresh = read_device(&emulator, &network, &device).await;
    assert_eq!(
        fresh["State"],
        json!("PENDING"),
        "a freshly created device must still be settling, got {fresh}"
    );

    let (status, answer) = update_device(
        &emulator,
        &network,
        &device,
        json!({ "Description": "updated while pending" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an update of a settling device: expected 200, got {status} with {answer}"
    );
    assert_eq!(
        answer["Device"]["State"],
        json!("UPDATING"),
        "the later of the two outstanding requests is what the device reports, got {answer}"
    );

    // Once both transitions are due, the device is settled and holds what the update wrote.
    std::thread::sleep(PAST_SETTLE);
    let settled = read_device(&emulator, &network, &device).await;
    assert_eq!(
        settled["State"],
        json!("AVAILABLE"),
        "the device must settle once both transitions are due, got {settled}"
    );
    assert_eq!(
        settled["Description"],
        json!("updated while pending"),
        "the update must still be what the device holds, got {settled}"
    );
}

#[tokio::test]
async fn update_device_on_a_device_the_store_does_not_hold_is_not_found_and_stores_nothing() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-device").await;
    let site = emulator.create_site(&network, "hq").await;
    let device = create_device_ok(&emulator, &network, seeded_device(&site)).await;
    // A second global network, so a device addressed under the wrong parent is a device
    // that exists — just not there.
    let other_network = emulator.create_global_network("sibling").await;

    let unknown_network = "global-network-00000000000000099".to_string();
    let unknown_device = "device-00000000000000099".to_string();

    let cases: Vec<(&str, String, String, &str, String)> = vec![
        (
            "a global network that was never minted",
            unknown_network.clone(),
            device.clone(),
            "global-network",
            unknown_network.clone(),
        ),
        (
            "a device that was never minted",
            network.clone(),
            unknown_device.clone(),
            "device",
            unknown_device.clone(),
        ),
        (
            "an existing device under another global network",
            other_network.clone(),
            device.clone(),
            "device",
            device.clone(),
        ),
    ];

    for (case, path_network, path_device, resource_type, resource_id) in &cases {
        let (status, body) = update_device(
            &emulator,
            path_network,
            path_device,
            json!({ "Description": "must not be stored" }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: wrong error code in {body}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some(*resource_type),
            "case {case}: expected ResourceType {resource_type:?}, got {body}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(resource_id.as_str()),
            "case {case}: expected ResourceId {resource_id:?}, got {body}"
        );
    }

    // None of those refusals wrote anything: the live device is what it was created with.
    let stored = read_device(&emulator, &network, &device).await;
    assert_eq!(
        stored["Description"],
        json!(BASE_DESCRIPTION),
        "a refused update must store nothing, got {stored}"
    );
}

#[tokio::test]
async fn a_site_id_that_names_no_site_of_the_global_network_is_a_field_failure() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-device").await;
    let site = emulator.create_site(&network, "hq").await;
    let device = create_device_ok(&emulator, &network, seeded_device(&site)).await;

    // A site of ANOTHER global network exists but is not addressable from this one, and a
    // site that was deleted names nothing at all — both leave the reference unresolvable,
    // as does an id no create ever minted.
    let other_network = emulator.create_global_network("sibling").await;
    let foreign_site = emulator.create_site(&other_network, "elsewhere").await;
    let deleted_site = emulator.create_site(&network, "gone").await;
    let (status, removed) = emulator
        .delete(&format!("/global-networks/{network}/sites/{deleted_site}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteSite: expected 200, got {status} with {removed}"
    );

    for (case, site_id) in [
        ("a site that was never minted", "site-00000000000000099"),
        ("a site of another global network", foreign_site.as_str()),
        ("a site that was deleted", deleted_site.as_str()),
        ("an id that is not a site id at all", network.as_str()),
    ] {
        let (status, body) =
            update_device(&emulator, &network, &device, json!({ "SiteId": site_id })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: wrong error code in {body}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("FieldValidationFailed"),
            "case {case}: expected a field failure, got {body}"
        );
        assert_eq!(
            body["Fields"][0]["Name"].as_str(),
            Some("siteId"),
            "case {case}: the refusal must name the member at fault, got {body}"
        );
    }

    // No refusal moved the device, and it never left the listing of its own site.
    let stored = read_device(&emulator, &network, &device).await;
    assert_eq!(
        stored["SiteId"],
        json!(site.clone()),
        "a refused update must store nothing, got {stored}"
    );
}

#[tokio::test]
async fn a_moved_device_is_listed_under_the_site_it_now_sits_at() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-device").await;
    let from = emulator.create_site(&network, "from").await;
    let to = emulator.create_site(&network, "to").await;
    let device = create_device_ok(&emulator, &network, json!({ "SiteId": from })).await;
    // A device of the destination site that no update touches, so a listing that answered
    // the wrong site would be visible.
    let resident = create_device_ok(&emulator, &network, json!({ "SiteId": to })).await;

    // Each row is one update, and the devices each site's listing must answer afterwards.
    let rows: Vec<(&str, Value, Vec<String>, Vec<String>)> = vec![
        (
            "moved to the other site",
            json!({ "SiteId": to.clone() }),
            vec![],
            vec![device.clone(), resident.clone()],
        ),
        (
            "moved back again",
            json!({ "SiteId": from.clone() }),
            vec![device.clone()],
            vec![resident.clone()],
        ),
        (
            "unsited by the empty string",
            json!({ "SiteId": "" }),
            vec![],
            vec![resident.clone()],
        ),
        (
            "an update carrying no SiteId leaves it unsited",
            json!({ "Description": "still unsited" }),
            vec![],
            vec![resident.clone()],
        ),
    ];

    for (case, body, at_from, at_to) in rows {
        let (status, answer) = update_device(&emulator, &network, &device, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {answer}"
        );
        for (site, expected) in [(&from, &at_from), (&to, &at_to)] {
            let (status, listed) = emulator
                .get(&format!(
                    "/global-networks/{network}/devices?siteId={site}"
                ))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: GetDevices expected 200, got {status} with {listed}"
            );
            let mut want = expected.clone();
            want.sort();
            assert_eq!(
                ids_of(&listed, "Devices", "DeviceId"),
                want,
                "case {case}: the listing of site {site} must answer {want:?}, got {listed}"
            );
        }
    }
}

#[tokio::test]
async fn a_descriptor_past_its_documented_maximum_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-device").await;
    let site = emulator.create_site(&network, "hq").await;
    let device = create_device_ok(&emulator, &network, seeded_device(&site)).await;

    // The three members carrying the documented 128-character bound, each tested at the
    // bound (accepted) and one character past it (refused), in ASCII and in a script whose
    // characters are multi-byte — the bound is counted in characters, not bytes.
    for member in ["Vendor", "Model", "SerialNumber"] {
        for (case, character) in [("ascii", "v"), ("non-ascii", "ü")] {
            let at_bound = character.repeat(MAX_DESCRIPTOR_CHARS);
            let (status, accepted) = update_device(
                &emulator,
                &network,
                &device,
                json!({ member: at_bound.clone() }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {member}/{case} at {MAX_DESCRIPTOR_CHARS} characters: expected 200, got {status} with {accepted}"
            );
            assert_eq!(
                accepted["Device"][member],
                json!(at_bound),
                "case {member}/{case} at the bound: the value must be stored, got {accepted}"
            );

            let past_bound = character.repeat(MAX_DESCRIPTOR_CHARS + 1);
            let (status, refused) =
                update_device(&emulator, &network, &device, json!({ member: past_bound })).await;
            assert_eq!(
                status, 400,
                "case {member}/{case} at {} characters: expected 400, got {status} with {refused}",
                MAX_DESCRIPTOR_CHARS + 1
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {member}/{case} past the bound: wrong error code in {refused}"
            );
            assert_eq!(
                refused["Fields"][0]["Name"].as_str(),
                Some(member),
                "case {member}/{case} past the bound: the refusal must name the member, got {refused}"
            );

            // The refusal wrote nothing: the device still holds the accepted value.
            let stored = read_device(&emulator, &network, &device).await;
            assert_eq!(
                stored[member],
                json!(at_bound),
                "case {member}/{case}: a refused update must store nothing, got {stored}"
            );
        }
    }
}

#[tokio::test]
async fn moving_a_device_that_is_still_associated_with_a_link_is_refused_as_a_conflict() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-device").await;
    let site = emulator.create_site(&network, "hq").await;
    let other_site = emulator.create_site(&network, "branch").await;
    let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
    let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;

    let (status, associated) = emulator
        .post(
            &format!("/global-networks/{network}/link-associations"),
            json!({ "DeviceId": device, "LinkId": link }),
        )
        .await;
    assert_eq!(
        status, 200,
        "AssociateLink: expected 200, got {status} with {associated}"
    );

    // Both ways of taking the device away from the site the association stands on.
    for (case, body) in [
        (
            "moved to another site",
            json!({ "SiteId": other_site.clone() }),
        ),
        ("unsited by the empty string", json!({ "SiteId": "" })),
    ] {
        let (status, refused) = update_device(&emulator, &network, &device, body).await;
        assert_eq!(
            status, 409,
            "case {case}: expected 409, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case}: wrong error code in {refused}"
        );
        assert_eq!(
            refused["Message"].as_str(),
            Some(CONFLICT_MESSAGE),
            "case {case}: expected the documented message {CONFLICT_MESSAGE:?}, got {refused}"
        );
        assert_eq!(
            refused["ResourceId"].as_str(),
            Some(device.as_str()),
            "case {case}: the conflict must name the device, got {refused}"
        );
        assert_eq!(
            refused["ResourceType"].as_str(),
            Some("device"),
            "case {case}: the conflict must name the resource kind, got {refused}"
        );
    }

    // The refusals wrote nothing, and an update that leaves the device where it sits — the
    // same site named again, or no site member at all — is accepted.
    let stored = read_device(&emulator, &network, &device).await;
    assert_eq!(
        stored["SiteId"],
        json!(site.clone()),
        "a refused update must leave the device at its site, got {stored}"
    );
    for (case, body) in [
        (
            "the same site named again",
            json!({ "SiteId": site.clone(), "Description": "same site" }),
        ),
        (
            "no SiteId at all",
            json!({ "Description": "another member only" }),
        ),
    ] {
        let (status, answer) = update_device(&emulator, &network, &device, body).await;
        assert_eq!(
            status, 200,
            "case {case}: an update that keeps the site must be accepted, got {status} with {answer}"
        );
        assert_eq!(
            answer["Device"]["SiteId"],
            json!(site.clone()),
            "case {case}: the device must stay at its site, got {answer}"
        );
    }

    // Once the association is gone, the move the conflict refused is accepted.
    let (status, disassociated) = emulator
        .delete(&format!(
            "/global-networks/{network}/link-associations?deviceId={device}&linkId={link}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "DisassociateLink: expected 200, got {status} with {disassociated}"
    );
    let (status, moved) = update_device(
        &emulator,
        &network,
        &device,
        json!({ "SiteId": other_site.clone() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the move must be accepted once nothing ties the device to a link, got {status} with {moved}"
    );
    assert_eq!(
        moved["Device"]["SiteId"],
        json!(other_site.clone()),
        "the device must have moved, got {moved}"
    );
}
