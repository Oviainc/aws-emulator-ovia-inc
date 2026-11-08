// SPDX-License-Identifier: CC0-1.0

//! The Amplify UI Builder component lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one the control plane states for its primary
//! resource: a component's members are whatever the create, or a later update, stored, and
//! every operation computes its answer from that record.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::state::{
    page_token, parse_page_token, PagedCollection,
};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The largest page `ListComponentsRequest$maxResults` admits: it targets
/// `ListEntityLimit`, modelled `@range(min: 1, max: 100)`. Spelled out here, independently
/// of the constant `src/state.rs` carries, so a wrong bound in the implementation fails.
const MAX_RESULTS_CEILING: i64 = 100;

/// The smallest page that member admits.
const MAX_RESULTS_FLOOR: i64 = 1;

/// The longest `ComponentName` / `ComponentType` the model admits (`@length(min: 1, max:
/// 255)` on both).
const NAME_MAX_LENGTH: usize = 255;

/// The map key `Component$properties`, `$overrides`, `$bindingProperties` and
/// `$collectionProperties` each document as not a valid property name ("You can't specify
/// `tags` as a valid property for ...").
const RESERVED_PROPERTY_NAME: &str = "tags";

/// A SigV4 credential scope naming this service: it is what routes a request to the
/// amplifyuibuilder service.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment the cases address, unless a case is about addressing
/// another one.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// `Component$id` targets the model's `Uuid` shape. Checked here against the textual form
/// of a version 4 UUID rather than through the crate's own minting code, so an id that is
/// not a UUID cannot make this test pass.
fn is_uuid_v4(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let ok = match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
        };
        if !ok {
            return false;
        }
    }
    // Version nibble, then the variant nibble the RFC pins to one of 8, 9, a, b.
    bytes[14] == b'4' && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
}

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
    /// The component id the answer carries, or the empty string.
    ///
    /// `CreateComponent`, `GetComponent` and `UpdateComponent` all bind their component to
    /// `@httpPayload`, so the body IS the `Component`.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }

    /// The error-type discriminator the body carries — `__type`, or the `code` / `Code`
    /// spellings the same JSON error parsers also accept — or the empty string when it
    /// carries none.
    ///
    /// Every refusal of one of this service's own modelled exceptions must carry NONE of
    /// them: AmplifyUIBuilder names no exception on the wire, so a client classifies a
    /// failure by HTTP status.
    fn error_code(&self) -> &str {
        ["__type", "code", "Code"]
            .iter()
            .find_map(|member| self.body[*member].as_str())
            .unwrap_or_default()
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

fn components_uri(app_id: &str, environment: &str) -> String {
    format!("/app/{app_id}/environment/{environment}/components")
}

async fn create_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    data: Value,
) -> Answer {
    call(
        router,
        "POST",
        &components_uri(app_id, environment),
        Some(data),
    )
    .await
}

async fn create(router: &MultiServiceRouter, data: Value) -> Answer {
    create_in(router, APP_ID, ENVIRONMENT, data).await
}

async fn create_with_token(router: &MultiServiceRouter, data: Value, token: &str) -> Answer {
    create_in_with_token(router, APP_ID, ENVIRONMENT, data, token).await
}

async fn create_in_with_token(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    data: Value,
    token: &str,
) -> Answer {
    let uri = format!("{}?clientToken={}", components_uri(app_id, environment), token);
    call(router, "POST", &uri, Some(data)).await
}

async fn get_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    id: &str,
) -> Answer {
    let uri = format!("{}/{id}", components_uri(app_id, environment));
    call(router, "GET", &uri, None).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    get_in(router, APP_ID, ENVIRONMENT, id).await
}

async fn list_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    query: &str,
) -> Answer {
    let uri = format!("{}{query}", components_uri(app_id, environment));
    call(router, "GET", &uri, None).await
}

async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    list_in(router, APP_ID, ENVIRONMENT, query).await
}

async fn update(router: &MultiServiceRouter, id: &str, patch: Value) -> Answer {
    let uri = format!("{}/{id}", components_uri(APP_ID, ENVIRONMENT));
    call(router, "PATCH", &uri, Some(patch)).await
}

async fn delete(router: &MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("{}/{id}", components_uri(APP_ID, ENVIRONMENT));
    call(router, "DELETE", &uri, None).await
}

/// The ids `ListComponents` reported, in the order it reported them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["entities"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListComponents must return an entities list: {:?}",
                answer.body
            )
        })
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// True when `value` is a JSON string an ISO-8601 / RFC 3339 date-time parser accepts —
/// the wire form `@timestampFormat("date-time")` requires of `Component$createdAt` and
/// `$modifiedAt`.
fn is_date_time(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| DateTime::from_str(text, Format::DateTime).is_ok())
}

/// The instant a `date-time` member names, in seconds since the epoch. Panics on anything
/// that is not such a string, so a wrong wire form fails the case that reads it.
fn epoch_seconds(value: &Value) -> f64 {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("expected a date-time string, got {value:?}"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| panic!("{text:?} is not an ISO-8601 date-time ({error})"))
        .as_secs_f64()
}

/// A one-entry JSON object, for a case whose key is computed rather than written out.
fn one_entry(key: &str, value: Value) -> Value {
    let mut map = Map::new();
    map.insert(key.to_string(), value);
    Value::Object(map)
}

/// The six members `CreateComponentData` requires, as a starting point for a case that is
/// about something other than those values.
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

/// The create payloads the round-trip and list cases drive, differing along what the rule
/// generalises over: which values, how many map and list entries, how deeply nested the
/// children are, whether an optional member is present at all, and the published length
/// bounds of `name` and `componentType`.
fn create_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("only the required members", minimal_create("Hero")),
        (
            "bound properties, one variant, tags and a source id",
            json!({
                "name": "HeroCard",
                "componentType": "Card",
                "properties": {
                    "label": {"value": "Hello"},
                    "src": {"bindingProperties": {"property": "image", "field": "url"}},
                },
                "variants": [
                    {"variantValues": {"size": "large"}, "overrides": {"Text": {"fontSize": "24px"}}},
                ],
                "overrides": {"Text": {"color": "#ffffff"}},
                "bindingProperties": {
                    "image": {
                        "type": "Data",
                        "bindingProperties": {"model": "Post", "field": "image"},
                    },
                },
                "tags": {"env": "test", "team": "studio"},
                "sourceId": "figma:1:23",
                "schemaVersion": "1.0",
            }),
        ),
        (
            "nested children, events and collection properties",
            json!({
                "name": "PostCollection",
                "componentType": "Collection",
                "properties": {},
                "variants": [],
                "overrides": {},
                "bindingProperties": {},
                "children": [
                    {
                        "componentType": "Text",
                        "name": "Title",
                        "properties": {"label": {"value": "a title"}},
                    },
                    {
                        "componentType": "Flex",
                        "name": "Body",
                        "properties": {},
                        "children": [
                            {"componentType": "Badge", "name": "Tag", "properties": {}},
                        ],
                    },
                ],
                "events": {
                    "click": {
                        "action": "Amplify.Navigation",
                        "parameters": {"url": {"value": "/home"}},
                    },
                },
                "collectionProperties": {
                    "posts": {
                        "model": "Post",
                        "sort": [{"field": "createdAt", "direction": "DESC"}],
                        "identifiers": ["id"],
                    },
                },
            }),
        ),
        (
            "three variants and several overridden elements",
            json!({
                "name": "Button",
                "componentType": "Button",
                "properties": {"label": {"value": "Go"}},
                "variants": [
                    {"variantValues": {"state": "default"}},
                    {"variantValues": {"state": "hover"}, "overrides": {"Button": {"color": "red"}}},
                    {"variantValues": {"state": "pressed", "size": "small"}, "overrides": {}},
                ],
                "overrides": {
                    "Button": {"borderRadius": "4px", "padding": "8px"},
                    "Label": {"fontWeight": "700"},
                },
                "bindingProperties": {},
            }),
        ),
        (
            "the longest name the model admits and the shortest component type",
            json!({
                "name": "n".repeat(NAME_MAX_LENGTH),
                "componentType": "V",
                "properties": {},
                "variants": [],
                "overrides": {},
                "bindingProperties": {},
            }),
        ),
    ]
}

/// `CreateComponent` persists what it was given and `GetComponent` reports that record:
/// every member the create carried comes back with the value it was stored with, the id is
/// a fresh UUID, `createdAt` and `modifiedAt` are date-time strings at the same instant,
/// and the two bodies are equal.
#[tokio::test]
async fn a_created_component_is_read_back_from_what_was_stored() {
    let router = emulator();
    let mut minted = Vec::new();

    for (case, data) in create_cases() {
        let created = create(&router, data.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateComponent, got {} with {:?}",
            created.status, created.body
        );

        let id = created.id();
        assert!(
            is_uuid_v4(&id),
            "case {case:?}: expected a version 4 UUID id, got {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "case {case:?}: CreateComponent reissued the id {id:?}"
        );
        minted.push(id.clone());

        assert_eq!(
            at(&created.body, "/appId"),
            json!(APP_ID),
            "case {case:?}: the created component must carry the appId it was created under"
        );
        assert_eq!(
            at(&created.body, "/environmentName"),
            json!(ENVIRONMENT),
            "case {case:?}: the created component must carry the environment it was created under"
        );
        assert!(
            is_date_time(&at(&created.body, "/createdAt")),
            "case {case:?}: createdAt must be an ISO-8601 date-time string, got {:?}",
            at(&created.body, "/createdAt")
        );
        assert_eq!(
            at(&created.body, "/modifiedAt"),
            at(&created.body, "/createdAt"),
            "case {case:?}: a component nothing has updated reports modifiedAt at its \
             createdAt, got {:?}",
            at(&created.body, "/modifiedAt")
        );

        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetComponent, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created.body,
            "case {case:?}: GetComponent must report exactly what CreateComponent persisted"
        );

        for (member, expected) in data.as_object().expect("each case is a JSON object") {
            let observed = at(&read.body, &format!("/{member}"));
            assert_eq!(
                &observed, expected,
                "case {case:?}: member {member:?} was created as {expected:?} but \
                 GetComponent reports {observed:?}"
            );
        }
    }

    // Every created component, and only those, is listed for that environment.
    let listed = listed_ids(&list(&router, "").await);
    assert_eq!(
        listed, minted,
        "ListComponents must report every stored component once, in creation order"
    );
}

/// `clientToken` is an `@idempotencyToken`: the same token always names the component it
/// first created, distinct tokens name distinct components, and no token at all mints a new
/// component every time.
#[tokio::test]
async fn a_repeated_create_with_one_client_token_returns_one_component() {
    // Rows differ in token shape and in how often the create is retried.
    let cases: Vec<(&str, String, usize)> = vec![
        ("a plain token, retried once", "token-a".to_string(), 2),
        ("the same token, retried four times", "token-b".to_string(), 4),
        ("a one-character token", "!".to_string(), 3),
        ("a 64-character token", "t".repeat(64), 2),
        ("a token of punctuation only", "~!-_.%7E".to_string(), 2),
    ];

    let router = emulator();
    let mut distinct = Vec::new();
    for (index, (case, token, attempts)) in cases.iter().enumerate() {
        let mut ids = Vec::new();
        for attempt in 0..*attempts {
            // A name per row: a component name is unique within its app environment, so
            // two rows sharing one would be refused as a conflict (see
            // `component_refusals.rs`) rather than exercising the token rule.
            let data = minimal_create(&format!("Idempotent{index}"));
            let answer = create_with_token(&router, data, token).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: attempt {attempt} expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            ids.push(answer.id());
        }
        assert!(
            ids.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case:?}: one client token must name one component, got {ids:?}"
        );
        distinct.push(ids[0].clone());
    }

    // Two creates with no token at all are two components.
    let first = create(&router, minimal_create("Untokened")).await.id();
    let second = create(&router, minimal_create("UntokenedAgain")).await.id();
    assert_ne!(
        first, second,
        "a create carrying no clientToken must mint a new component every time"
    );
    distinct.push(first);
    distinct.push(second);

    let mut unique = distinct.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        distinct.len(),
        "distinct client tokens must name distinct components, got {distinct:?}"
    );

    let listed = listed_ids(&list(&router, "").await);
    assert_eq!(
        listed.len(),
        distinct.len(),
        "state must hold exactly the components that were created, got {listed:?} for \
         {distinct:?}"
    );

    // A token is scoped to the app environment it was used in: reusing one elsewhere
    // creates that environment's own component rather than reporting one the request does
    // not address. Rows differ in which half of the key changes.
    let shared_token = "token-a";
    for (case, app_id, environment) in [
        ("another environment of the same app", APP_ID, "production"),
        ("another app, the same environment", "d9999zzzz", ENVIRONMENT),
        ("another app and environment", "d9999zzzz", "production"),
    ] {
        let answer =
            create_in_with_token(&router, app_id, environment, minimal_create("Scoped"), shared_token)
                .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            at(&answer.body, "/appId"),
            json!(app_id),
            "case {case:?}: the answer must carry the appId the request addressed, got {:?}",
            answer.body
        );
        assert_eq!(
            at(&answer.body, "/environmentName"),
            json!(environment),
            "case {case:?}: the answer must carry the environment the request addressed, got \
             {:?}",
            answer.body
        );
        let id = answer.id();
        assert!(
            !distinct.contains(&id),
            "case {case:?}: a token reused in another app environment must not replay a \
             component stored elsewhere, got {id:?}"
        );
        assert_eq!(
            listed_ids(&list_in(&router, app_id, environment, "").await),
            vec![id.clone()],
            "case {case:?}: the component must be stored where the request addressed it"
        );
        // Retried there, the same token is still idempotent.
        let retried =
            create_in_with_token(&router, app_id, environment, minimal_create("Scoped"), shared_token)
                .await;
        assert_eq!(
            retried.id(),
            id,
            "case {case:?}: one token names one component within its own app environment"
        );
        distinct.push(id);
    }
}

/// `ListComponents` is `@paginated(items: entities)`: whatever `maxResults` it is given,
/// following `nextToken` to the end enumerates every component stored for that app
/// environment exactly once, in one stable order, and no page is longer than was asked for.
#[tokio::test]
async fn list_components_pages_over_one_stable_order() {
    let router = emulator();
    let mut created = Vec::new();
    for index in 0..7 {
        created.push(
            create(&router, minimal_create(&format!("page-{index}")))
                .await
                .id(),
        );
    }
    // A component in another environment of the same app, and one in another app: neither
    // belongs to the listed environment.
    let other_environment = create_in(&router, APP_ID, "production", minimal_create("Elsewhere"))
        .await
        .id();
    let other_app = create_in(&router, "d9999zzzz", ENVIRONMENT, minimal_create("Elsewhere"))
        .await
        .id();

    let unpaged = listed_ids(&list(&router, "").await);
    assert_eq!(
        unpaged, created,
        "an unpaged ListComponents must report every component of that environment in \
         creation order"
    );
    assert!(
        !unpaged.contains(&other_environment) && !unpaged.contains(&other_app),
        "ListComponents must report only the addressed (appId, environmentName), got \
         {unpaged:?}"
    );

    // Both published bounds of `maxResults`, and the sizes between that cut the seven
    // components into a different number of pages.
    for page_size in [
        MAX_RESULTS_FLOOR,
        2,
        3,
        6,
        7,
        MAX_RESULTS_CEILING,
    ] {
        let mut walked = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let answer = list(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={page_size}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let page = listed_ids(&answer);
            assert!(
                page.len() <= page_size as usize,
                "case maxResults={page_size}: a page carried {} components: {page:?}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case maxResults={page_size}: pagination did not terminate, walked {walked:?}"
            );
            match answer.body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, created,
            "case maxResults={page_size}: walking every page must enumerate every component \
             once, in the same order"
        );
    }

    // A page that ends the list carries no token.
    let whole = list(&router, &format!("?maxResults={MAX_RESULTS_CEILING}")).await;
    assert_eq!(
        whole.body["nextToken"],
        Value::Null,
        "the last page must omit nextToken, got {:?}",
        whole.body
    );

    // One past each published bound of `maxResults` is refused at the boundary.
    for page_size in [MAX_RESULTS_FLOOR - 1, MAX_RESULTS_CEILING + 1] {
        let refused = list(&router, &format!("?maxResults={page_size}")).await;
        assert_eq!(
            refused.status, 400,
            "case maxResults={page_size}: a value outside @range(1, 100) must be refused, got \
             {} with {:?}",
            refused.status, refused.body
        );
    }

    // A token this operation never issued is a bad request, not an empty page. The rows
    // differ in how the token fails to name a page of this list: a shape it never emits,
    // a correctly shaped token issued while listing another app environment, and a
    // correctly scoped token whose cursor was never handed out — above the highest one and
    // below the first one alike.
    let issued = list(&router, "?maxResults=1")
        .await
        .body["nextToken"]
        .as_str()
        .expect("a first page of seven components carries a nextToken")
        .to_string();
    let elsewhere = {
        for index in 0..2 {
            create_in(
                &router,
                APP_ID,
                "production",
                minimal_create(&format!("other-{index}")),
            )
            .await;
        }
        list_in(&router, APP_ID, "production", "?maxResults=1")
            .await
            .body["nextToken"]
            .as_str()
            .expect("a first page of two components carries a nextToken")
            .to_string()
    };
    let never_issued_cursor = format!("{}{}", &issued[..issued.len() - 16], "f".repeat(16));
    let cursor_below_the_first = format!("{}{}", &issued[..issued.len() - 16], "0".repeat(16));
    for token in [
        "not-a-token",
        "c-",
        "c-00000000000000zz",
        "0",
        &elsewhere,
        &never_issued_cursor,
        &cursor_below_the_first,
    ] {
        let refused = list(&router, &format!("?nextToken={token}")).await;
        assert_eq!(
            refused.status, 400,
            "case nextToken={token:?}: a token ListComponents never issued for this app \
             environment must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "",
            "case nextToken={token:?}: a refusal must name no error type, got {:?}",
            refused.body
        );
    }
    assert_ne!(
        issued, elsewhere,
        "a token is scoped to the app environment it was issued for, got {issued:?} for both"
    );

    // Deleting a component in the middle leaves the order of the rest, and pagination over
    // it, intact.
    let dropped = created[3].clone();
    let deleted = delete(&router, &dropped).await;
    assert_eq!(
        deleted.status, 200,
        "deleting {dropped:?} must answer an empty 2xx, got {} with {:?}",
        deleted.status, deleted.body
    );
    let remaining: Vec<String> = created
        .iter()
        .filter(|id| **id != dropped)
        .cloned()
        .collect();
    let mut walked = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(token) => format!("?maxResults=2&nextToken={token}"),
            None => "?maxResults=2".to_string(),
        };
        let answer = list(&router, &query).await;
        walked.extend(listed_ids(&answer));
        match answer.body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        walked, remaining,
        "after a delete, paging must enumerate exactly the components still stored"
    );
}

/// `UpdateComponent` merges only the members the request carries: the named member takes
/// the new value, every other member keeps what the create stored, `id` and `createdAt`
/// never move, and `modifiedAt` is set.
#[tokio::test]
async fn update_component_merges_only_the_members_the_request_carries() {
    // The baseline carries every optional member so a patch replacing one has something to
    // replace and the untouched ones have something to preserve.
    let baseline = json!({
        "name": "Baseline",
        "componentType": "View",
        "properties": {"label": {"value": "before"}},
        "variants": [{"variantValues": {"state": "default"}}],
        "overrides": {"Text": {"color": "#000000"}},
        "bindingProperties": {"image": {"type": "Data"}},
        "children": [{"componentType": "Text", "name": "Title", "properties": {}}],
        "collectionProperties": {"posts": {"model": "Post"}},
        "events": {"click": {"action": "Amplify.Navigation"}},
        "sourceId": "figma:0:0",
        "schemaVersion": "1.0",
        "tags": {"env": "test"},
    });

    let cases = vec![
        ("the name only", json!({"name": "HeroV2"}), "/name", json!("HeroV2")),
        (
            "the component type only",
            json!({"componentType": "Flex"}),
            "/componentType",
            json!("Flex"),
        ),
        (
            "replaced properties",
            json!({"properties": {"label": {"value": "after"}, "extra": {"value": "x"}}}),
            "/properties",
            json!({"label": {"value": "after"}, "extra": {"value": "x"}}),
        ),
        (
            "emptied properties",
            json!({"properties": {}}),
            "/properties",
            json!({}),
        ),
        (
            "a longer variant list",
            json!({"variants": [
                {"variantValues": {"state": "default"}},
                {"variantValues": {"state": "hover"}, "overrides": {"Text": {"color": "red"}}},
            ]}),
            "/variants",
            json!([
                {"variantValues": {"state": "default"}},
                {"variantValues": {"state": "hover"}, "overrides": {"Text": {"color": "red"}}},
            ]),
        ),
        (
            "emptied overrides",
            json!({"overrides": {}}),
            "/overrides",
            json!({}),
        ),
        (
            "replaced binding properties",
            json!({"bindingProperties": {"caption": {"type": "String", "defaultValue": "none"}}}),
            "/bindingProperties",
            json!({"caption": {"type": "String", "defaultValue": "none"}}),
        ),
        (
            "replaced children",
            json!({"children": [
                {"componentType": "Badge", "name": "Tag", "properties": {}},
                {"componentType": "Text", "name": "Body", "properties": {"label": {"value": "b"}}},
            ]}),
            "/children",
            json!([
                {"componentType": "Badge", "name": "Tag", "properties": {}},
                {"componentType": "Text", "name": "Body", "properties": {"label": {"value": "b"}}},
            ]),
        ),
        (
            "replaced collection properties",
            json!({"collectionProperties": {"posts": {"model": "Comment", "identifiers": ["id"]}}}),
            "/collectionProperties",
            json!({"posts": {"model": "Comment", "identifiers": ["id"]}}),
        ),
        (
            "replaced events",
            json!({"events": {"change": {"action": "Amplify.DataStoreUpdateItem"}}}),
            "/events",
            json!({"change": {"action": "Amplify.DataStoreUpdateItem"}}),
        ),
        (
            "a new source id",
            json!({"sourceId": "figma:9:9"}),
            "/sourceId",
            json!("figma:9:9"),
        ),
        (
            "a new schema version",
            json!({"schemaVersion": "2.0"}),
            "/schemaVersion",
            json!("2.0"),
        ),
    ];

    let router = emulator();
    for (index, (case, patch, pointer, expected)) in cases.into_iter().enumerate() {
        // A name per row: a component name identifies a component within its app
        // environment, so each row's baseline takes its own name rather than being
        // refused as a conflict (the rule `component_refusals.rs` pins).
        let mut data = baseline.clone();
        data["name"] = json!(format!("Baseline{index}"));
        let created = create(&router, data).await;
        let id = created.id();
        let before = get(&router, &id).await.body;

        let updated = update(&router, &id, patch.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateComponent, got {} with {:?}",
            updated.status, updated.body
        );

        let after = get(&router, &id).await.body;
        assert_eq!(
            at(&after, pointer),
            expected,
            "case {case:?}: {pointer} expected {expected:?}, got {:?}",
            at(&after, pointer)
        );
        assert_eq!(
            after, updated.body,
            "case {case:?}: UpdateComponent's own answer must match the stored record"
        );

        // Nothing but the named member and `modifiedAt` moved.
        let changed = pointer.trim_start_matches('/');
        let mut was: Map<String, Value> = before
            .as_object()
            .expect("a component is a JSON object")
            .clone();
        let mut is: Map<String, Value> = after
            .as_object()
            .expect("a component is a JSON object")
            .clone();
        for key in [changed, "modifiedAt"] {
            was.remove(key);
            is.remove(key);
        }
        assert_eq!(
            was, is,
            "case {case:?}: an update naming only {changed:?} changed something else"
        );

        assert_eq!(
            at(&after, "/id"),
            json!(id),
            "case {case:?}: an update must not move the component's id"
        );
        assert_eq!(
            at(&after, "/createdAt"),
            at(&before, "/createdAt"),
            "case {case:?}: an update must not move createdAt"
        );
        let created_at = epoch_seconds(&at(&after, "/createdAt"));
        let modified_at = epoch_seconds(&at(&after, "/modifiedAt"));
        assert!(
            modified_at >= created_at,
            "case {case:?}: an updated component carries a modifiedAt at or after its \
             createdAt, got {modified_at} against {created_at}"
        );
    }

    // A payload naming the component it addresses is accepted and merged, since
    // `UpdateComponentData$id` names the component being updated.
    let mut data = baseline.clone();
    data["name"] = json!("BaselineEchoed");
    let id = create(&router, data).await.id();
    let echoed = update(&router, &id, json!({"id": id, "name": "EchoedId"})).await;
    assert_eq!(
        echoed.status, 200,
        "an updatedComponent.id equal to the addressed id must be accepted, got {} with {:?}",
        echoed.status, echoed.body
    );
    assert_eq!(
        at(&echoed.body, "/name"),
        json!("EchoedId"),
        "the merge must still apply, got {:?}",
        echoed.body
    );

    // A payload naming a different component than the URI is contradictory, so it is
    // refused rather than applied either way.
    let mut data = baseline.clone();
    data["name"] = json!("Baseline");
    let id = create(&router, data).await.id();
    let refused = update(
        &router,
        &id,
        json!({"id": "00000000-0000-4000-8000-000000000000", "name": "Mismatched"}),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "an updatedComponent.id naming another component must be refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "",
        "a refusal must name no error type, got {:?}",
        refused.body
    );
    assert_eq!(
        at(&get(&router, &id).await.body, "/name"),
        json!("Baseline"),
        "a refused update must leave the stored component alone"
    );
}

/// An id that names no stored component anywhere is reported as missing, not as an empty
/// success: `GetComponent`, `DeleteComponent` and `UpdateComponent` all answer HTTP 404,
/// naming no error type. (An id that resolves under ANOTHER app environment is a different
/// failure, pinned in `component_refusals.rs`.)
#[tokio::test]
async fn an_id_naming_no_component_is_reported_as_missing() {
    let router = emulator();
    let id = create(&router, minimal_create("Doomed")).await.id();

    let never_created = "11111111-1111-4111-8111-111111111111";
    for (case, answer) in [
        ("GetComponent", get(&router, never_created).await),
        (
            "UpdateComponent",
            update(&router, never_created, json!({"name": "Nothing"})).await,
        ),
        ("DeleteComponent", delete(&router, never_created).await),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an id that was never created must answer HTTP 404, got {} with \
             {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "",
            "case {case:?}: a refusal must name no error type, got {:?}",
            answer.body
        );
    }

    // Nothing was created or removed as a side effect: a failed lookup changed nothing.
    assert_eq!(
        listed_ids(&list(&router, "").await),
        vec![id.clone()],
        "a refused lookup must not touch the store"
    );

    // A delete removes the record: the answer carries no members, and every later read of
    // that id reports it missing.
    let deleted = delete(&router, &id).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteComponent must answer an empty 2xx, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.body,
        Value::Null,
        "DeleteComponentResponse has no members, got {:?}",
        deleted.body
    );
    assert_eq!(
        get(&router, &id).await.status,
        404,
        "GetComponent must report a deleted component as missing"
    );
    assert_eq!(
        delete(&router, &id).await.status,
        404,
        "deleting an already-deleted component must report it as missing"
    );
    assert!(
        listed_ids(&list(&router, "").await).is_empty(),
        "ListComponents must not list a deleted component"
    );
}

/// `tags` is not a valid property name in any of a component's property maps, on the create
/// or on an update, and a refused request stores nothing.
#[tokio::test]
async fn a_reserved_property_name_is_refused() {
    let router = emulator();
    let reserved = RESERVED_PROPERTY_NAME;

    // One row per map the model documents the rule for; the value shape differs per map.
    let create_rows: Vec<(&str, Value)> = vec![
        ("properties", one_entry(reserved, json!({"value": "no"}))),
        ("overrides", one_entry(reserved, json!({"color": "no"}))),
        (
            "bindingProperties",
            one_entry(reserved, json!({"type": "String"})),
        ),
        (
            "collectionProperties",
            one_entry(reserved, json!({"model": "Post"})),
        ),
    ];

    for (member, value) in &create_rows {
        let mut data = minimal_create("Reserved");
        data[*member] = value.clone();
        let refused = create(&router, data).await;
        assert_eq!(
            refused.status, 400,
            "case {member:?}: a reserved property name must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "",
            "case {member:?}: a refusal must name no error type, got {:?}",
            refused.body
        );
    }
    assert!(
        listed_ids(&list(&router, "").await).is_empty(),
        "a refused create must store nothing"
    );

    // The same rule on an update, judged against the stored record it would leave behind.
    let id = create(&router, minimal_create("Reserved")).await.id();
    let stored = get(&router, &id).await.body;
    for (member, value) in &create_rows {
        let refused = update(&router, &id, one_entry(member, value.clone())).await;
        assert_eq!(
            refused.status, 400,
            "case {member:?}: an update may not introduce a reserved property name, got {} \
             with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "",
            "case {member:?}: a refusal must name no error type, got {:?}",
            refused.body
        );
    }
    assert_eq!(
        get(&router, &id).await.body,
        stored,
        "a refused update must leave the stored component alone"
    );
}

/// A `nextToken` is arbitrary caller-supplied UTF-8, so the parser must ANSWER for every
/// byte string rather than fail on one: it accepts exactly the tokens `page_token` issued
/// for that app environment, over the collection being paged, with a cursor the service
/// handed out — the span from the first cursor minted to the highest — and returns `None`
/// for everything else, including a token whose byte length is right but whose bytes are
/// not hex digits (a multi-byte scalar straddling the halves' boundary would panic a parser
/// that split before checking).
#[test]
fn a_page_token_the_service_never_issued_is_answered_not_panicked() {
    let key = (APP_ID.to_string(), ENVIRONMENT.to_string());
    let other_app = ("d9999zzzz".to_string(), ENVIRONMENT.to_string());
    let other_environment = (APP_ID.to_string(), "production".to_string());
    let paged = PagedCollection::Components;
    let issued: u64 = 7;

    // Every token the service can issue for cursors it handed out round-trips. The
    // cursors handed out start at one: the counter pre-increments, so no record and no
    // token of this service ever carries the cursor zero.
    for cursor in [1, 2, issued] {
        let token = page_token(&key, paged, cursor);
        assert_eq!(
            parse_page_token(&token, &key, paged, issued),
            Some(cursor),
            "case cursor={cursor}: the token {token:?} this service issued must parse back"
        );
    }

    // Rows differ in how the token fails: the empty string, a shape never emitted, the
    // prefix alone, non-hex bytes of the right length, a multi-byte scalar across the
    // scope/cursor boundary, one byte too few and one too many, a cursor past everything
    // issued, a cursor below the first one ever handed out, a token issued for another
    // app environment, and a token issued over another collection of this one.
    let right_length_non_hex = format!("c-{}", "z".repeat(24));
    let multibyte_across_boundary = format!("c-{}{}{}", "a".repeat(6), "€", "b".repeat(15));
    let too_short = page_token(&key, paged, 1)[..25].to_string();
    let too_long = format!("{}0", page_token(&key, paged, 1));
    let past_issued = page_token(&key, paged, issued + 1);
    let largest_cursor = page_token(&key, paged, u64::MAX);
    let below_first_issued = page_token(&key, paged, 0);
    let from_other_app = page_token(&other_app, paged, 1);
    let from_other_environment = page_token(&other_environment, paged, 1);
    // Every OTHER collection this service pages: each of their tokens names a position in
    // its own creation order, so none of them names a page of the component list.
    let from_other_collections: Vec<(String, String)> = [
        PagedCollection::Forms,
        PagedCollection::Themes,
        PagedCollection::CodegenJobs,
    ]
    .into_iter()
    .map(|collection| {
        (
            format!("a token issued over {collection:?}"),
            page_token(&key, collection, 1),
        )
    })
    .collect();
    let mut cases: Vec<(&str, String)> = vec![
        ("the empty string", String::new()),
        ("a token shape never emitted", "not-a-token".to_string()),
        ("the prefix alone", "c-".to_string()),
        ("the right length, not hex", right_length_non_hex),
        (
            "a multi-byte scalar across the halves' boundary",
            multibyte_across_boundary.clone(),
        ),
        ("one hex digit too few", too_short),
        ("one hex digit too many", too_long),
        ("a cursor past everything issued", past_issued),
        ("the largest cursor a token can spell", largest_cursor),
        ("a cursor below the first one handed out", below_first_issued),
        ("a token issued for another app", from_other_app),
        (
            "a token issued for another environment",
            from_other_environment,
        ),
    ];
    cases.extend(
        from_other_collections
            .iter()
            .map(|(case, token)| (case.as_str(), token.clone())),
    );

    for (case, token) in cases {
        assert_eq!(
            parse_page_token(&token, &key, paged, issued),
            None,
            "case {case:?}: the token {token:?} names no page of this list"
        );
    }

    // The span is empty while nothing has been created, so before the first create even a
    // correctly scoped token of the lowest cursor names no page.
    for cursor in [0, 1, 2] {
        let token = page_token(&key, paged, cursor);
        assert_eq!(
            parse_page_token(&token, &key, paged, 0),
            None,
            "case cursor={cursor} with nothing handed out: the token {token:?} names no page"
        );
    }

    // The multi-byte row is the one that would panic a parser splitting before checking:
    // state the byte layout it relies on, so the case cannot decay into an ASCII one.
    assert_eq!(
        multibyte_across_boundary.len(),
        2 + 24,
        "the hostile token must be exactly as long as a real one, got {:?}",
        multibyte_across_boundary
    );
    assert!(
        !multibyte_across_boundary.is_char_boundary(2 + 8),
        "the hostile token must carry a scalar across the halves' boundary, got {:?}",
        multibyte_across_boundary
    );
}
