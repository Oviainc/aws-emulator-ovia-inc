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
//!
//! The fixed rows below span those maps one by one. Because the rule is a refusal that
//! holds for any payload putting the key in one of them, it is also driven over GENERATED
//! payloads — random map contents, variant lists and `children` trees — so no tree shape,
//! list position or nesting depth is left to a hand-written row. The last suite pins the
//! other side of the rule: a map the model states it for is the only place the key is
//! refused, so the identically named key in `events` or in a component's own `tags` is an
//! ordinary key and is stored.

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

/// The map a component — a reported one, or the payload a request carried — holds at
/// `path`, or `Null` when it holds none there.
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

/// How many generated payloads the generated suite judges. Bounded, so this stays a test
/// rather than a soak.
const GENERATED_CASES: usize = 48;

/// The base every generated case's seed is derived from, so the suite judges the same
/// payloads on every run and a reported failure is reproducible.
const GENERATOR_SEED: u64 = 0x5eed_1234_abcd_ef01;

/// The seeded generator the cases below draw their payloads from: SplitMix64, the mixing
/// function a 64-bit seed is expanded with.
///
/// Written here rather than taken from a generator crate on purpose: a dev-dependency
/// moves this crate's `Cargo.toml` and the workspace `Cargo.lock`, the two files every
/// other fix also moves, and this change has to land against a mainline that advances
/// between submits. Eight lines of arithmetic, nothing on disk, and the seed is a constant
/// every assertion message names. The same choice is made for the same reason in
/// `service-com-amazonaws-b2bi-b2bi/tests/all/generate_mapping.rs`.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..bound` — every bound here is tiny, so the modulo bias cannot affect
    /// whether the rule under test holds.
    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

/// The map shapes the rule covers, as the model targets them
/// (`smithy_query(query="shape",shape="ComponentProperties,ComponentOverrides,ComponentBindingProperties,ComponentCollectionProperties,ComponentVariantValues")`).
/// Which one a generated map is decides what an entry in it looks like, so a generated
/// payload is one the service deserializes and its answer is about the KEY alone.
#[derive(Clone, Copy, Debug)]
enum MapKind {
    /// `ComponentProperties`: each value is a `ComponentProperty`.
    Properties,
    /// `ComponentOverrides`: each value is a `ComponentOverridesValue`, a map of strings.
    Overrides,
    /// `ComponentBindingProperties`: each value is a `ComponentBindingPropertiesValue`.
    BindingProperties,
    /// `ComponentCollectionProperties`: each value is a `ComponentDataConfiguration`,
    /// whose `model` member is required.
    CollectionProperties,
    /// `ComponentVariantValues`: each value is a plain string.
    VariantValues,
}

impl MapKind {
    /// One entry's value, of the shape this map's value member targets.
    fn entry_value(self, nth: usize) -> Value {
        match self {
            MapKind::Properties => json!({"value": format!("value-{nth}")}),
            MapKind::Overrides => json!({"color": format!("color-{nth}")}),
            MapKind::BindingProperties => {
                json!({"type": "String", "defaultValue": format!("default-{nth}")})
            }
            MapKind::CollectionProperties => json!({"model": format!("Model{nth}")}),
            MapKind::VariantValues => json!(format!("variant-{nth}")),
        }
    }
}

/// A map of `kind` carrying ordinary property names only: the base a generated case then
/// puts one reserved key into.
fn allowed_map(kind: MapKind, rng: &mut SplitMix64) -> Value {
    let mut map = serde_json::Map::new();
    for nth in 0..1 + rng.below(3) {
        map.insert(format!("p{nth}"), kind.entry_value(nth));
    }
    Value::Object(map)
}

/// A generated `children` list `depth` levels deep, and the path of every
/// `ComponentChild$properties` map in it, relative to the payload holding the list.
///
/// How many siblings a level carries and how deeply the payload nests them is what the
/// rule generalises over, so both are generated rather than written out.
fn generated_children(
    rng: &mut SplitMix64,
    prefix: &str,
    depth: usize,
) -> (Value, Vec<(String, MapKind)>) {
    let mut maps = Vec::new();
    let mut children = Vec::new();
    if depth > 0 {
        for index in 0..1 + rng.below(2) {
            let path = format!("{prefix}children[{index}]");
            let properties = allowed_map(MapKind::Properties, rng);
            maps.push((format!("{path}.properties"), MapKind::Properties));
            let (nested, nested_maps) = generated_children(rng, &format!("{path}."), depth - 1);
            let nested = if nested_maps.is_empty() {
                None
            } else {
                maps.extend(nested_maps);
                Some(nested)
            };
            children.push(child(&format!("Child{index}"), properties, nested));
        }
    }
    (Value::Array(children), maps)
}

/// A generated payload, and the path of every map in it the rule covers.
struct Generated {
    /// The members a create or an update carries.
    members: Value,
    /// Every rule-covered map the payload holds, by the path the service names it at.
    maps: Vec<(String, MapKind)>,
}

/// One generated payload: the component's own maps, a generated `variants` list and a
/// generated `children` tree, all carrying ordinary property names.
fn generated_payload(rng: &mut SplitMix64) -> Generated {
    let mut members = json!({});
    let mut maps = Vec::new();

    for (member, kind) in [
        ("properties", MapKind::Properties),
        ("overrides", MapKind::Overrides),
        ("bindingProperties", MapKind::BindingProperties),
    ] {
        members[member] = allowed_map(kind, rng);
        maps.push((member.to_string(), kind));
    }
    // `collectionProperties` is the one of the four a payload may leave out.
    if rng.below(2) == 0 {
        members["collectionProperties"] = allowed_map(MapKind::CollectionProperties, rng);
        maps.push((
            "collectionProperties".to_string(),
            MapKind::CollectionProperties,
        ));
    }

    let mut variants = Vec::new();
    for index in 0..rng.below(4) {
        // Both `ComponentVariant` members are optional, so a generated variant carries
        // either, both or neither.
        let mut variant = json!({});
        if rng.below(10) < 7 {
            variant["variantValues"] = allowed_map(MapKind::VariantValues, rng);
            maps.push((
                format!("variants[{index}].variantValues"),
                MapKind::VariantValues,
            ));
        }
        if rng.below(10) < 7 {
            variant["overrides"] = allowed_map(MapKind::Overrides, rng);
            maps.push((format!("variants[{index}].overrides"), MapKind::Overrides));
        }
        variants.push(variant);
    }
    members["variants"] = Value::Array(variants);

    let depth = rng.below(4);
    let (children, child_maps) = generated_children(rng, "", depth);
    if !child_maps.is_empty() {
        members["children"] = children;
        maps.extend(child_maps);
    }

    Generated { members, maps }
}

/// `members` with the reserved key added to the map it holds at `path`.
fn with_reserved_key_at(members: &Value, path: &str, kind: MapKind) -> Value {
    let mut members = members.clone();
    let map = members
        .pointer_mut(&pointer_of(path))
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| panic!("the generated payload must hold a map at {path}"));
    map.insert(RESERVED_PROPERTY_NAME.to_string(), kind.entry_value(0));
    members
}

/// Over generated payloads, the reserved key is refused wherever one of the rule's maps
/// holds it — at any depth of `children`, any position of `variants`, in a map of any
/// size — and the same payload without it is accepted and reported back from the store,
/// on the create and on the update alike.
#[tokio::test]
async fn the_reserved_key_is_refused_in_a_generated_payload() {
    for index in 0..GENERATED_CASES {
        // One seed per case, from the fixed base: the payloads differ case to case and
        // repeat run to run, and every assertion message carries the seed that built the
        // case it failed on.
        let seed = GENERATOR_SEED ^ index as u64;
        let mut rng = SplitMix64(seed);
        let generated = generated_payload(&mut rng);
        let (path, kind) = generated.maps[rng.below(generated.maps.len())].clone();
        let case = format!("seed {seed}, case {index}: the reserved key in {path} ({kind:?})");
        let reserved = with_reserved_key_at(&generated.members, &path, kind);
        let router = emulator();

        // The member the refusal must name: the input member holding the payload, then
        // the path of the map inside it, so a superstring of the path does not satisfy it.
        let named_by_the_create = format!("componentToCreate.{path}");
        let named_by_the_update = format!("updatedComponent.{path}");

        let refused = create(&router, with_members(minimal_create("Generated"), &reserved)).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: the create must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert!(
            refused.message().contains(&named_by_the_create)
                && refused.message().contains(RESERVED_PROPERTY_NAME),
            "case {case}: the refusal must name {named_by_the_create} and the reserved key, got \
             {:?}",
            refused.message()
        );
        assert_eq!(
            refused.error_code(),
            "",
            "case {case}: a refusal must name no error type, got {:?}",
            refused.body
        );
        let listed = list(&router).await;
        assert!(
            listed_ids(&listed).is_empty(),
            "case {case}: a refused create must store nothing, got {:?}",
            listed.body
        );

        // The same payload without the reserved key: accepted, and every rule-covered map
        // in it reported back exactly as sent.
        let created = create(
            &router,
            with_members(minimal_create("Generated"), &generated.members),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case}: only the reserved key is refused, got {} with {:?}",
            created.status, created.body
        );
        let id = created.id();
        for (stored_path, _) in &generated.maps {
            assert_eq!(
                map_at(&created.body, stored_path),
                map_at(&generated.members, stored_path),
                "case {case}: the created component's {stored_path} must be what the create \
                 carried"
            );
        }

        // An update may not introduce the key either, and a refused update leaves the
        // stored record exactly as the create left it.
        let stored = get(&router, &id).await.body;
        let refused = update(&router, &id, reserved.clone()).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: the update must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert!(
            refused.message().contains(&named_by_the_update)
                && refused.message().contains(RESERVED_PROPERTY_NAME),
            "case {case}: the refusal must name {named_by_the_update} and the reserved key, got \
             {:?}",
            refused.message()
        );
        assert_eq!(
            refused.error_code(),
            "",
            "case {case}: a refusal must name no error type, got {:?}",
            refused.body
        );
        assert_eq!(
            get(&router, &id).await.body,
            stored,
            "case {case}: a refused update must leave the stored component alone"
        );

        // A second generated payload, carrying no reserved key, merges and reads back: the
        // refusal above is about the key, not about the members the patch carries.
        let patch = generated_payload(&mut rng);

        let patched = update(&router, &id, patch.members.clone()).await;
        assert_eq!(
            patched.status, 200,
            "case {case}: a patch without the reserved key must be accepted, got {} with {:?}",
            patched.status, patched.body
        );
        let read_back = get(&router, &id).await.body;
        for (patched_path, _) in &patch.maps {
            assert_eq!(
                map_at(&read_back, patched_path),
                map_at(&patch.members, patched_path),
                "case {case}: GetComponent must report the {patched_path} the update stored"
            );
        }
    }
}

/// The maps of a component payload the model documents NO reserved-name rule for, from
/// `smithy_query(query="shape",shape="Component,ComponentChild")`: `Component$events`,
/// `ComponentChild$events` and `Component$tags`. Each row is the case name, the path of
/// the map, the members putting the identically named key in it, and whether
/// `UpdateComponentData` carries that member at all.
fn rows_outside_the_rule() -> Vec<(&'static str, &'static str, Value, bool)> {
    vec![
        (
            "the component's own events",
            "events",
            json!({"events": {RESERVED_PROPERTY_NAME: {"action": "Amplify.Navigation"}}}),
            true,
        ),
        (
            "a child's events",
            "children[0].events",
            json!({"children": [{
                "componentType": "Text",
                "name": "Child",
                "properties": {"heading": {"value": "ok"}},
                "events": {RESERVED_PROPERTY_NAME: {"action": "Amplify.Navigation"}},
            }]}),
            true,
        ),
        (
            "the component's own tags",
            "tags",
            json!({"tags": {RESERVED_PROPERTY_NAME: "yes"}}),
            false,
        ),
    ]
}

/// A key named like the reserved property is an ordinary key in a map the model states no
/// rule for: it is stored and reported rather than refused, so the rule reaches exactly
/// the maps the model documents it for and no further.
#[tokio::test]
async fn an_identically_named_key_is_kept_where_the_rule_does_not_reach() {
    for (case, path, members, on_update) in rows_outside_the_rule() {
        let router = emulator();

        let created = create(&router, with_members(minimal_create("Outside"), &members)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: a key named like the reserved property is ordinary in {path}, got {} \
             with {:?}",
            created.status, created.body
        );
        assert_eq!(
            map_at(&created.body, path),
            map_at(&members, path),
            "case {case:?}: the created component's {path} must be what the create carried"
        );
        let id = created.id();
        assert_eq!(
            map_at(&get(&router, &id).await.body, path),
            map_at(&members, path),
            "case {case:?}: GetComponent must report the {path} the create stored"
        );

        if !on_update {
            continue;
        }
        let patched_id = create(&router, minimal_create("OutsideUpdated")).await.id();
        let patched = update(&router, &patched_id, members.clone()).await;
        assert_eq!(
            patched.status, 200,
            "case {case:?}: an update may carry that key in {path}, got {} with {:?}",
            patched.status, patched.body
        );
        assert_eq!(
            map_at(&get(&router, &patched_id).await.body, path),
            map_at(&members, path),
            "case {case:?}: GetComponent must report the {path} the update stored"
        );
    }
}
