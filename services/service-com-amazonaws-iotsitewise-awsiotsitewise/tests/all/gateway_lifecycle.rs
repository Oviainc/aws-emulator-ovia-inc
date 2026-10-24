// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise gateway slice (`CreateGateway` ->
//! `DescribeGateway`), driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a real
//! client sees.
//!
//! The rules these suites pin:
//!
//! * a created gateway is persisted and reads back exactly as it was created, with
//!   the ARN form `CreateGateway` documents and no capability it was not given;
//! * a gateway specifies exactly ONE platform type, and that platform's device
//!   (`greengrass` group ARN, `greengrassV2` core device thing, `siemensIE` IoT
//!   Core thing) hosts at most one gateway, so a second create naming a device or
//!   a name already in use is the modelled 409 naming the gateway it collided with;
//! * `gatewayVersion` defaults to 2, and a version-3 GreengrassV2 gateway also
//!   carries `coreDeviceOperatingSystem`;
//! * an id that resolves to no stored gateway is a modelled 404 rather than a
//!   fabricated answer.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`) is the one
//! the asset suite already has; this suite reuses it rather than growing a second.

use serde_json::{json, Value};

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::asset_lifecycle::{
    call, error_code, is_modelled_id, router, router_with_engine, REGION,
};

/// Where every gateway request goes, as `OPERATIONS.md` reports the route:
/// `POST /20200301/gateways`, `GET /20200301/gateways/{gatewayId}`.
const GATEWAYS: &str = "/20200301/gateways";

/// A `greengrassV2` platform naming `thing`, optionally with an operating system.
///
/// The three `CoreDeviceOperatingSystem` values used below — `LINUX_AARCH64`,
/// `LINUX_AMD64`, `WINDOWS_AMD64` — are the enum's whole value list
/// (smithy_query query="shape" shape="CoreDeviceOperatingSystem").
fn greengrass_v2(thing: &str, operating_system: Option<&str>) -> Value {
    match operating_system {
        Some(os) => json!({
            "greengrassV2": { "coreDeviceThingName": thing, "coreDeviceOperatingSystem": os }
        }),
        None => json!({ "greengrassV2": { "coreDeviceThingName": thing } }),
    }
}

/// A legacy `greengrass` (V1) platform naming a Greengrass group ARN.
fn greengrass(group: &str) -> Value {
    json!({ "greengrass": { "groupArn": format!("arn:aws:greengrass:{REGION}:123456789012:/greengrass/groups/{group}") } })
}

/// A `siemensIE` platform naming an IoT Core thing.
fn siemens_ie(thing: &str) -> Value {
    json!({ "siemensIE": { "iotCoreThingName": thing } })
}

/// One row of the create-and-read-back table: a platform type, the
/// `gatewayVersion` the request sends (or none at all) and the tags it carries.
#[derive(Debug)]
struct CreateCase {
    label: &'static str,
    platform: Value,
    version: Option<&'static str>,
    /// What `DescribeGateway` must report for `gatewayVersion`.
    expected_version: &'static str,
    tags: Option<Value>,
}

/// The rows: every platform member `GatewayPlatform` declares
/// (smithy_query query="shape" shape="GatewayPlatform": `greengrass`,
/// `greengrassV2`, `siemensIE`), crossed with an absent / explicit
/// `gatewayVersion` and with / without tags.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "greengrass-v1-default-version",
            platform: greengrass("legacy-group"),
            version: None,
            // "If not specified, the default is 2 (Classic streams, V2 gateway)."
            expected_version: "2",
            tags: None,
        },
        CreateCase {
            label: "greengrassV2-default-version",
            platform: greengrass_v2("core-device-default", None),
            version: None,
            expected_version: "2",
            tags: None,
        },
        CreateCase {
            label: "greengrassV2-explicit-v2",
            platform: greengrass_v2("core-device-v2", None),
            version: Some("2"),
            expected_version: "2",
            tags: Some(json!({ "stage": "test" })),
        },
        CreateCase {
            label: "greengrassV2-v3-linux-aarch64",
            platform: greengrass_v2("core-device-aarch64", Some("LINUX_AARCH64")),
            version: Some("3"),
            expected_version: "3",
            tags: None,
        },
        CreateCase {
            label: "greengrassV2-v3-linux-amd64",
            platform: greengrass_v2("core-device-amd64", Some("LINUX_AMD64")),
            version: Some("3"),
            expected_version: "3",
            tags: Some(json!({ "team": "ops", "env": "edge" })),
        },
        CreateCase {
            label: "greengrassV2-v3-windows-amd64",
            platform: greengrass_v2("core-device-windows", Some("WINDOWS_AMD64")),
            version: Some("3"),
            expected_version: "3",
            tags: None,
        },
        CreateCase {
            label: "siemensIE-explicit-v2",
            platform: siemens_ie("siemens-thing"),
            version: Some("2"),
            expected_version: "2",
            tags: None,
        },
    ]
}

/// The body one row sends.
fn create_body(case: &CreateCase, name: &str) -> Value {
    let mut body = json!({ "gatewayName": name });
    let object = body.as_object_mut().expect("an object body");
    object.insert("gatewayPlatform".to_string(), case.platform.clone());
    if let Some(version) = case.version {
        object.insert("gatewayVersion".to_string(), json!(version));
    }
    if let Some(tags) = case.tags.clone() {
        object.insert("tags".to_string(), tags);
    }
    body
}

#[tokio::test]
async fn created_gateway_reads_back_through_describe_for_every_platform() {
    let (router, engine) = router_with_engine();
    let mut seen_ids: Vec<String> = Vec::new();

    for case in create_cases() {
        let name = format!("gateway-{}", case.label);
        let (status, created) =
            call(&router, "POST", GATEWAYS, Some(create_body(&case, &name))).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201, got {status} {created}",
            case.label
        );

        let id = created["gatewayId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_id(&id),
            "case {:?}: gatewayId must be a modelled UUID, got {created}",
            case.label
        );
        assert_eq!(
            created["gatewayArn"].as_str().unwrap_or(""),
            format!(
                "arn:aws:iotsitewise:{REGION}:{}:gateway/{id}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {:?}: unexpected gatewayArn in {created}",
            case.label
        );
        assert!(
            !seen_ids.contains(&id),
            "case {:?}: gatewayId {id} was already minted for another gateway",
            case.label
        );
        seen_ids.push(id.clone());

        let (status, described) = call(&router, "GET", &format!("{GATEWAYS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeGateway expected 200, got {status} {described}",
            case.label
        );
        assert_eq!(
            described["gatewayId"].as_str().unwrap_or(""),
            id,
            "case {:?}: DescribeGateway reported another id in {described}",
            case.label
        );
        assert_eq!(
            described["gatewayName"].as_str().unwrap_or(""),
            name,
            "case {:?}: DescribeGateway reported another name in {described}",
            case.label
        );
        assert_eq!(
            described["gatewayArn"], created["gatewayArn"],
            "case {:?}: DescribeGateway reported another ARN in {described}",
            case.label
        );
        assert_eq!(
            described["gatewayPlatform"], case.platform,
            "case {:?}: DescribeGateway reported another platform in {described}",
            case.label
        );
        assert_eq!(
            described["gatewayVersion"].as_str().unwrap_or(""),
            case.expected_version,
            "case {:?}: unexpected gatewayVersion in {described}",
            case.label
        );
        // A gateway nobody configured a capability on reports an empty list, never a
        // capability it was not given.
        assert_eq!(
            described["gatewayCapabilitySummaries"],
            json!([]),
            "case {:?}: a new gateway has no capability, got {described}",
            case.label
        );
        let creation = described["creationDate"].as_f64().unwrap_or(0.0);
        let last_update = described["lastUpdateDate"].as_f64().unwrap_or(0.0);
        assert!(
            creation > 0.0,
            "case {:?}: creationDate must be a real instant, got {described}",
            case.label
        );
        assert_eq!(
            creation, last_update,
            "case {:?}: an untouched gateway's lastUpdateDate is its creationDate, got {described}",
            case.label
        );

        // Reading does not change what the next caller sees.
        let (status, again) = call(&router, "GET", &format!("{GATEWAYS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: second DescribeGateway expected 200, got {status} {again}",
            case.label
        );
        assert_eq!(
            again, described,
            "case {:?}: DescribeGateway answered differently on the second read: {again}",
            case.label
        );

        // The tags the create carried are persisted on the gateway. Asserted against
        // the store because this service's tagging operations are still scaffolds, so
        // there is no reader for them on the wire yet.
        let expected_tags: Vec<(String, String)> = case
            .tags
            .as_ref()
            .and_then(Value::as_object)
            .map(|tags| {
                let mut pairs: Vec<(String, String)> = tags
                    .iter()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().unwrap_or_default().to_string())
                    })
                    .collect();
                pairs.sort();
                pairs
            })
            .unwrap_or_default();
        let state = service_state(&*engine).expect("the iotsitewise store");
        let mut stored_tags: Vec<(String, String)> = state
            .lock()
            .gateways
            .get(&id)
            .unwrap_or_else(|| panic!("case {:?}: gateway {id} was not persisted", case.label))
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        stored_tags.sort();
        assert_eq!(
            stored_tags, expected_tags,
            "case {:?}: unexpected stored tags for gateway {id}",
            case.label
        );
    }

    assert_eq!(
        seen_ids.len(),
        create_cases().len(),
        "every row must have created a gateway, got {seen_ids:?}"
    );
}

#[tokio::test]
async fn a_platform_device_hosts_at_most_one_gateway() {
    let router = router();

    // One row per platform type: the device the first gateway takes, and the second
    // create that names the SAME device under a different name and version.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "greengrass-group",
            greengrass("shared-group"),
            greengrass("shared-group"),
        ),
        (
            "greengrassV2-core-device",
            greengrass_v2("shared-core-device", None),
            greengrass_v2("shared-core-device", Some("LINUX_AMD64")),
        ),
        (
            "siemensIE-thing",
            siemens_ie("shared-siemens-thing"),
            siemens_ie("shared-siemens-thing"),
        ),
    ];

    for (case, first_platform, second_platform) in cases {
        let (status, created) = call(
            &router,
            "POST",
            GATEWAYS,
            Some(json!({
                "gatewayName": format!("first-{case}"),
                "gatewayPlatform": first_platform,
            })),
        )
        .await;
        assert_eq!(status, 201, "case {case:?}: expected 201, got {status} {created}");
        let id = created["gatewayId"].as_str().unwrap_or("").to_string();

        let (status, conflict) = call(
            &router,
            "POST",
            GATEWAYS,
            Some(json!({
                "gatewayName": format!("second-{case}"),
                "gatewayPlatform": second_platform,
                "gatewayVersion": "3",
            })),
        )
        .await;
        // The collision is on a request ATTRIBUTE already in use by another resource
        // — the platform device — and not on the identity of the gateway this create
        // would make, so it is the terminal request error and not
        // `ResourceAlreadyExistsException`, which this service keeps for a resource
        // that is itself already there (see the gateway-name suite below).
        assert_eq!(
            status, 400,
            "case {case:?}: a duplicate device expected 400, got {status} {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {conflict}"
        );
        // The refusal names the DIMENSION the caller supplied, not an id it never
        // sent: diagnosis has to point at the thing name or group ARN that collided.
        let message = conflict["message"].as_str().unwrap_or("");
        let device = device_name(&first_platform_of(case));
        assert!(
            message.contains(&device),
            "case {case:?}: the refusal must name the colliding device {device:?}, got {conflict}"
        );
        assert!(
            !message.contains(&id),
            "case {case:?}: the refusal must not name a gateway id the caller never sent, got {conflict}"
        );

        // The gateway already on the device is untouched, and a create naming a FREE
        // device still succeeds.
        let (status, described) = call(&router, "GET", &format!("{GATEWAYS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the pre-existing gateway must survive, got {status} {described}"
        );
        let (status, other) = call(
            &router,
            "POST",
            GATEWAYS,
            Some(json!({
                "gatewayName": format!("third-{case}"),
                "gatewayPlatform": greengrass_v2(&format!("free-{case}"), None),
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: a free device must still be accepted, got {status} {other}"
        );
    }
}

/// The platform of the `case` row of [`a_platform_device_hosts_at_most_one_gateway`],
/// by the row's name — so the assertion above reads the device out of the same place
/// the row built it from.
fn first_platform_of(case: &str) -> Value {
    match case {
        "greengrass-group" => greengrass("shared-group"),
        "greengrassV2-core-device" => greengrass_v2("shared-core-device", None),
        _ => siemens_ie("shared-siemens-thing"),
    }
}

/// The device a `gatewayPlatform` names, whichever platform type it is.
fn device_name(platform: &Value) -> String {
    for (key, member) in [
        ("greengrass", "groupArn"),
        ("greengrassV2", "coreDeviceThingName"),
        ("siemensIE", "iotCoreThingName"),
    ] {
        if let Some(value) = platform[key][member].as_str() {
            return value.to_string();
        }
    }
    panic!("the platform {platform} names no device")
}

#[tokio::test]
async fn a_gateway_name_is_taken_by_at_most_one_gateway() {
    let router = router();

    // Same name, a different device each time: the name alone is the conflict.
    let cases: Vec<(&str, Value)> = vec![
        ("greengrassV2", greengrass_v2("name-clash-core-device", None)),
        ("siemensIE", siemens_ie("name-clash-siemens-thing")),
        ("greengrass", greengrass("name-clash-group")),
    ];
    let name = "the-one-gateway-name";

    let (status, created) = call(
        &router,
        "POST",
        GATEWAYS,
        Some(json!({
            "gatewayName": name,
            "gatewayPlatform": greengrass_v2("original-core-device", None),
        })),
    )
    .await;
    assert_eq!(status, 201, "the first gateway: expected 201, got {status} {created}");
    let id = created["gatewayId"].as_str().unwrap_or("").to_string();

    for (case, platform) in cases {
        let (status, conflict) = call(
            &router,
            "POST",
            GATEWAYS,
            Some(json!({ "gatewayName": name, "gatewayPlatform": platform })),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a duplicate name expected 409, got {status} {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "ResourceAlreadyExistsException",
            "case {case:?}: unexpected error code in {conflict}"
        );
        assert_eq!(
            conflict["resourceId"].as_str().unwrap_or(""),
            id,
            "case {case:?}: the conflict must name the gateway holding the name, got {conflict}"
        );
    }
}

#[tokio::test]
async fn exactly_one_platform_and_the_v3_operating_system_rule_are_enforced() {
    let router = router();

    // (case, platform, gatewayVersion, accepted)
    let cases: Vec<(&str, Value, Option<&str>, bool)> = vec![
        ("no platform at all", json!({}), None, false),
        (
            "greengrass and greengrassV2",
            json!({
                "greengrass": greengrass("both-group")["greengrass"].clone(),
                "greengrassV2": { "coreDeviceThingName": "both-core-device" },
            }),
            None,
            false,
        ),
        (
            "greengrassV2 and siemensIE",
            json!({
                "greengrassV2": { "coreDeviceThingName": "pair-core-device" },
                "siemensIE": { "iotCoreThingName": "pair-siemens-thing" },
            }),
            None,
            false,
        ),
        (
            "all three platforms",
            json!({
                "greengrass": greengrass("triple-group")["greengrass"].clone(),
                "greengrassV2": { "coreDeviceThingName": "triple-core-device" },
                "siemensIE": { "iotCoreThingName": "triple-siemens-thing" },
            }),
            None,
            false,
        ),
        (
            "v3 greengrassV2 without coreDeviceOperatingSystem",
            greengrass_v2("v3-missing-os", None),
            Some("3"),
            false,
        ),
        // `GatewayVersion` is modelled as a digit string (`^[0-9]+$`), so a padded
        // "03" is the same version 3 and carries the same requirement.
        (
            "v03 greengrassV2 without coreDeviceOperatingSystem",
            greengrass_v2("v03-missing-os", None),
            Some("03"),
            false,
        ),
        (
            "v03 greengrassV2 with coreDeviceOperatingSystem",
            greengrass_v2("v03-with-os", Some("WINDOWS_AMD64")),
            Some("03"),
            true,
        ),
        (
            "v3 greengrassV2 with coreDeviceOperatingSystem",
            greengrass_v2("v3-with-os", Some("LINUX_AARCH64")),
            Some("3"),
            true,
        ),
        // The operating-system rule is stated for the GreengrassV2 platform only, so
        // a version-3 gateway on another platform is accepted without one.
        ("v3 siemensIE", siemens_ie("v3-siemens-thing"), Some("3"), true),
        (
            "v2 greengrassV2 without coreDeviceOperatingSystem",
            greengrass_v2("v2-no-os", None),
            Some("2"),
            true,
        ),
    ];

    for (case, platform, version, accepted) in cases {
        let mut body = json!({ "gatewayName": format!("platform-case-{case}") });
        let object = body.as_object_mut().expect("an object body");
        object.insert("gatewayPlatform".to_string(), platform);
        if let Some(version) = version {
            object.insert("gatewayVersion".to_string(), json!(version));
        }

        let (status, answer) = call(&router, "POST", GATEWAYS, Some(body)).await;
        if accepted {
            assert_eq!(status, 201, "case {case:?}: expected 201, got {status} {answer}");
        } else {
            assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {answer}");
            assert_eq!(
                error_code(&answer),
                "InvalidRequestException",
                "case {case:?}: unexpected error code in {answer}"
            );
        }
    }
}

#[tokio::test]
async fn describing_a_gateway_that_was_never_created_is_refused() {
    let router = router();

    let (status, created) = call(
        &router,
        "POST",
        GATEWAYS,
        Some(json!({
            "gatewayName": "the-only-gateway",
            "gatewayPlatform": greengrass_v2("the-only-core-device", None),
        })),
    )
    .await;
    assert_eq!(status, 201, "the only gateway: expected 201, got {status} {created}");
    let id = created["gatewayId"].as_str().unwrap_or("").to_string();

    // Well-formed ids no create ever minted: one all-f, one that differs from the
    // stored id in a single nibble.
    let mut neighbour: Vec<char> = id.chars().collect();
    let at = neighbour
        .iter()
        .position(|ch| *ch != '-' && *ch != '0')
        .expect("a non-zero character in the id");
    neighbour[at] = '0';
    let neighbour: String = neighbour.into_iter().collect();
    let unknown = ["ffffffff-ffff-ffff-ffff-ffffffffffff", neighbour.as_str()];

    for case in unknown {
        assert!(
            is_modelled_id(case),
            "case {case:?}: the probe id must itself be modelled"
        );
        let (status, body) = call(&router, "GET", &format!("{GATEWAYS}/{case}"), None).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // The refusals left the stored gateway readable.
    let (status, described) = call(&router, "GET", &format!("{GATEWAYS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "the stored gateway after the refusals: got {status} {described}"
    );
}
