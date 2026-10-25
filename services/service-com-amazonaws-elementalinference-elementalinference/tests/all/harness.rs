// SPDX-License-Identifier: CC0-1.0

//! The one request harness every suite in this crate drives the emulator through.
//!
//! A test here is an END-TO-END request: the router picks the service from the SigV4
//! credential scope, the framework deserializes and validates the body, the handler
//! runs, and the response is serialized as `restJson1` — so what a suite asserts is
//! what a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    current_epoch_secs, default_fixture_catalog, TRANSITION_SPAN,
};
use service_com_amazonaws_elementalinference_elementalinference::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The two kinds of resource this service mints and tags, as the modelled `resourceArn`
/// pattern spells them (`…:(feed|dictionary)/[a-zA-Z0-9]{1,19}$`). A rule about a
/// RESOURCE is asserted on both.
pub const TAGGABLE_KINDS: [&str; 2] = ["feed", "dictionary"];

/// The region every request below is signed for. The service reads it out of the SigV4
/// credential scope, so the ARNs it mints must carry exactly this region.
pub const REGION: &str = "us-west-2";

/// The `maxResults` a suite asks for when it means "every resource of this endpoint":
/// the largest page a list of this service admits
/// (`smithy_query(query="operation",operation="ListFeeds")` gives `maxResults` the range
/// `{min: 1, max: 100}`), which is also as many resources of a kind as one Region can
/// hold, so one call reports all of them and a suite never has to page.
pub const PAGE_SIZE: usize = 100;

/// The `message` an error body carries.
pub fn message_of(error: &Value) -> &str {
    error["message"]
        .as_str()
        .unwrap_or_else(|| panic!("the refusal carries a message, got {error}"))
}

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

/// Percent-encode `value` for one component of a request URI — a path segment or a
/// query-parameter value.
///
/// Every byte outside RFC 3986's unreserved set is escaped, which is what a client does
/// for a value it binds to one component: notably the `/` and `:` of an ARN, which would
/// otherwise read as further path segments, and the `&` and `=` of a query value, which
/// would otherwise read as further parameters.
pub fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The path a tagging request goes to, with the `resourceArn` percent-encoded.
///
/// `resourceArn` is an `@httpLabel` bound to one path segment, so a client encodes it
/// ([`percent_encode`]) exactly as this does.
pub fn tags_path(resource_arn: &str) -> String {
    format!("/v1/tags/{}", percent_encode(resource_arn))
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
///
/// Signed for [`REGION`], the Region every suite that is not testing the Region
/// dimension uses.
pub async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(router, REGION, method, uri, body).await
}

/// Send one request signed for `region` and return its status and parsed body.
///
/// This service is REGIONAL: the Region a request is signed for is the endpoint it
/// reached, and the service reads it out of the SigV4 credential scope. A suite that
/// asserts what one Region's endpoint owns sends its requests through here.
pub async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
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
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/{SIGNING_NAME}/aws4_request, \
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
            "dataSourceConfiguration": {"fixtureId": held_fixture_id()}
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

/// Seconds in a UTC day: a UTC day has no offset changes, so an instant's day number is
/// its epoch-second count divided by this.
const SECONDS_PER_DAY: i64 = 86_400;

/// The `fixtureId` of a fixture this emulator's data source really holds.
///
/// A clipping output's data source names the fixture whose event data the service maps
/// onto that output, and the id comes from a `SearchFixtures` result
/// (`DataSourceConfiguration$fixtureId`), so an output a suite sends has to name a
/// fixture the catalog holds. The catalog a store comes up with is
/// `default_fixture_catalog`, and a held fixture's id depends only on the sport, the UTC
/// day and the slot — so the fixture scheduled TODAY is in the catalog of every store
/// built within a day either side of this call, unlike one at the ends of the span the
/// schedule covers.
pub fn held_fixture_id() -> String {
    let now = current_epoch_secs();
    let today = now.div_euclid(SECONDS_PER_DAY);
    default_fixture_catalog(now)
        .into_iter()
        .find(|fixture| fixture.scheduled_day() == today)
        .map(|fixture| fixture.id)
        .expect("the data source's schedule covers the present day")
}

/// One output named `name`, carrying the `kind` config and `status` — the three members
/// `CreateOutput` requires (`smithy_query(query="shape",shape="CreateOutput")`).
pub fn output(name: &str, kind: &str, status: &str) -> Value {
    json!({"name": name, "outputConfig": output_config(kind), "status": status})
}

/// One ENABLED output named `name`, carrying the `kind` config — the output a suite
/// builds when the output's own `status` is not what it is testing.
pub fn enabled_output(name: &str, kind: &str) -> Value {
    output(name, kind, "ENABLED")
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

/// One output named `name` whose clipping config names `fixture_id` as its data source
/// (`DataSourceConfiguration$fixtureId`: "The ID of the fixture whose event data you want
/// Elemental Inference to map onto this clipping output").
///
/// A clipping output's data source is the ONLY way a feed references a fixture, so this is
/// the one builder every suite that needs that reference goes through — the fixture's
/// counterpart of [`subtitling_output`].
pub fn clipping_output(name: &str, fixture_id: &str) -> Value {
    json!({
        "name": name,
        "status": "ENABLED",
        "outputConfig": {"clipping": {
            "callbackMetadata": "season opener",
            "dataSourceConfiguration": {"fixtureId": fixture_id}
        }},
    })
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

/// A tag pair, from two `&str`s.
pub fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// A `TagMap` body member built from a list of pairs.
pub fn tag_object(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

/// The `tags` member of a resource response, as a map so a comparison does not depend
/// on key order.
pub fn tags_of(resource: &Value) -> BTreeMap<String, String> {
    resource["tags"]
        .as_object()
        .unwrap_or_else(|| panic!("a resource response carries a tags map, got {resource}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("a tag value is a string, got {value}"))
                    .to_string(),
            )
        })
        .collect()
}

/// A tag set as a map, for comparing against what a read reports.
pub fn expected_tags(tags: &[(String, String)]) -> BTreeMap<String, String> {
    tags.iter().cloned().collect()
}

/// The create body of `kind`, named `name` and carrying `tags` — the smallest resource of
/// either kind, so a suite about TAGS states nothing else about the resource.
pub fn tagged_create_body(kind: &str, name: &str, tags: &[(String, String)]) -> Value {
    match kind {
        "feed" => json!({
            "name": name,
            "outputs": [
                {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
            ],
            "tags": tag_object(tags),
        }),
        "dictionary" => json!({"name": name, "language": "eng", "tags": tag_object(tags)}),
        other => panic!("no such resource kind: {other}"),
    }
}

/// Create one resource of `kind` carrying `tags`, and return its id and its ARN.
pub async fn create_tagged_resource(
    router: &MultiServiceRouter,
    kind: &str,
    name: &str,
    tags: &[(String, String)],
) -> (String, String) {
    let body = tagged_create_body(kind, name, tags);
    let (status, created) = call(router, "POST", &format!("/v1/{kind}"), Some(body)).await;
    assert_eq!(
        status, 202,
        "creating a {kind} named {name:?} answers 202, got {status} with {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a created {kind} carries an id, got {created}"))
        .to_string();
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a created {kind} carries an arn, got {created}"))
        .to_string();
    (id, arn)
}

/// Read one resource of `kind` back through its own implemented reader (`GetFeed` /
/// `GetDictionary`), which is where a tag write or delete has to show up.
pub async fn read_resource(router: &MultiServiceRouter, kind: &str, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/v1/{kind}/{id}"), None).await
}

/// The ids one endpoint's list operation reports, sorted so a comparison does not
/// depend on which Region minted an id first.
///
/// One page of [`PAGE_SIZE`], which is every resource of the kind a Region can hold, so
/// the list a suite compares against is the whole of what that endpoint reports.
pub async fn listed_ids(
    router: &MultiServiceRouter,
    region: &str,
    path: &str,
    member: &str,
) -> Vec<String> {
    let uri = format!("{path}?maxResults={PAGE_SIZE}");
    let (status, page) = call_in_region(router, region, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {region:?}: GET {uri} answers 200, got {status} with {page}"
    );
    assert!(
        page["nextToken"].is_null(),
        "case {region:?}: every resource of this Region fits in one page, got {page}"
    );
    let mut ids: Vec<String> = page[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {region:?}: GET {uri} reports a {member} array, got {page}"))
        .iter()
        .map(|summary| {
            summary["id"]
                .as_str()
                .expect("a summary carries an id")
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// One row of the unresolvable-ARN table: how the ARN differs from the one the resource
/// carries.
pub struct ArnCase {
    pub label: &'static str,
    pub arn: String,
}

/// ARNs that satisfy the modelled `resourceArn` pattern but name no resource of this
/// store, built by moving each field of a real resource's ARN in turn: the resource
/// kind, the id, the region and the account.
pub fn unresolvable_arns(kind: &str, arn: &str) -> Vec<ArnCase> {
    let other_kind = if kind == "feed" { "dictionary" } else { "feed" };
    let fields: Vec<&str> = arn.split(':').collect();
    let (region, account, resource) = (fields[3], fields[4], fields[5]);
    let id = resource.split('/').next_back().expect("the ARN carries an id");
    let rebuilt = |region: &str, account: &str, resource: &str| {
        format!(
            "{}:{}:{}:{region}:{account}:{resource}",
            fields[0], fields[1], fields[2]
        )
    };
    vec![
        ArnCase {
            label: "an id no resource of this kind holds",
            arn: rebuilt(region, account, &format!("{kind}/aaaaaaaaaaaaaaaaaaa")),
        },
        ArnCase {
            label: "this resource's id under the other resource kind",
            arn: rebuilt(region, account, &format!("{other_kind}/{id}")),
        },
        ArnCase {
            label: "the right resource in another region",
            arn: rebuilt("eu-west-1", account, resource),
        },
        ArnCase {
            label: "the right resource in another account",
            // Any 12-digit account that is not the one this emulator serves
            // (`engine::EMULATOR_ACCOUNT_ID`), which the stored ARN carries.
            arn: rebuilt(region, "123456789012", resource),
        },
    ]
}
