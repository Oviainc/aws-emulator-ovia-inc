// SPDX-License-Identifier: CC0-1.0

//! Which cursors a `nextToken` of this service's paginated operations may carry, end to
//! end over HTTP.
//!
//! The rule under test: a page token names a page only when the cursor it carries is one
//! the service actually handed out — the cursors in existence run from the first one ever
//! minted up to the highest — so a well-formed, correctly scoped token whose cursor lies
//! outside that span is refused with the modelled `InvalidParameterException` (HTTP 400),
//! whether it lies above the highest cursor or below the first one.
//!
//! All four paginated operations of this crate the cases drive (`ListComponents`,
//! `ExportComponents`, `ExportForms`, `ExportThemes`) share one token validator, so each
//! case here is driven against every one of them, each with a token of the collection it
//! pages: the cursor bound is a property of the token, not of the operation that reads it.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.

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

/// How a page token of this service is spelled: a literal prefix, then the app-environment
/// digest, then the cursor, both halves in hex. Spelled out here rather than imported, and
/// checked below against a token the service really issued, so a token shape the service
/// does not emit cannot make these cases pass.
const TOKEN_PREFIX: &str = "c-";
const SCOPE_HEX_WIDTH: usize = 8;
const CURSOR_HEX_WIDTH: usize = 16;

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
    /// The error-type discriminator the body carries — `__type`, or the `code` / `Code`
    /// spellings the same JSON error parsers also accept — or the empty string when it
    /// carries none. AmplifyUIBuilder names no exception on the wire, so every refusal of
    /// one of this service's modelled exceptions must carry none of them.
    fn error_code(&self) -> &str {
        ["__type", "code", "Code"]
            .iter()
            .find_map(|member| self.body[*member].as_str())
            .unwrap_or_default()
    }

    /// How many `entities` the answer carries, or `None` when it carries no list at all.
    fn entity_count(&self) -> Option<usize> {
        self.body["entities"].as_array().map(Vec::len)
    }

    /// The `nextToken` the answer carries, if any.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
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

/// The create payload of each entity kind: the members its `Create*Data` requires, which
/// is all a case about page tokens needs.
fn minimal_component(name: &str) -> Value {
    json!({
        "name": name,
        "componentType": "View",
        "properties": {},
        "variants": [],
        "overrides": {},
        "bindingProperties": {},
    })
}

fn minimal_form(name: &str) -> Value {
    json!({
        "name": name,
        "formActionType": "create",
        "dataType": {"dataSourceType": "DataStore", "dataTypeName": "Post"},
        "fields": {},
        "style": {},
        "sectionalElements": {},
        "schemaVersion": "1.0",
    })
}

fn minimal_theme(name: &str) -> Value {
    json!({"name": name, "values": []})
}

/// Where an entity kind of this app environment is created and listed.
fn collection_uri(kind: &str) -> String {
    format!("/app/{APP_ID}/environment/{ENVIRONMENT}/{kind}")
}

/// Where an entity kind of this app environment is exported: the `@http` URI the
/// `Export*` operations are bound to.
fn export_uri(kind: &str) -> String {
    format!("/export/app/{APP_ID}/environment/{ENVIRONMENT}/{kind}")
}

/// Create one entity of `kind` and insist the service stored it: every case here needs a
/// store that has handed out cursors.
async fn create(router: &MultiServiceRouter, kind: &str, data: Value) {
    let created = call(router, "POST", &collection_uri(kind), Some(data)).await;
    assert_eq!(
        created.status, 200,
        "case create {kind}: creating the fixture must answer HTTP 200, got {} with {:?}",
        created.status, created.body
    );
}

/// The operations a page token can be presented to, each with the `@http` URI it is bound
/// to and the collection listing whose issued tokens that operation honours — a token names
/// a page of ONE collection, so each row is driven with a token of the collection it pages.
struct Paginated {
    operation: &'static str,
    uri: String,
    collection: &'static str,
}

fn paginated_operations() -> Vec<Paginated> {
    vec![
        Paginated {
            operation: "ListComponents",
            uri: collection_uri("components"),
            collection: "components",
        },
        Paginated {
            operation: "ExportComponents",
            uri: export_uri("components"),
            collection: "components",
        },
        Paginated {
            operation: "ExportForms",
            uri: export_uri("forms"),
            collection: "forms",
        },
        Paginated {
            operation: "ExportThemes",
            uri: export_uri("themes"),
            collection: "themes",
        },
    ]
}

/// A token spelled exactly as the service spells one — same prefix, same app-environment
/// digest as `issued` — carrying `cursor`.
fn forge(issued: &str, cursor: u64) -> String {
    let scope = &issued[TOKEN_PREFIX.len()..TOKEN_PREFIX.len() + SCOPE_HEX_WIDTH];
    format!("{TOKEN_PREFIX}{scope}{cursor:0width$x}", width = CURSOR_HEX_WIDTH)
}

/// The cursor the token `issued` carries.
fn cursor_of(issued: &str) -> u64 {
    let digits = &issued[issued.len() - CURSOR_HEX_WIDTH..];
    u64::from_str_radix(digits, 16).unwrap_or_else(|error| {
        panic!("the token {issued:?} must end in {CURSOR_HEX_WIDTH} hex digits: {error}")
    })
}

/// Walk the list of `collection` one record at a time, returning every token it issued:
/// each one is a token the service really handed out over that collection, and the last
/// carries the newest cursor of the store when that collection's records were the last
/// entities created.
async fn issued_tokens(
    router: &MultiServiceRouter,
    collection: &str,
    pages_at_most: usize,
) -> Vec<String> {
    let list_uri = collection_uri(collection);
    let mut tokens: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let uri = match &token {
            Some(token) => format!("{list_uri}?maxResults=1&nextToken={token}"),
            None => format!("{list_uri}?maxResults=1"),
        };
        let page = call(router, "GET", &uri, None).await;
        assert_eq!(
            page.status, 200,
            "case {collection} nextToken={token:?}: a token the list issued must be served, got \
             {} with {:?}",
            page.status, page.body
        );
        match page.next_token() {
            Some(next) => {
                tokens.push(next.clone());
                token = Some(next);
            }
            None => break,
        }
        assert!(
            tokens.len() < pages_at_most,
            "walking the {collection} one at a time must issue fewer than {pages_at_most} \
             tokens, got {tokens:?}"
        );
    }
    tokens
}

/// A token whose cursor is outside the span the service hands out is refused by every
/// paginated operation, while one carrying a cursor it did hand out — the FIRST one as
/// much as the highest — is served.
///
/// The rows differ in where the cursor sits relative to that span: below its floor (the
/// cursor zero, which the pre-incrementing counter never mints), at the floor, at the
/// highest cursor handed out, one past it, and at the largest `u64` a token can spell.
#[tokio::test]
async fn a_cursor_outside_the_span_handed_out_is_refused_by_every_paginated_operation() {
    let router = emulator();

    // Themes and forms first, then components, so the newest cursor the store has handed
    // out belongs to a component and the walk below can read it off an issued token.
    for index in 0..2 {
        create(&router, "themes", minimal_theme(&format!("theme-{index}"))).await;
        create(&router, "forms", minimal_form(&format!("form-{index}"))).await;
    }
    let components = 4;
    for index in 0..components {
        create(
            &router,
            "components",
            minimal_component(&format!("component-{index}")),
        )
        .await;
    }

    let walked = issued_tokens(&router, "components", components).await;
    let issued = walked
        .last()
        .unwrap_or_else(|| panic!("a walk of {components} components must issue a token"))
        .clone();
    assert_eq!(
        issued.len(),
        TOKEN_PREFIX.len() + SCOPE_HEX_WIDTH + CURSOR_HEX_WIDTH,
        "case issued token: the service must spell a token as prefix, scope and cursor, got \
         {issued:?}"
    );
    let highest = cursor_of(&issued);
    assert_eq!(
        forge(&issued, highest),
        issued,
        "case issued token: re-spelling the cursor of {issued:?} must reproduce it exactly"
    );
    assert!(
        highest >= u64::try_from(components).expect("the fixture count fits in u64"),
        "case issued token: the newest cursor must count every entity created, got {highest} \
         after four other entities and {components} components"
    );

    // One token each of the other two collections' lists really issued, so every row below
    // is driven with a token of the collection its operation pages: a token names a page of
    // ONE collection, and the scope digest it carries is what says which.
    let fixtures_per_collection = 2;
    let form_token = issued_tokens(&router, "forms", fixtures_per_collection)
        .await
        .last()
        .unwrap_or_else(|| panic!("a walk of {fixtures_per_collection} forms must issue a token"))
        .clone();
    let theme_token = issued_tokens(&router, "themes", fixtures_per_collection)
        .await
        .last()
        .unwrap_or_else(|| panic!("a walk of {fixtures_per_collection} themes must issue a token"))
        .clone();
    let token_of = |collection: &str| -> String {
        match collection {
            "components" => issued.clone(),
            "forms" => form_token.clone(),
            "themes" => theme_token.clone(),
            other => panic!("no fixture token for the {other} collection"),
        }
    };

    // The floor of the span: the cursor counter pre-increments, so the first cursor handed
    // out is one and zero names no record. Spelled out here, independently of the constant
    // `src/state.rs` carries, so a wrong floor in the implementation fails this test rather
    // than moving with it.
    let first = 1;

    // Rows: where the cursor sits, its value, and whether the service handed it out.
    let cases: Vec<(&str, u64, bool)> = vec![
        ("below the first cursor ever minted", 0, false),
        ("the first cursor handed out", first, true),
        ("the highest cursor handed out", highest, true),
        ("one past the highest handed out", highest + 1, false),
        ("the largest cursor a token can spell", u64::MAX, false),
    ];

    for Paginated {
        operation,
        uri,
        collection,
    } in paginated_operations()
    {
        let scope_source = token_of(collection);
        for (case, cursor, handed_out) in &cases {
            let token = forge(&scope_source, *cursor);
            let answer = call(&router, "GET", &format!("{uri}?nextToken={token}"), None).await;
            if *handed_out {
                assert_eq!(
                    answer.status, 200,
                    "case {operation} / {case} (cursor {cursor}): a cursor the service handed \
                     out must be served, got {} with {:?}",
                    answer.status, answer.body
                );
                assert!(
                    answer.entity_count().is_some(),
                    "case {operation} / {case} (cursor {cursor}): a served page must carry an \
                     entities list, got {:?}",
                    answer.body
                );
            } else {
                assert_eq!(
                    answer.status, 400,
                    "case {operation} / {case} (cursor {cursor}): a token the service never \
                     issued must be refused, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.entity_count(),
                    None,
                    "case {operation} / {case} (cursor {cursor}): a refusal must carry no \
                     entities, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "",
                    "case {operation} / {case} (cursor {cursor}): a refusal must name no error \
                     type, got {:?}",
                    answer.body
                );
                let message = answer.body["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(operation) && message.contains(&token),
                    "case {operation} / {case} (cursor {cursor}): the refusal must name the \
                     operation and the token it refuses, got {message:?}"
                );
            }
        }
    }

    // Refusing the cursors outside the span changes nothing for a caller walking the
    // tokens the service issued: every one of them is still served as a page.
    let list_uri = collection_uri("components");
    for token in &walked {
        let answer = call(
            &router,
            "GET",
            &format!("{list_uri}?maxResults=1&nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case issued token {token:?}: a token the service handed out must still be served, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.entity_count(),
            Some(1),
            "case issued token {token:?}: a page of maxResults=1 must carry one entity, got {:?}",
            answer.body
        );
    }
}

/// The span of cursors a token may carry is the one the STORE has handed out, so its
/// ceiling moves as entities are created while its floor does not: a cursor just past the
/// newest one is refused until a create mints it, and the cursor below the floor stays
/// refused however much the store holds.
#[tokio::test]
async fn the_span_a_token_may_carry_follows_what_the_store_handed_out() {
    let router = emulator();
    let list_uri = collection_uri("components");

    create(&router, "components", minimal_component("first")).await;
    create(&router, "components", minimal_component("second")).await;
    let issued = call(&router, "GET", &format!("{list_uri}?maxResults=1"), None)
        .await
        .next_token()
        .expect("a first page of two components carries a nextToken");
    let highest = cursor_of(&issued);
    let beyond = forge(&issued, highest + 1);

    // One past the newest cursor: refused now, because no record holds it yet.
    let refused = call(
        &router,
        "GET",
        &format!("{list_uri}?nextToken={beyond}"),
        None,
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case cursor={} before it is minted: a cursor the store has not handed out must be \
         refused, got {} with {:?}",
        highest + 1,
        refused.status,
        refused.body
    );

    // The next create mints exactly that cursor, and the same token is then a page.
    create(&router, "components", minimal_component("third")).await;
    let served = call(
        &router,
        "GET",
        &format!("{list_uri}?nextToken={beyond}"),
        None,
    )
    .await;
    assert_eq!(
        served.status, 200,
        "case cursor={} once minted: a cursor the store has handed out must be served, got {} \
         with {:?}",
        highest + 1,
        served.status,
        served.body
    );
    assert_eq!(
        served.entity_count(),
        Some(1),
        "case cursor={} once minted: the page must resume at the component that cursor names, \
         got {:?}",
        highest + 1,
        served.body
    );

    // The floor does not move: zero is a cursor no create ever mints.
    let below = forge(&issued, 0);
    let refused = call(
        &router,
        "GET",
        &format!("{list_uri}?nextToken={below}"),
        None,
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case cursor=0 with three components stored: the cursor below the first one ever minted \
         must stay refused, got {} with {:?}",
        refused.status, refused.body
    );
}
