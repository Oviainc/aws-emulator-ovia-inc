// SPDX-License-Identifier: CC0-1.0

//! A layer is a REGIONAL resource, and what a function's `Layers` list may name.
//!
//! Three rules are under test, each over a table of inputs the recorded case is one row of.
//!
//! 1. **A layer version lives in the Region it was published into and is addressable only
//!    through that Region's endpoint.** `GetLayerVersion` binds
//!    `GET /2018-10-31/layers/{LayerName}/versions/{VersionNumber}` and models
//!    `ResourceNotFoundException` (client, 404), so a read made in another Region is refused
//!    with it and the canonical sentence AWS gives that error — the layer is not there to
//!    report. The listings report the same thing by not listing it, and a delete made in
//!    another Region removes nothing. The same layer NAME in two Regions is therefore two
//!    layers, each numbering its versions from 1 and holding its own archives. The rows span
//!    the reader (the version's own read, the read by ARN, both listings, the delete), the
//!    Region the call is made in (the layer's own, a second Region holding a layer of the
//!    same name, two Regions holding none) and the version number asked for.
//! 2. **Every entry of a function's `Layers` names a layer version of the FUNCTION's
//!    Region.** `CreateFunction` and `UpdateFunctionConfiguration` both bind `Layers` and
//!    both model `InvalidParameterValueException` (client, 400), and a layer is added to an
//!    execution environment that runs in the function's own Region, so an entry naming
//!    another Region's layer is refused with that error, naming the Region the layers are
//!    expected to be in. The rows span the operation, the foreign Region, the position of
//!    the foreign entry in the list, and the control that the same version IS accepted when
//!    the function is in its Region.
//! 3. **No two entries of a function's `Layers` name versions of ONE layer**, refused with
//!    the same error naming the two conflicting `LayerVersionArn`s as the request submitted
//!    them. The rows span the operation, the order of the two entries, how many other
//!    entries the list carries, and the control that one version of each of two layers is
//!    accepted.
//!
//! A refused call stores nothing and changes nothing: every row reads the function's
//! configuration back through `GetFunctionConfiguration` afterwards, so a refusal that had
//! already mutated the record fails here.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Map, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region most of this suite's calls are made in.
const EAST: &str = "us-east-1";

/// A second Region, which holds layers of its own under the same names.
const WEST: &str = "us-west-2";

/// A third Region, which holds no layer at all.
const EU: &str = "eu-west-1";

/// A fourth Region, which holds no layer either — a second row for "holds none", so the
/// rule is not read off one Region's spelling.
const APAC: &str = "ap-south-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The sentence AWS gives `ResourceNotFoundException` for a layer version it does not
/// hold, which `crate::state::RESOURCE_NOT_FOUND_MESSAGE` states once for this service.
const NOT_FOUND_MESSAGE: &str = "The resource you requested does not exist.";

/// How long past a due instant this suite waits before calling. The move from `Pending` to
/// `Active` is owed by the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The response body as JSON, or `Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The response body as text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The wire error code (`__type`) of an error response.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map_or_else(|| self.text(), str::to_string)
    }

    /// The `message` of an error response.
    fn error_message(&self) -> String {
        let body = self.json();
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .map_or_else(|| self.text(), str::to_string)
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request to `region`'s endpoint, signed for that Region's scope.
/// Nothing in the signature is verified: the harness turns request authentication off, and
/// the scope is read only for the Region the caller addressed.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: &Value,
) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let host = format!("lambda.{region}.amazonaws.com");
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{region}/lambda/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    );
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{uri}"))
        .header("host", host)
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// Percent-encode the characters an ARN carries that a query string would otherwise read
/// as structure.
fn encoded(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
        .replace('&', "%26")
        .replace('=', "%3D")
}

/// Layer archive bytes of a given length. Only the length and the digest are reported, but
/// they differ per length, so two archives never look alike.
fn archive_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The `CodeSize` an archive of these bytes must be reported with.
fn archive_size(bytes: &[u8]) -> i64 {
    i64::try_from(bytes.len()).unwrap_or_default()
}

/// `PublishLayerVersion` in `region`, asserting the version number it reports.
async fn publish_layer(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    archive: &[u8],
    expected_version: i64,
) {
    let answer = call_in(
        router,
        region,
        "POST",
        &format!("/2018-10-31/layers/{name}/versions"),
        &json!({ "Content": { "ZipFile": zip_file(archive) } }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?} in {region}: expected 201 from PublishLayerVersion, got {} {}",
        answer.status,
        answer.text()
    );
    let version = answer.json().get("Version").and_then(Value::as_i64);
    assert_eq!(
        version,
        Some(expected_version),
        "case {name:?} in {region}: expected version {expected_version}, got {version:?}"
    );
}

/// `GetLayerVersion` in `region`.
async fn get_layer_version(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    version: i64,
) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}");
    call_in(router, region, "GET", &uri, &Value::Null).await
}

/// `GetLayerVersionByArn` in `region`, whose `Arn` member is bound to the `Arn` query
/// parameter of `GET /2018-10-31/layers?find=LayerVersion`.
async fn get_layer_version_by_arn(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
) -> Answer {
    let uri = format!("/2018-10-31/layers?find=LayerVersion&Arn={}", encoded(arn));
    call_in(router, region, "GET", &uri, &Value::Null).await
}

/// The version numbers `ListLayerVersions` reports in `region` for one layer name.
async fn listed_versions(router: &MultiServiceRouter, region: &str, name: &str) -> Vec<i64> {
    let answer = call_in(
        router,
        region,
        "GET",
        &format!("/2018-10-31/layers/{name}/versions"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {name:?} in {region}: expected 200 from ListLayerVersions, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .json()
        .get("LayerVersions")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("Version").and_then(Value::as_i64))
                .collect()
        })
        .unwrap_or_default()
}

/// The layer names `ListLayers` reports in `region`.
async fn listed_layers(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let answer = call_in(router, region, "GET", "/2018-10-31/layers", &Value::Null).await;
    assert_eq!(
        answer.status,
        200,
        "case {region}: expected 200 from ListLayers, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .json()
        .get("Layers")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("LayerName").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The ARN version `version` of the layer `name` of `region` carries.
fn layer_version_arn(region: &str, name: &str, version: i64) -> String {
    format!("arn:aws:lambda:{region}:{ACCOUNT}:layer:{name}:{version}")
}

/// The unqualified ARN a function of `name` in `region` carries.
fn function_arn(region: &str, name: &str) -> String {
    format!("arn:aws:lambda:{region}:{ACCOUNT}:function:{name}")
}

/// `CreateFunction` in `region`, with the `Layers` list the row names.
async fn create_function_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    layers: Option<&[String]>,
) -> Answer {
    let mut body = Map::new();
    body.insert("FunctionName".to_string(), json!(name));
    body.insert("Role".to_string(), json!(ROLE));
    body.insert("Runtime".to_string(), json!("python3.12"));
    body.insert("Handler".to_string(), json!("index.handler"));
    body.insert(
        "Code".to_string(),
        json!({ "ZipFile": zip_file(&archive_bytes(64)) }),
    );
    if let Some(layers) = layers {
        body.insert("Layers".to_string(), json!(layers));
    }
    call_in(router, region, "POST", FUNCTIONS_URI, &Value::Object(body)).await
}

/// `UpdateFunctionConfiguration` in `region`, with the `Layers` list the row names.
///
/// "When the code is Creating, you can't invoke or modify the function", so this waits past
/// the instant the create made the move to `Active` due before it modifies anything: the
/// answer under test is the one about the LAYERS, not about a function still being
/// provisioned.
async fn update_layers_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    layers: &[String],
) -> Answer {
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
    call_in(
        router,
        region,
        "PUT",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        &json!({ "Layers": layers }),
    )
    .await
}

/// The `Arn` of every layer `GetFunctionConfiguration` reports for one function, which is
/// what a refusal must have left alone.
async fn attached_layers(router: &MultiServiceRouter, region: &str, name: &str) -> Vec<String> {
    let answer = call_in(
        router,
        region,
        "GET",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {name:?} in {region}: expected 200 from GetFunctionConfiguration, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .json()
        .get("Layers")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("Arn").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// RULE 1: a layer version is addressable only through the Region it was published into,
/// and the same name in another Region is another layer.
#[tokio::test]
async fn a_layer_version_is_reachable_only_in_the_region_it_was_published_into() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // `probe` holds two versions in EAST and one — with an archive of its own — in WEST,
    // so the two records cannot be mistaken for one.
    let east_first = archive_bytes(512);
    let east_second = archive_bytes(1_024);
    let west_first = archive_bytes(777);
    publish_layer(&router, EAST, "probe", &east_first, 1).await;
    publish_layer(&router, EAST, "probe", &east_second, 2).await;
    publish_layer(&router, WEST, "probe", &west_first, 1).await;

    // One row per (Region, version asked for): the versions each Region holds, and the ones
    // it does not — including the version number the OTHER Region holds.
    let reads: &[(&str, i64, Option<i64>)] = &[
        (EAST, 1, Some(archive_size(&east_first))),
        (EAST, 2, Some(archive_size(&east_second))),
        (EAST, 3, None),
        (WEST, 1, Some(archive_size(&west_first))),
        // EAST's version 2 is not WEST's to report: numbering is per layer, so WEST's layer
        // has no version 2 at all.
        (WEST, 2, None),
        (EU, 1, None),
        (EU, 2, None),
        (APAC, 1, None),
    ];
    for (region, version, expected_size) in reads {
        let read = get_layer_version(&router, region, "probe", *version).await;
        let case = format!("GetLayerVersion probe:{version} in {region}");
        match expected_size {
            Some(size) => {
                assert_eq!(
                    read.status,
                    200,
                    "case {case:?}: expected 200, got {} {}",
                    read.status,
                    read.text()
                );
                let reported = read
                    .json()
                    .get("Content")
                    .and_then(|content| content.get("CodeSize"))
                    .and_then(Value::as_i64);
                assert_eq!(
                    reported,
                    Some(*size),
                    "case {case:?}: expected CodeSize {size}, got {reported:?}"
                );
                let arn = read
                    .json()
                    .get("LayerVersionArn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert_eq!(
                    arn,
                    layer_version_arn(region, "probe", *version),
                    "case {case:?}: expected the ARN of the reading Region, got {arn:?}"
                );
            }
            None => {
                assert_eq!(
                    read.status,
                    404,
                    "case {case:?}: expected 404, got {} {}",
                    read.status,
                    read.text()
                );
                let code = read.error_code();
                assert!(
                    code.contains("ResourceNotFoundException"),
                    "case {case:?}: expected the ResourceNotFoundException error code, got \
                     {code:?}"
                );
                let message = read.error_message();
                assert_eq!(
                    message, NOT_FOUND_MESSAGE,
                    "case {case:?}: expected the message {NOT_FOUND_MESSAGE:?}, got {message:?}"
                );
            }
        }
    }

    // The read BY ARN resolves the same way: the ARN names its Region, and only a call made
    // in that Region reaches it.
    let by_arn: &[(&str, &str, u16)] = &[
        (EAST, EAST, 200),
        (EAST, WEST, 404),
        (EAST, EU, 404),
        (WEST, WEST, 200),
        (WEST, EAST, 404),
        (WEST, APAC, 404),
    ];
    for (arn_region, call_region, expected) in by_arn {
        let arn = layer_version_arn(arn_region, "probe", 1);
        let read = get_layer_version_by_arn(&router, call_region, &arn).await;
        assert_eq!(
            read.status,
            *expected,
            "case {arn:?} read in {call_region}: expected {expected}, got {} {}",
            read.status,
            read.text()
        );
    }

    // The listings report what each Region holds and nothing else.
    let listings: &[(&str, Vec<i64>, Vec<String>)] = &[
        (EAST, vec![2, 1], vec!["probe".to_string()]),
        (WEST, vec![1], vec!["probe".to_string()]),
        (EU, Vec::new(), Vec::new()),
        (APAC, Vec::new(), Vec::new()),
    ];
    for (region, expected_versions, expected_layers) in listings {
        let versions = listed_versions(&router, region, "probe").await;
        assert_eq!(
            &versions, expected_versions,
            "case {region}: expected ListLayerVersions {expected_versions:?}, got {versions:?}"
        );
        let layers = listed_layers(&router, region).await;
        assert_eq!(
            &layers, expected_layers,
            "case {region}: expected ListLayers {expected_layers:?}, got {layers:?}"
        );
    }

    // A delete made in another Region removes nothing: it is idempotent over a version that
    // Region does not hold, and the version the owning Region holds survives it.
    for region in [EU, WEST] {
        let deleted = call_in(
            &router,
            region,
            "DELETE",
            "/2018-10-31/layers/probe/versions/2",
            &Value::Null,
        )
        .await;
        assert_eq!(
            deleted.status,
            204,
            "case delete probe:2 in {region}: expected 204, got {} {}",
            deleted.status,
            deleted.text()
        );
        let read = get_layer_version(&router, EAST, "probe", 2).await;
        assert_eq!(
            read.status,
            200,
            "case delete probe:2 in {region}: expected EAST's version 2 to survive, got {} {}",
            read.status,
            read.text()
        );
    }
}

/// RULE 2: every entry of a function's `Layers` names a layer version of the function's own
/// Region.
#[tokio::test]
async fn a_functions_layers_must_name_layer_versions_of_its_own_region() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // The same two layer names published in three Regions, so every row names a version
    // that EXISTS — in the wrong Region.
    for region in [EAST, WEST, EU] {
        publish_layer(&router, region, "helpers", &archive_bytes(256), 1).await;
        publish_layer(&router, region, "extras", &archive_bytes(384), 1).await;
    }
    let local = layer_version_arn(EAST, "helpers", 1);
    let local_extra = layer_version_arn(EAST, "extras", 1);
    let expected_message = format!(
        "Layers are not in the same region as the function. Layers are expected to be in \
         region {EAST}."
    );

    // The rows span the foreign Region and WHERE the foreign entry sits in the list; the
    // last row is the control, where every entry is this Region's.
    let rows: &[(&str, Vec<String>, bool)] = &[
        (
            "foreign-alone",
            vec![layer_version_arn(WEST, "helpers", 1)],
            false,
        ),
        (
            "foreign-after-local",
            vec![local.clone(), layer_version_arn(EU, "extras", 1)],
            false,
        ),
        (
            "foreign-before-local",
            vec![layer_version_arn(EU, "helpers", 1), local_extra.clone()],
            false,
        ),
        (
            "every-entry-foreign",
            vec![
                layer_version_arn(WEST, "helpers", 1),
                layer_version_arn(WEST, "extras", 1),
            ],
            false,
        ),
        (
            // A layer Lambda itself provides belongs to no Region, so it is no entry of
            // another Region's.
            "aws-managed-beside-local",
            vec![
                "arn:aws:lambda:::awslayer:AmazonLinux1803".to_string(),
                local.clone(),
            ],
            true,
        ),
        (
            "every-entry-local",
            vec![local.clone(), local_extra.clone()],
            true,
        ),
    ];

    for (case, layers, accepted) in rows {
        // On the CREATE: a refused create stores no function at all.
        let name = format!("created-{case}");
        let created = create_function_in(&router, EAST, &name, Some(layers)).await;
        if *accepted {
            assert_eq!(
                created.status,
                201,
                "case {case:?} on CreateFunction: expected 201, got {} {}",
                created.status,
                created.text()
            );
            let attached = attached_layers(&router, EAST, &name).await;
            assert_eq!(
                &attached, layers,
                "case {case:?} on CreateFunction: expected the layers {layers:?}, got \
                 {attached:?}"
            );
        } else {
            assert_eq!(
                created.status,
                400,
                "case {case:?} on CreateFunction: expected 400, got {} {}",
                created.status,
                created.text()
            );
            let code = created.error_code();
            assert!(
                code.contains("InvalidParameterValueException"),
                "case {case:?} on CreateFunction: expected the \
                 InvalidParameterValueException error code, got {code:?}"
            );
            let message = created.error_message();
            assert_eq!(
                message, expected_message,
                "case {case:?} on CreateFunction: expected the message \
                 {expected_message:?}, got {message:?}"
            );
            let read = call_in(
                &router,
                EAST,
                "GET",
                &format!("{FUNCTIONS_URI}/{name}/configuration"),
                &Value::Null,
            )
            .await;
            assert_eq!(
                read.status,
                404,
                "case {case:?} on CreateFunction: expected the refused create to store no \
                 function, got {} {}",
                read.status,
                read.text()
            );
        }

        // And on the UPDATE, against a function that already holds one of this Region's
        // layers: a refusal leaves that list exactly as it was.
        let updated_name = format!("updated-{case}");
        let created =
            create_function_in(&router, EAST, &updated_name, Some(std::slice::from_ref(&local)))
                .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: the function under update must be created, got {} {}",
            created.status,
            created.text()
        );
        let answer = update_layers_in(&router, EAST, &updated_name, layers).await;
        let expected_layers: Vec<String> = if *accepted {
            layers.clone()
        } else {
            vec![local.clone()]
        };
        if *accepted {
            assert_eq!(
                answer.status,
                200,
                "case {case:?} on UpdateFunctionConfiguration: expected 200, got {} {}",
                answer.status,
                answer.text()
            );
        } else {
            assert_eq!(
                answer.status,
                400,
                "case {case:?} on UpdateFunctionConfiguration: expected 400, got {} {}",
                answer.status,
                answer.text()
            );
            let message = answer.error_message();
            assert_eq!(
                message, expected_message,
                "case {case:?} on UpdateFunctionConfiguration: expected the message \
                 {expected_message:?}, got {message:?}"
            );
        }
        let attached = attached_layers(&router, EAST, &updated_name).await;
        assert_eq!(
            attached, expected_layers,
            "case {case:?} on UpdateFunctionConfiguration: expected the layers \
             {expected_layers:?}, got {attached:?}"
        );
    }

    // The control that the rule is about the REGION and not about the ARNs: the very
    // version WEST's rows were refused for is accepted by a function of WEST.
    let west_layer = layer_version_arn(WEST, "helpers", 1);
    let created = create_function_in(
        &router,
        WEST,
        "at-home",
        Some(std::slice::from_ref(&west_layer)),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case \"at-home\": expected 201 from CreateFunction in {WEST}, got {} {}",
        created.status,
        created.text()
    );
    let attached = attached_layers(&router, WEST, "at-home").await;
    assert_eq!(
        attached,
        vec![west_layer.clone()],
        "case \"at-home\": expected the layer {west_layer:?}, got {attached:?}"
    );
}

/// RULE 3: no two entries of a function's `Layers` name versions of one layer.
#[tokio::test]
async fn a_functions_layers_may_not_name_two_versions_of_one_layer() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for version in 1..=3 {
        publish_layer(
            &router,
            EAST,
            "stack",
            &archive_bytes(128 * usize::try_from(version).unwrap_or(1)),
            version,
        )
        .await;
    }
    publish_layer(&router, EAST, "solo", &archive_bytes(96), 1).await;
    let first = layer_version_arn(EAST, "stack", 1);
    let second = layer_version_arn(EAST, "stack", 2);
    let third = layer_version_arn(EAST, "stack", 3);
    let other = layer_version_arn(EAST, "solo", 1);

    // The rows span the order of the two conflicting entries, how many other entries the
    // list carries, and the repeat of ONE entry; the last two rows are the controls.
    //
    // A row is its case name, the `Layers` list it submits, and the two entries the refusal
    // must name — `None` where the list is admissible.
    type Row = (&'static str, Vec<String>, Option<(String, String)>);
    let rows: &[Row] = &[
        (
            "ascending",
            vec![first.clone(), second.clone()],
            Some((first.clone(), second.clone())),
        ),
        (
            "descending",
            vec![second.clone(), first.clone()],
            Some((second.clone(), first.clone())),
        ),
        (
            "around-another-layer",
            vec![first.clone(), other.clone(), third.clone()],
            Some((first.clone(), third.clone())),
        ),
        (
            "same-entry-twice",
            vec![third.clone(), third.clone()],
            Some((third.clone(), third.clone())),
        ),
        (
            "one-version-of-each",
            vec![other.clone(), second.clone()],
            None,
        ),
        ("a-single-entry", vec![third.clone()], None),
    ];

    for (case, layers, conflict) in rows {
        let expected_message = conflict.as_ref().map(|(first, second)| {
            format!(
                "Two different versions of the same layer are not allowed to be referenced \
                 in the same function. {first} and {second} are versions of the same layer."
            )
        });

        let name = format!("created-{case}");
        let created = create_function_in(&router, EAST, &name, Some(layers)).await;
        match &expected_message {
            None => {
                assert_eq!(
                    created.status,
                    201,
                    "case {case:?} on CreateFunction: expected 201, got {} {}",
                    created.status,
                    created.text()
                );
                let attached = attached_layers(&router, EAST, &name).await;
                assert_eq!(
                    &attached, layers,
                    "case {case:?} on CreateFunction: expected the layers {layers:?}, got \
                     {attached:?}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    created.status,
                    400,
                    "case {case:?} on CreateFunction: expected 400, got {} {}",
                    created.status,
                    created.text()
                );
                let code = created.error_code();
                assert!(
                    code.contains("InvalidParameterValueException"),
                    "case {case:?} on CreateFunction: expected the \
                     InvalidParameterValueException error code, got {code:?}"
                );
                let message = created.error_message();
                assert_eq!(
                    &message, expected,
                    "case {case:?} on CreateFunction: expected the message {expected:?}, got \
                     {message:?}"
                );
            }
        }

        let updated_name = format!("updated-{case}");
        let created =
            create_function_in(&router, EAST, &updated_name, Some(std::slice::from_ref(&other)))
                .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: the function under update must be created, got {} {}",
            created.status,
            created.text()
        );
        let answer = update_layers_in(&router, EAST, &updated_name, layers).await;
        match &expected_message {
            None => {
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?} on UpdateFunctionConfiguration: expected 200, got {} {}",
                    answer.status,
                    answer.text()
                );
                let attached = attached_layers(&router, EAST, &updated_name).await;
                assert_eq!(
                    &attached, layers,
                    "case {case:?} on UpdateFunctionConfiguration: expected the layers \
                     {layers:?}, got {attached:?}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    answer.status,
                    400,
                    "case {case:?} on UpdateFunctionConfiguration: expected 400, got {} {}",
                    answer.status,
                    answer.text()
                );
                let message = answer.error_message();
                assert_eq!(
                    &message, expected,
                    "case {case:?} on UpdateFunctionConfiguration: expected the message \
                     {expected:?}, got {message:?}"
                );
                let attached = attached_layers(&router, EAST, &updated_name).await;
                assert_eq!(
                    attached,
                    vec![other.clone()],
                    "case {case:?} on UpdateFunctionConfiguration: expected the refusal to \
                     leave the layer {other:?} attached, got {attached:?}"
                );
            }
        }
    }

    // The two layers of one Region are told apart by their NAME, so the refusal above is
    // not about two ARNs that merely look alike: a function ARN of the same Region is still
    // the function the rows updated.
    let arn = function_arn(EAST, "updated-ascending");
    let read = call_in(
        &router,
        EAST,
        "GET",
        &format!("{FUNCTIONS_URI}/{arn}/configuration"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        read.status,
        200,
        "expected the refused function to be readable by its ARN, got {} {}",
        read.status,
        read.text()
    );
}
