// SPDX-License-Identifier: CC0-1.0

//! `DescribePackagingConfiguration`: the read-back of a stored PackagingConfiguration.
//!
//! Every case drives real HTTP through the router the production binary builds, so
//! what is asserted is the status and JSON body a `mediapackage-vod` client sees —
//! never a handler called directly. Each test asserts a RULE over a TABLE of inputs;
//! one recorded packaging format is one row.
//!
//! The rules under test:
//!
//! * a describe of an `Id` this service holds a PackagingConfiguration for answers
//!   200 with the record that was persisted — its ARN, `CreatedAt`,
//!   `PackagingGroupId`, tags, and the ONE package block it was created with,
//!   verbatim, whichever of the four modelled formats that is, with the other three
//!   package members absent;
//! * the read is reporting only: repeating it answers the same body, and it changes
//!   nothing a later caller sees (a refused duplicate create still reads back as the
//!   first stored record);
//! * an `Id` this service holds no PackagingConfiguration for answers 404
//!   `NotFoundException` whose message binds the requested id in the crate's one
//!   form, and an id that names a resource of a DIFFERENT type is such an id.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::packaging_lifecycle::assert_projection;

/// The region the test signs for. It appears in the ARN the read reports, so it is
/// asserted rather than assumed.
const REGION: &str = "us-west-2";

/// The wire error code of a missing addressed resource, and its modelled `httpError`
/// (`smithy_query(query="operation",operation="DescribePackagingConfiguration")`:
/// `NotFoundException`, `errorKind client`, `httpError 404`).
const NOT_FOUND: (&str, u16) = ("NotFoundException", 404);

/// Every package member `DescribePackagingConfiguration` can report, from the
/// operation's modelled output (`CmafPackage`, `DashPackage`, `HlsPackage`,
/// `MssPackage` — their `jsonName`s).
const PACKAGE_MEMBERS: [&str; 4] = ["cmafPackage", "dashPackage", "hlsPackage", "mssPackage"];

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// A router carrying exactly this service over one shared engine — the same assembly
/// `core/router` performs, minus the other services.
///
/// This suite reads statuses, bodies and the `x-amzn-errortype` header, so it
/// carries its own small harness like the crate's other suites.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mediapackagevod_mediapackagevod::EmulatedService::new(Arc::new(
            engine::InMemoryEngine::new(),
        )),
    );
    router
}

/// Send one request and return its status, its `x-amzn-errortype` and its parsed JSON
/// body (`Null` when empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    let host = format!("mediapackage-vod.{REGION}.amazonaws.com");
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone())
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}\
                 /mediapackage-vod/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| "<unset>".to_string());
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, errortype, parsed)
}

/// Create a packaging group, asserting it succeeded.
async fn create_group(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, _, group) = call(router, "POST", "/packaging_groups", Some(json!({ "id": id }))).await;
    assert_eq!(status, 200, "CreatePackagingGroup {id}: body {group}");
    group
}

/// Create a packaging configuration carrying `package` (one package member) and
/// `tags`, asserting it succeeded, and return the create's own response.
async fn create_config(
    router: &MultiServiceRouter,
    id: &str,
    group_id: &str,
    package: &Value,
    tags: Option<&Value>,
) -> Value {
    let mut body = json!({ "id": id, "packagingGroupId": group_id });
    let Value::Object(members) = package else {
        panic!("the package block must be a JSON object");
    };
    for (key, value) in members {
        body[key] = value.clone();
    }
    if let Some(tags) = tags {
        body["tags"] = tags.clone();
    }
    let (status, _, config) =
        call(router, "POST", "/packaging_configurations", Some(body)).await;
    // 202 Accepted: the status the recorded wire responses show for a successful
    // create, which `src/ops/create_packaging_configuration.rs` explains (the model
    // declares 200).
    assert_eq!(status, 202, "CreatePackagingConfiguration {id}: body {config}");
    config
}

/// Read a packaging configuration back, returning the status, the
/// `x-amzn-errortype` and the parsed body the call answered.
async fn describe_config(router: &MultiServiceRouter, id: &str) -> (u16, String, Value) {
    call(router, "GET", &format!("/packaging_configurations/{id}"), None).await
}

/// One row of the round-trip table: a case name, the package member it carries, the
/// package block itself and the tags the configuration is created with.
struct Row {
    case: &'static str,
    member: &'static str,
    package: Value,
    tags: Option<Value>,
}

/// The package blocks the round-trip table spans.
///
/// One row per modelled packaging format, and rows differing in manifest count,
/// manifest name (named vs defaulted) and which optional members the block carries.
/// Enum values come from the model: `AdMarkers` (NONE | SCTE35_ENHANCED |
/// PASSTHROUGH), `StreamOrder` (ORIGINAL | VIDEO_BITRATE_ASCENDING |
/// VIDEO_BITRATE_DESCENDING), `Profile` (NONE | HBBTV_1_5), `SegmentTemplateFormat`
/// (NUMBER_WITH_TIMELINE | TIME_WITH_TIMELINE | NUMBER_WITH_DURATION),
/// `__PeriodTriggersElement` (ADS) and `ManifestLayout` (FULL | COMPACT), via
/// `smithy_query(query="shape",shape="AdMarkers,StreamOrder,Profile,\
/// SegmentTemplateFormat,__PeriodTriggersElement,ManifestLayout")`.
fn round_trip_rows() -> Vec<Row> {
    vec![
        Row {
            case: "hls, one manifest, no optional members, no tags",
            member: "hlsPackage",
            package: json!({ "hlsPackage": { "hlsManifests": [{}] } }),
            tags: None,
        },
        Row {
            case: "hls, two manifests, every optional member, two tags",
            member: "hlsPackage",
            package: json!({
                "hlsPackage": {
                    "hlsManifests": [
                        {
                            "manifestName": "primary",
                            "adMarkers": "SCTE35_ENHANCED",
                            "includeIframeOnlyStream": true,
                            "programDateTimeIntervalSeconds": 60,
                            "repeatExtXKey": true,
                            "streamSelection": {
                                "minVideoBitsPerSecond": 100_000,
                                "maxVideoBitsPerSecond": 5_000_000,
                                "streamOrder": "VIDEO_BITRATE_DESCENDING"
                            }
                        },
                        { "manifestName": "backup", "adMarkers": "NONE" }
                    ],
                    "includeDvbSubtitles": true,
                    "segmentDurationSeconds": 6,
                    "useAudioRenditionGroup": false
                }
            }),
            tags: Some(json!({ "env": "prod", "team": "vod" })),
        },
        Row {
            case: "dash, named manifest, period triggers, one tag",
            member: "dashPackage",
            package: json!({
                "dashPackage": {
                    "dashManifests": [{
                        "manifestName": "wide",
                        "manifestLayout": "COMPACT",
                        "minBufferTimeSeconds": 30,
                        "profile": "HBBTV_1_5"
                    }],
                    "includeEncoderConfigurationInSegments": true,
                    "includeIframeOnlyStream": false,
                    "periodTriggers": ["ADS"],
                    "segmentDurationSeconds": 2,
                    "segmentTemplateFormat": "TIME_WITH_TIMELINE"
                }
            }),
            tags: Some(json!({ "env": "staging" })),
        },
        Row {
            case: "cmaf, named manifest, empty tag map",
            member: "cmafPackage",
            package: json!({
                "cmafPackage": {
                    "hlsManifests": [{ "manifestName": "cmaf", "adMarkers": "PASSTHROUGH" }],
                    "includeEncoderConfigurationInSegments": false,
                    "segmentDurationSeconds": 4
                }
            }),
            tags: Some(json!({})),
        },
        Row {
            case: "mss, two manifests, stream selection",
            member: "mssPackage",
            package: json!({
                "mssPackage": {
                    "mssManifests": [
                        {
                            "manifestName": "smooth",
                            "streamSelection": { "streamOrder": "ORIGINAL" }
                        },
                        { "manifestName": "smooth-low" }
                    ],
                    "segmentDurationSeconds": 3
                }
            }),
            tags: None,
        },
    ]
}

/// The stored record a describe reports is the one the create persisted, for every
/// modelled packaging format — and only the package member it was created with.
#[tokio::test]
async fn describe_reports_the_stored_packaging_configuration() {
    for row in round_trip_rows() {
        let Row {
            case,
            member,
            package,
            tags,
        } = row;
        let router = router();
        create_group(&router, "vod-group").await;
        let created = create_config(&router, "cfg", "vod-group", &package, tags.as_ref()).await;

        let (status, errortype, described) = describe_config(&router, "cfg").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} ({errortype}) body {described}"
        );

        // Identity: the id addressed, the group it packages for, and the computed ARN.
        assert_eq!(
            described["id"].as_str(),
            Some("cfg"),
            "case {case:?}: expected the addressed id, got {}",
            described["id"]
        );
        assert_eq!(
            described["packagingGroupId"].as_str(),
            Some("vod-group"),
            "case {case:?}: expected the stored packagingGroupId, got {}",
            described["packagingGroupId"]
        );
        let expected_arn = format!(
            "arn:aws:mediapackage-vod:{REGION}:{}:packaging-configurations/cfg",
            account_id()
        );
        assert_eq!(
            described["arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected the computed ARN, got {}",
            described["arn"]
        );

        // Every member the create reported is reported identically by the read: the
        // read answers from the stored record, not from the request it is answering.
        for reported in [
            "arn",
            "createdAt",
            "id",
            "packagingGroupId",
            "tags",
            "cmafPackage",
            "dashPackage",
            "hlsPackage",
            "mssPackage",
        ] {
            assert_eq!(
                described[reported], created[reported],
                "case {case:?}: {reported} must be what CreatePackagingConfiguration \
                 stored ({}), got {}",
                created[reported], described[reported]
            );
        }

        // The package block the create request carried is persisted: every member it
        // stated reads back with the same value. The service also MATERIALISES the
        // defaults of the members the request left unset (see
        // `success_path_projections::create_packaging_configuration_materialises_the_server_side_defaults`),
        // so the stored block is a superset of the requested one rather than a copy of
        // it — hence the shared SUBSET comparison.
        assert_projection(
            case,
            &format!("the stored {member}"),
            &package[member],
            &described[member],
        );
        for absent in PACKAGE_MEMBERS.iter().filter(|name| **name != member) {
            assert!(
                described.get(absent).is_none_or(Value::is_null),
                "case {case:?}: {absent} must be absent on a {member} configuration, got {}",
                described[*absent]
            );
        }

        // Tags are reported from the one tag store; an absent `Tags` and an empty map
        // are the same request, so both read back as an empty map.
        let expected_tags = tags.clone().unwrap_or_else(|| json!({}));
        assert_eq!(
            described["tags"], expected_tags,
            "case {case:?}: expected the stored tags {expected_tags}, got {}",
            described["tags"]
        );
        assert!(
            !described["createdAt"].as_str().unwrap_or_default().is_empty(),
            "case {case:?}: expected a CreatedAt, got {}",
            described["createdAt"]
        );
    }
}

/// The describe is reporting, not mutating: reading repeatedly answers the same body,
/// reading one configuration does not disturb its siblings, and a create refused for
/// re-using an id leaves the first stored record readable unchanged.
#[tokio::test]
async fn describe_changes_nothing_a_later_caller_sees() {
    let router = router();
    create_group(&router, "vod-group").await;

    let rows = round_trip_rows();
    // One configuration per row, all in one group, so a read of each is also a read
    // in the presence of the others.
    let ids: Vec<String> = (0..rows.len()).map(|index| format!("cfg-{index}")).collect();
    for (id, row) in ids.iter().zip(rows.iter()) {
        create_config(&router, id, "vod-group", &row.package, row.tags.as_ref()).await;
    }

    let mut first_reads = Vec::new();
    for id in &ids {
        let (status, _, body) = describe_config(&router, id).await;
        assert_eq!(status, 200, "DescribePackagingConfiguration {id}: body {body}");
        first_reads.push(body);
    }

    // Read every configuration again, in the reverse order, after each has been read
    // once: a read that consumed or advanced anything would show up here.
    for (id, first) in ids.iter().zip(first_reads.iter()).rev() {
        let (status, _, again) = describe_config(&router, id).await;
        assert_eq!(status, 200, "re-read of {id}: body {again}");
        assert_eq!(
            &again, first,
            "case {id:?}: a repeated read must answer the same body as the first \
             ({first}), got {again}"
        );
    }

    // A duplicate create is refused as a parameter violation, and the record the
    // first create stored is still the record that reads back.
    let first_row = &round_trip_rows()[0];
    let other_row = &round_trip_rows()[2];
    let mut duplicate = json!({ "id": ids[0].clone(), "packagingGroupId": "vod-group" });
    let Value::Object(members) = &other_row.package else {
        panic!("the package block must be a JSON object");
    };
    for (key, value) in members {
        duplicate[key] = value.clone();
    }
    let (status, _, refusal) =
        call(&router, "POST", "/packaging_configurations", Some(duplicate)).await;
    assert_eq!(
        status, 422,
        "a create re-using a stored id must be refused, got {status} body {refusal}"
    );
    let (status, _, after) = describe_config(&router, &ids[0]).await;
    assert_eq!(status, 200, "re-read after a refused duplicate: body {after}");
    assert_eq!(
        &after, &first_reads[0],
        "a refused duplicate create must not change the stored record: expected {}, got {after}",
        first_reads[0]
    );
    assert_projection(
        "a refused duplicate create",
        &format!("the stored {}", first_row.member),
        &first_row.package[first_row.member],
        &after[first_row.member],
    );
    assert!(
        after.get(other_row.member).is_none_or(Value::is_null),
        "a refused duplicate create must not add its own package block, got {}",
        after[other_row.member]
    );
}

/// An `Id` this service holds no PackagingConfiguration for is a missing ADDRESSED
/// resource: 404 `NotFoundException`, with the message bound to the id the REQUEST
/// asked for. An id that names a resource of another type is such an id — the three
/// resource types are addressed in separate namespaces.
#[tokio::test]
async fn describe_of_an_unknown_id_is_not_found() {
    let router = router();
    create_group(&router, "shared-name").await;
    create_config(
        &router,
        "stored-cfg",
        "shared-name",
        &json!({ "hlsPackage": { "hlsManifests": [{}] } }),
        None,
    )
    .await;
    let (status, _, asset) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({
            "id": "an-asset",
            "packagingGroupId": "shared-name",
            "sourceArn": "arn:aws:s3:::test-bucket/an-asset.smil",
            "sourceRoleArn": format!("arn:aws:iam::{}:role/MediaPackageVod", account_id()),
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateAsset: body {asset}");

    // (case, the id to address)
    let cases: Vec<(&str, &str)> = vec![
        ("an id nothing was ever created with", "no-such-cfg"),
        ("the id of a PackagingGroup", "shared-name"),
        ("the id of an Asset", "an-asset"),
        ("a stored id in the wrong case", "STORED-CFG"),
        ("a stored id with a suffix", "stored-cfg-2"),
    ];

    for (case, id) in cases {
        let (status, errortype, body) = describe_config(&router, id).await;
        assert_eq!(
            status, NOT_FOUND.1,
            "case {case:?} ({id}): expected {}, got {status} body {body}",
            NOT_FOUND.1
        );
        assert_eq!(
            errortype, NOT_FOUND.0,
            "case {case:?} ({id}): expected the modelled error code, got {errortype:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            // The recorded AWS message for a missing PackagingConfiguration names the
            // resource type by its ABBREVIATED label: behavioral unit
            // `mediapackage-vod.delete-packaging-configuration-state-transition`
            // asserts `PackagingConfig with id=<id> not found`, so that is the label
            // this service words the message with.
            Some(format!("PackagingConfig with id={id} not found").as_str()),
            "case {case:?} ({id}): expected the id bound into the message, got {}",
            body["message"]
        );
        // A missing addressed resource is not a parameter-validation refusal, so it
        // carries no per-attribute `errors` map.
        assert!(
            body.get("errors").is_none_or(Value::is_null),
            "case {case:?} ({id}): a NotFoundException carries no errors map, got {}",
            body["errors"]
        );
    }

    // None of that is a blanket answer: the stored id still reads back.
    let (status, _, stored) = describe_config(&router, "stored-cfg").await;
    assert_eq!(status, 200, "the stored configuration must still read back: {stored}");
    assert_eq!(
        stored["id"].as_str(),
        Some("stored-cfg"),
        "expected the stored id, got {}",
        stored["id"]
    );
}
