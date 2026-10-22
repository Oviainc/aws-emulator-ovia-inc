// SPDX-License-Identifier: CC0-1.0

//! The one request harness every suite in this crate drives the emulator through.
//!
//! A test here is an END-TO-END request: the router picks the service from the SigV4
//! credential scope, the framework deserializes and validates the body, the handler
//! runs, and the response is serialized as `restJson1` — so what a suite asserts is
//! what a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::TRANSITION_SPAN;
use service_com_amazonaws_elementalinference_elementalinference::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region every request below is signed for. The service reads it out of the SigV4
/// credential scope, so the ARNs it mints must carry exactly this region.
pub const REGION: &str = "us-west-2";

/// This service's SigV4 signing name (`OPERATIONS.md`, "Wire protocol"): the router
/// resolves the claiming service from it.
pub const SIGNING_NAME: &str = "elemental-inference";

/// Long enough for a clock-owed feed transition to have fallen due.
pub fn past_the_transition_span() {
    // A plain blocking sleep: the test's runtime has nothing else to run, and the
    // service never sleeps for a transition — it stores the due instant.
    std::thread::sleep(TRANSITION_SPAN + std::time::Duration::from_millis(250));
}

/// The ARN form the model gives a dictionary: `dictionary/<id>` under this service's
/// namespace, in the request's region and the emulator's account (the modelled
/// `resourceArn` pattern admits `:(feed|dictionary)/[a-zA-Z0-9]{1,19}`).
pub fn expected_dictionary_arn(id: &str) -> String {
    format!(
        "arn:aws:{SIGNING_NAME}:{REGION}:{}:dictionary/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A router serving this service alone over one shared engine.
pub fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// As [`router`], and also the shared engine it serves from — for a suite that has to
/// reach this service's state directly (the fixture catalog no operation writes).
pub fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These requests carry an unsigned placeholder Authorization header, which is
    // there only so the router can resolve the signing name.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one request and return its status and parsed body (`Value::Null` when empty).
pub async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |body| serde_json::to_vec(&body).unwrap());
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .unwrap();
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|e| {
            panic!(
                "{method} {uri}: body is not JSON ({e}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// One `outputConfig` of each kind the `OutputConfig` union models
/// (`smithy_query(query="shape",shape="OutputConfig")`: cropping, clipping,
/// subtitling, contextualMetadata).
pub fn output_config(kind: &str) -> Value {
    match kind {
        "cropping" => json!({"cropping": {"templateGroups": [
            {"name": "group1", "templateUris": ["s3://templates/wide.psd"]}
        ]}}),
        "clipping" => json!({"clipping": {
            "callbackMetadata": "season opener",
            "dataSourceConfiguration": {"fixtureId": "fixture-1"}
        }}),
        "subtitling" => json!({"subtitling": {
            "language": "eng-gb",
            "aspectRatio": {"width": 16, "height": 9},
            "profanityFilter": "CENSOR"
        }}),
        "contextualMetadata" => json!({"contextualMetadata": {
            "summaryGeneration": "ENABLED",
            "extendedAnalysis": "DISABLED"
        }}),
        other => panic!("no such OutputConfig member: {other}"),
    }
}

/// One output whose subtitling config names `dictionary` (`SubtitlingConfig$dictionary`:
/// "the ID of a custom dictionary"), or names none when it is `None`.
///
/// A subtitling output is the ONLY way a feed references a dictionary, so this is the
/// one builder every suite that needs that reference goes through.
pub fn subtitling_output(name: &str, dictionary: Option<&str>) -> Value {
    let mut config = output_config("subtitling");
    if let Some(dictionary) = dictionary {
        config["subtitling"]
            .as_object_mut()
            .expect("the subtitling config is an object")
            .insert("dictionary".to_string(), json!(dictionary));
    }
    json!({"name": name, "outputConfig": config, "status": "ENABLED"})
}

/// A `CreateFeed` body with one enabled cropping output and no optional members: the
/// smallest feed a suite can create when the feed's own shape is not what it is
/// testing.
pub fn minimal_feed(name: &str) -> Value {
    json!({
        "name": name,
        "outputs": [
            {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
        ]
    })
}

/// Create one feed through the wire path and return its id.
pub async fn create_feed(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(router, "POST", "/v1/feed", Some(minimal_feed(name))).await;
    assert_eq!(
        status, 202,
        "CreateFeed({name}) must be accepted, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .expect("the created feed carries an id")
        .to_string()
}
