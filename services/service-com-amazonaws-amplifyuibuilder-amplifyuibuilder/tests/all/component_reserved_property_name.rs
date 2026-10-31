// SPDX-License-Identifier: CC0-1.0

//! The reserved component property name, over every map the model documents the rule for.
//!
//! The rule under test: a map the model documents "You can't specify `tags` as a valid
//! property" for may not carry that key, wherever the create or update payload nests that
//! map — so a request naming it is refused with HTTP 400 and stores nothing, while the
//! identical shape under any other key is accepted and reported back unchanged.
//!
//! The maps the rule covers, from the model
//! (`smithy_query(query="shape",shape="Component,ComponentChild,ComponentVariant")`):
//! `Component$properties`, `$overrides`, `$bindingProperties` and `$collectionProperties`;
//! `ComponentChild$properties`, at every depth of `children`; and
//! `ComponentVariant$variantValues` and `$overrides`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment the cases address.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The map key no component property map may carry.
const RESERVED_PROPERTY_NAME: &str = "tags";

/// An ordinary property name, used to drive the accepted counterpart of each row: the
/// identical payload with the reserved key renamed.
const ALLOWED_PROPERTY_NAME: &str = "label";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The component id the answer carries, or the empty string. `CreateComponent`,
    /// `GetComponent` and `UpdateComponent` bind their component to `@httpPayload`, so the
    /// body IS the `Component`.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }

    /// The error-type discriminator the body carries — `__type`, or the `code` / `Code`
    /// spellings the same JSON error parsers accept — or the empty string when it carries
    /// none. AmplifyUIBuilder names no exception on the wire.
    fn error_code(&self) -> &str {
        ["__type", "code", "Code"]
            .iter()
            .find_map(|member| self.body[*member].as_str())
            .unwrap_or_default()
    }

    /// The `message` the refusal carries, or the empty string.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "amplifyuibuilder.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_AMPLIFYUIBUILDER);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

fn components_uri() -> String {
    format!("/app/{APP_ID}/environment/{ENVIRONMENT}/components")
}

async fn create(router: &MultiServiceRouter, data: Value) -> Answer {
    call(router, "POST", &components_uri(), Some(data)).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("{}/{id}", components_uri());
    call(router, "GET", &uri, None).await
}

async fn update(router: &MultiServiceRouter, id: &str, patch: Value) -> Answer {
    let uri = format!("{}/{id}", components_uri());
    call(router, "PATCH", &uri, Some(patch)).await
}

async fn list(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", &components_uri(), None).await
}

/// The ids `ListComponents` reported.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["entities"]
        .as_array()
        .unwrap_or_else(|| panic!("ListComponents must return an entities list: {:?}", answer.body))
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The members `CreateComponentData` requires, as the base each row overrides.
fn minimal_create(name: &str) -> Value {
    json!({
        "name": name,
        "componentType": "View",
        "properties": {},
        "variants": [],
        "overrides": {},
        "bindingProperties": {},
    })
}

/// One `ComponentChild`: the three members it requires, plus its own `children` when the
/// row nests deeper.
fn child(name: &str, properties: Value, children: Option<Value>) -> Value {
    let mut value = json!({
        "componentType": "Text",
        "name": name,
        "properties": properties,
    });
    if let Some(children) = children {
        value["children"] = children;
    }
    value
}

/// `base` with every member of `members` replaced by the row's value.
fn with_members(base: Value, members: &Value) -> Value {
    let mut data = base;
    let members = members
        .as_object()
        .unwrap_or_else(|| panic!("a row's members must be an object, got {members:?}"));
    for (member, value) in members {
        data[member.as_str()] = value.clone();
    }
    data
}

/// The same payload with every reserved key renamed to an ordinary property name, so one
/// row drives both the refusal and the acceptance of the identical shape.
fn under_an_allowed_key(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, nested)| {
                    let key = if key == RESERVED_PROPERTY_NAME {
                        ALLOWED_PROPERTY_NAME.to_string()
                    } else {
                        key.clone()
                    };
                    (key, under_an_allowed_key(nested))
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(under_an_allowed_key).collect()),
        other => other.clone(),
    }
}

/// The JSON pointer addressing the map a row is about, from the member path the service
/// reports: `children[0].properties` addresses `/children/0/properties`.
fn pointer_of(path: &str) -> String {
    let flat = path.replace('[', ".").replace(']', "");
    let mut pointer = String::new();
    for segment in flat.split('.') {
        pointer.push('/');
        pointer.push_str(segment);
    }
    pointer
}

/// The map a component reports at `path`, or `Null` when it reports none there.
fn map_at(component: &Value, path: &str) -> Value {
    component
        .pointer(&pointer_of(path))
        .cloned()
        .unwrap_or(Value::Null)
}

/// One row per map the model documents the rule for: the case name, the member path the
/// refusal must name, and the create members that put the reserved key in that map. The
/// rows differ along what the rule generalises over — which map, which value shape, which
/// list index and how deeply the payload nests it.
fn rows() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "the component's own properties",
            "properties",
            json!({"properties": {RESERVED_PROPERTY_NAME: {"value": "no"}}}),
        ),
        (
            "the component's own overrides",
            "overrides",
            json!({"overrides": {RESERVED_PROPERTY_NAME: {"color": "no"}}}),
        ),
        (
            "the component's own bindingProperties",
            "bindingProperties",
            json!({"bindingProperties": {RESERVED_PROPERTY_NAME: {"type": "String"}}}),
        ),
        (
            "the component's own collectionProperties",
            "collectionProperties",
            json!({"collectionProperties": {RESERVED_PROPERTY_NAME: {"model": "Post"}}}),
        ),
        (
            "the only child's properties",
            "children[0].properties",
            json!({"children": [child("Only", json!({RESERVED_PROPERTY_NAME: {"value": "no"}}), None)]}),
        ),
        (
            "the second child's properties",
            "children[1].properties",
            json!({"children": [
                child("First", json!({"heading": {"value": "ok"}}), None),
                child("Second", json!({RESERVED_PROPERTY_NAME: {"value": "no"}}), None),
            ]}),
        ),
        (
            "a grandchild's properties",
            "children[0].children[0].properties",
            json!({"children": [child(
                "Parent",
                json!({"heading": {"value": "ok"}}),
                Some(json!([child("Grandchild", json!({RESERVED_PROPERTY_NAME: {"value": "no"}}), None)])),
            )]}),
        ),
        (
            "a great-grandchild's properties, past the second child of its level",
            "children[0].children[1].children[0].properties",
            json!({"children": [child(
                "Parent",
                json!({"heading": {"value": "ok"}}),
                Some(json!([
                    child("FirstGrandchild", json!({"heading": {"value": "ok"}}), None),
                    child(
                        "SecondGrandchild",
                        json!({"heading": {"value": "ok"}}),
                        Some(json!([child(
                            "GreatGrandchild",
                            json!({RESERVED_PROPERTY_NAME: {"value": "no"}}),
                            None,
                        )])),
                    ),
                ])),
            )]}),
        ),
        (
            "the first variant's variantValues",
            "variants[0].variantValues",
            json!({"variants": [{"variantValues": {RESERVED_PROPERTY_NAME: "no"}}]}),
        ),
        (
            "the second variant's overrides",
            "variants[1].overrides",
            json!({"variants": [
                {"variantValues": {"size": "large"}},
                {"overrides": {RESERVED_PROPERTY_NAME: {"color": "no"}}},
            ]}),
        ),
    ]
}

/// A create naming the reserved key in ANY of those maps is refused and stores nothing,
/// and the identical payload under an ordinary key is accepted and reported back.
#[tokio::test]
async fn a_reserved_key_is_refused_in_every_map_the_rule_covers() {
    for (case, path, members) in rows() {
        let router = emulator();

        let refused = create(&router, with_members(minimal_create("Reserved"), &members)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a create naming the reserved key in {path} must be refused, got {} \
             with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "",
            "case {case:?}: a refusal must name no error type, got {:?}",
            refused.body
        );
        assert!(
            refused.message().contains(path) && refused.message().contains(RESERVED_PROPERTY_NAME),
            "case {case:?}: the refusal must name {path} and the reserved key, got {:?}",
            refused.message()
        );
        assert!(
            listed_ids(&list(&router).await).is_empty(),
            "case {case:?}: a refused create must store nothing, got {:?}",
            list(&router).await.body
        );

        // The identical shape under an ordinary key: accepted, and reported back from the
        // persisted record by `GetComponent`.
        let allowed = under_an_allowed_key(&members);
        let created = create(&router, with_members(minimal_create("Allowed"), &allowed)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: only the reserved key is refused, got {} with {:?}",
            created.status, created.body
        );
        let stored_map = map_at(&created.body, path);
        assert!(
            stored_map.get(ALLOWED_PROPERTY_NAME).is_some()
                && stored_map.get(RESERVED_PROPERTY_NAME).is_none(),
            "case {case:?}: the created component's {path} must carry {ALLOWED_PROPERTY_NAME:?} \
             and no reserved key, got {stored_map:?}"
        );
        let id = created.id();
        let read_back = get(&router, &id).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: GetComponent must report the created component, got {} with {:?}",
            read_back.status, read_back.body
        );
        assert_eq!(
            map_at(&read_back.body, path),
            stored_map,
            "case {case:?}: GetComponent must report the {path} the create stored"
        );
    }
}

/// An update may not introduce the reserved key into any of those maps either, and a
/// refused update leaves the stored component exactly as it was.
#[tokio::test]
async fn a_reserved_key_is_refused_on_an_update_too() {
    for (case, path, members) in rows() {
        let router = emulator();
        let id = create(&router, minimal_create("Updatable")).await.id();
        let stored = get(&router, &id).await.body;

        let refused = update(&router, &id, members.clone()).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: an update naming the reserved key in {path} must be refused, got {} \
             with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "",
            "case {case:?}: a refusal must name no error type, got {:?}",
            refused.body
        );
        assert!(
            refused.message().contains(path) && refused.message().contains(RESERVED_PROPERTY_NAME),
            "case {case:?}: the refusal must name {path} and the reserved key, got {:?}",
            refused.message()
        );
        assert_eq!(
            get(&router, &id).await.body,
            stored,
            "case {case:?}: a refused update must leave the stored component alone"
        );

        // The same update under an ordinary key goes through, so the refusal is about the
        // key and not about the member the patch carries.
        let accepted = update(&router, &id, under_an_allowed_key(&members)).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: only the reserved key is refused, got {} with {:?}",
            accepted.status, accepted.body
        );
        let patched = map_at(&accepted.body, path);
        assert!(
            patched.get(ALLOWED_PROPERTY_NAME).is_some()
                && patched.get(RESERVED_PROPERTY_NAME).is_none(),
            "case {case:?}: the updated component's {path} must carry {ALLOWED_PROPERTY_NAME:?} \
             and no reserved key, got {patched:?}"
        );
        assert_eq!(
            map_at(&get(&router, &id).await.body, path),
            patched,
            "case {case:?}: GetComponent must report the {path} the update stored"
        );
    }
}
