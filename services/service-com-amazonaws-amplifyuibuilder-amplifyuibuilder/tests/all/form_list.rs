// SPDX-License-Identifier: CC0-1.0

//! `ListForms`, end to end over HTTP.
//!
//! The rule under test: a list of an app environment reports the `FormSummary`
//! projection — appId, dataType, environmentName, formActionType, id and name, and
//! nothing else — of every form stored for the addressed (appId, environmentName) and of
//! nothing else, in the service's one creation order; it changes nothing, a page carries
//! at most as many summaries as `maxResults` asked for, and following `nextToken`
//! enumerates each stored form exactly once.
//!
//! `smithy_query(query="operation", operation="ListForms")` and
//! `smithy_query(query="shape", shape="FormSummary,ListEntityLimit")` are where that
//! comes from: the operation is `@readonly` and `@paginated(inputToken: nextToken,
//! outputToken: nextToken, pageSize: maxResults, items: entities)`, binds
//! `appId`/`environmentName` as `@httpLabel` and `maxResults`/`nextToken` as
//! `@httpQuery`, `entities` targets a list of `FormSummary` whose six `@required`
//! members are appId, dataType, environmentName, formActionType, id and name, and
//! `maxResults` targets `ListEntityLimit`, `@range(min: 1, max: 100)`.
//!
//! Each case is driven through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment the cases address, unless a case is about addressing
/// another one.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The published bounds of `ListEntityLimit`, the shape `ListFormsRequest$maxResults`
/// targets (`@range(min: 1, max: 100)`). They are spelled out here rather than imported
/// so a service that admits a value the model refuses, or refuses one it admits, fails
/// this test.
const MAX_RESULTS_FLOOR: i32 = 1;
const MAX_RESULTS_CEILING: i32 = 100;

/// The members `FormSummary` declares, every one of them `@required`: a listed entity
/// carries exactly these and no part of the full `Form` record.
const SUMMARY_MEMBERS: [&str; 6] = [
    "appId",
    "dataType",
    "environmentName",
    "formActionType",
    "id",
    "name",
];

/// Every value `FormActionType` admits, from
/// `smithy_query(query="shape", shape="FormActionType")`.
const FORM_ACTION_TYPES: [&str; 2] = ["create", "update"];

/// Every value `FormDataSourceType` admits, from
/// `smithy_query(query="shape", shape="FormDataSourceType")`.
const FORM_DATA_SOURCE_TYPES: [&str; 2] = ["DataStore", "Custom"];

/// The longest `FormName` the model admits (`@length(min: 1, max: 255)` on
/// `FormSummary$name`, from `smithy_query(query="shape", shape="FormSummary")`).
const NAME_MAX_LENGTH: usize = 255;

/// How many hex digits the cursor half of a page token carries, as this service spells a
/// token (`state::PAGE_TOKEN_HEX_WIDTH`). The refusal cases below rewrite exactly that
/// half, so the width is named here and the token is checked against it rather than the
/// slice being taken on faith.
const TOKEN_CURSOR_HEX_WIDTH: usize = 16;

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
    /// The form id a `Form` payload carries, or the empty string.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }

    /// The error-type discriminator the body carries — `__type`, or the `code` / `Code`
    /// spellings the same JSON error parsers also accept — or the empty string when it
    /// carries none. AmplifyUIBuilder names no exception on the wire, so every refusal
    /// must carry none of them.
    fn error_code(&self) -> &str {
        ["__type", "code", "Code"]
            .iter()
            .find_map(|member| self.body[*member].as_str())
            .unwrap_or_default()
    }

    /// The `entities` the answer carries, as a list of JSON values.
    fn entities(&self) -> Vec<Value> {
        self.body["entities"]
            .as_array()
            .unwrap_or_else(|| panic!("a list must carry an entities list, got {:?}", self.body))
            .clone()
    }

    /// The ids the answer's `entities` carry, in the order they were reported.
    fn entity_ids(&self) -> Vec<String> {
        self.entities()
            .iter()
            .map(|entity| entity["id"].as_str().unwrap_or_default().to_string())
            .collect()
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

/// The `@http` URI `CreateForm` and `ListForms` share.
fn forms_uri(app_id: &str, environment: &str) -> String {
    format!("/app/{app_id}/environment/{environment}/forms")
}

async fn create_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    data: Value,
) -> Answer {
    call(router, "POST", &forms_uri(app_id, environment), Some(data)).await
}

async fn create(router: &MultiServiceRouter, data: Value) -> Answer {
    create_in(router, APP_ID, ENVIRONMENT, data).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("{}/{id}", forms_uri(APP_ID, ENVIRONMENT));
    call(router, "GET", &uri, None).await
}

async fn delete(router: &MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("{}/{id}", forms_uri(APP_ID, ENVIRONMENT));
    call(router, "DELETE", &uri, None).await
}

async fn list_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    query: &str,
) -> Answer {
    let uri = format!("{}{query}", forms_uri(app_id, environment));
    call(router, "GET", &uri, None).await
}

async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    list_in(router, APP_ID, ENVIRONMENT, query).await
}

/// The members `CreateFormData` requires, as a starting point for a case that is about
/// something other than those values.
fn minimal_create(name: &str) -> Value {
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

/// The create payloads the list cases drive, differing along what the rule generalises
/// over: which enumerated value each summarised member carries, how many `fields` and
/// `sectionalElements` entries there are, whether the optional `cta`, `tags` and
/// `labelDecorator` are present at all, and how long the name is — none of which a
/// summary reports beyond the six members it declares.
fn create_cases() -> Vec<(&'static str, Value)> {
    let mut cases = vec![
        ("only the required members", minimal_create("Blank")),
        (
            "two fields, a style and a full CTA",
            json!({
                "name": "PostCreateForm",
                "formActionType": "create",
                "dataType": {"dataSourceType": "DataStore", "dataTypeName": "Post"},
                "fields": {
                    "title": {
                        "label": "Title",
                        "position": {"fixed": "first"},
                        "inputType": {"type": "TextField", "required": true},
                    },
                    "rating": {"label": "Rating", "inputType": {"type": "SliderField"}},
                },
                "style": {"horizontalGap": {"value": "16px"}},
                "sectionalElements": {"heading": {"type": "Heading", "level": 1, "text": "Post"}},
                "schemaVersion": "1.0",
                "cta": {
                    "position": "top_and_bottom",
                    "submit": {"excluded": false, "children": "Save"},
                },
            }),
        ),
        (
            "tags, a label decorator and a custom data source",
            json!({
                "name": "OrderUpdateForm",
                "formActionType": "update",
                "dataType": {"dataSourceType": "Custom", "dataTypeName": "Order"},
                "fields": {},
                "style": {},
                "sectionalElements": {},
                "schemaVersion": "2.1",
                "tags": {"env": "test", "team": "studio"},
                "labelDecorator": "required",
            }),
        ),
        (
            // `FormName` is `@length(min: 1, max: 255)`, so a name on its upper bound is
            // one a summary has to report whole.
            "a name at the @length max of FormName",
            minimal_create(&"n".repeat(NAME_MAX_LENGTH)),
        ),
        ("the shortest name the model admits", minimal_create("f")),
    ];

    // One row per published value of each enumerated member a summary reports, so the
    // catalog is covered rather than only the value one request happened to carry.
    for action in FORM_ACTION_TYPES {
        let mut data = minimal_create(&format!("ActionTyped-{action}"));
        data["formActionType"] = json!(action);
        cases.push(("every formActionType the model admits", data));
    }
    for source in FORM_DATA_SOURCE_TYPES {
        let mut data = minimal_create(&format!("SourceTyped-{source}"));
        data["dataType"] = json!({"dataSourceType": source, "dataTypeName": "Asset"});
        cases.push(("every dataSourceType the model admits", data));
    }

    // And a form whose maps grow: a summary reports none of their entries either.
    let mut fields = Map::new();
    let mut elements = Map::new();
    for index in 0..7 {
        fields.insert(format!("field{index}"), json!({"label": format!("F{index}")}));
        elements.insert(format!("element{index}"), json!({"type": "Divider"}));
    }
    let mut wide = minimal_create("WideForm");
    wide["fields"] = Value::Object(fields);
    wide["sectionalElements"] = Value::Object(elements);
    cases.push(("seven fields and seven sectional elements", wide));

    cases
}

/// A list reports the six-member `FormSummary` of every form of the addressed app
/// environment, in creation order — and of no other app environment's forms.
#[tokio::test]
async fn list_forms_reports_the_summary_of_every_stored_form() {
    let router = emulator();

    // An app environment nothing was created in lists an empty page and no token:
    // `entities` is `@required`, so it is `[]` rather than an omitted member.
    let empty = list(&router, "").await;
    assert_eq!(
        empty.status, 200,
        "case empty environment: expected HTTP 200, got {} with {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.entities(),
        Vec::<Value>::new(),
        "case empty environment: expected an empty entities list, got {:?}",
        empty.body
    );
    assert_eq!(
        empty.next_token(),
        None,
        "case empty environment: a page that ends the list must omit nextToken, got {:?}",
        empty.body
    );

    let mut created: Vec<(&'static str, String, Value)> = Vec::new();
    for (case, data) in create_cases() {
        let answer = create(&router, data.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from CreateForm, got {} with {:?}",
            answer.status, answer.body
        );
        created.push((case, answer.id(), data));
    }

    // Forms that belong to another environment of the same app, and to another app:
    // neither is part of this list.
    let other_environment = create_in(&router, APP_ID, "production", minimal_create("Elsewhere"))
        .await
        .id();
    let other_app = create_in(&router, "d9999zzzz", ENVIRONMENT, minimal_create("Elsewhere"))
        .await
        .id();

    let listed = list(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "expected HTTP 200 from ListForms, got {} with {:?}",
        listed.status, listed.body
    );
    let expected_ids: Vec<String> = created.iter().map(|(_, id, _)| id.clone()).collect();
    assert_eq!(
        listed.entity_ids(),
        expected_ids,
        "a list must report every form of that app environment once, in creation order, got {:?}",
        listed.entity_ids()
    );
    assert!(
        !listed.entity_ids().contains(&other_environment)
            && !listed.entity_ids().contains(&other_app),
        "a list must report only the addressed (appId, environmentName), got {:?}",
        listed.entity_ids()
    );

    // Each entity is the SUMMARY projection of the stored record: the six members the
    // model declares, each read off what the create persisted, and no other member.
    for (index, (case, id, create_data)) in created.iter().enumerate() {
        let entity = &listed.entities()[index];
        let read = get(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetForm, got {} with {:?}",
            read.status, read.body
        );
        let expected = json!({
            "appId": APP_ID,
            "dataType": create_data["dataType"],
            "environmentName": ENVIRONMENT,
            "formActionType": create_data["formActionType"],
            "id": id,
            "name": create_data["name"],
        });
        assert_eq!(
            entity, &expected,
            "case {case:?}: a listed entity must be the FormSummary of the stored form, got \
             {entity:?}"
        );
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                entity[member], read.body[member],
                "case {case:?}: the summary's {member} must be what GetForm reports, got {:?}",
                entity[member]
            );
        }
        let mut members: Vec<String> = entity
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: a listed entity must be an object"))
            .keys()
            .cloned()
            .collect();
        members.sort();
        let mut declared: Vec<String> = SUMMARY_MEMBERS.iter().map(|m| m.to_string()).collect();
        declared.sort();
        assert_eq!(
            members, declared,
            "case {case:?}: a summary carries only the members FormSummary declares, got \
             {members:?}"
        );
    }

    // The list of the other app environment reports that environment's own form, and only
    // it.
    let elsewhere = list_in(&router, APP_ID, "production", "").await;
    assert_eq!(
        elsewhere.entity_ids(),
        vec![other_environment.clone()],
        "case appId={APP_ID:?} environmentName=\"production\": expected only that environment's \
         form, got {:?}",
        elsewhere.entity_ids()
    );

    // `ListForms` is `@readonly`: listing twice reports the same thing, and leaves what a
    // later read reports untouched.
    let before_read = get(&router, &created[1].1).await.body.clone();
    let again = list(&router, "").await;
    assert_eq!(
        again.body, listed.body,
        "a second list must report the same thing as the first, got {:?}",
        again.body
    );
    assert_eq!(
        get(&router, &created[1].1).await.body,
        before_read,
        "a list must not change what a later GetForm reports"
    );
}

/// `ListForms` is `@paginated(pageSize: maxResults, items: entities)`: whatever
/// `maxResults` it is given, following `nextToken` to the end enumerates every form
/// stored for that app environment exactly once in one stable order, and no page is
/// longer than was asked for.
#[tokio::test]
async fn list_forms_pages_over_one_stable_order() {
    let router = emulator();
    let mut created = Vec::new();
    for index in 0..7 {
        created.push(
            create(&router, minimal_create(&format!("page-{index}")))
                .await
                .id(),
        );
    }

    // Both published bounds of `maxResults`, the sizes between that cut the seven forms
    // into a different number of pages, and no `maxResults` at all — which takes the
    // largest page the model allows.
    for page_size in [
        Some(MAX_RESULTS_FLOOR),
        Some(2),
        Some(3),
        Some(6),
        Some(7),
        Some(MAX_RESULTS_CEILING),
        None,
    ] {
        let mut walked = Vec::new();
        let mut pages = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match (page_size, &token) {
                (Some(size), Some(token)) => format!("?maxResults={size}&nextToken={token}"),
                (Some(size), None) => format!("?maxResults={size}"),
                (None, Some(token)) => format!("?nextToken={token}"),
                (None, None) => String::new(),
            };
            let answer = list(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={page_size:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let page = answer.entity_ids();
            let asked_for = page_size.unwrap_or(MAX_RESULTS_CEILING) as usize;
            assert!(
                page.len() <= asked_for,
                "case maxResults={page_size:?}: a page carried {} forms, more than the \
                 {asked_for} asked for: {page:?}",
                page.len()
            );
            pages.push(page.len());
            walked.extend(page);
            assert!(
                pages.len() <= created.len() + 1,
                "case maxResults={page_size:?}: pagination did not terminate, page lengths \
                 {pages:?}"
            );
            match answer.next_token() {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, created,
            "case maxResults={page_size:?}: walking every page must enumerate every form once, \
             in the same order, got {walked:?}"
        );
        // A full page followed by nothing still ends the walk, so the page count is the
        // number of pages the asked-for size cuts the list into.
        let asked_for = page_size.unwrap_or(MAX_RESULTS_CEILING) as usize;
        let expected_pages = created.len().div_euclid(asked_for)
            + usize::from(created.len() % asked_for != 0 || created.is_empty());
        assert_eq!(
            pages.len(),
            expected_pages,
            "case maxResults={page_size:?}: expected {expected_pages} page(s) of at most \
             {asked_for}, got page lengths {pages:?}"
        );
    }

    // A page that ends the list carries no token.
    let whole = list(&router, &format!("?maxResults={MAX_RESULTS_CEILING}")).await;
    assert_eq!(
        whole.next_token(),
        None,
        "the last page must omit nextToken, got {:?}",
        whole.body
    );

    // One past each published bound of `maxResults` is refused at the boundary.
    for page_size in [MAX_RESULTS_FLOOR - 1, MAX_RESULTS_CEILING + 1] {
        let refused = list(&router, &format!("?maxResults={page_size}")).await;
        assert_eq!(
            refused.status, 400,
            "case maxResults={page_size}: a value outside @range(1, 100) must be refused, got {} \
             with {:?}",
            refused.status, refused.body
        );
    }

    // Deleting a form in the middle leaves the order of the rest, and pagination over it,
    // intact.
    let dropped = created[3].clone();
    let deleted = delete(&router, &dropped).await;
    assert_eq!(
        deleted.status, 200,
        "deleting {dropped:?} must answer an empty 2xx, got {} with {:?}",
        deleted.status, deleted.body
    );
    let remaining: Vec<String> = created.iter().filter(|id| **id != dropped).cloned().collect();
    let mut walked = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(token) => format!("?maxResults=2&nextToken={token}"),
            None => "?maxResults=2".to_string(),
        };
        let answer = list(&router, &query).await;
        walked.extend(answer.entity_ids());
        match answer.next_token() {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(
        walked, remaining,
        "after a delete, paging must enumerate exactly the forms still stored, got {walked:?}"
    );
}

/// A `nextToken` this service never issued for the addressed app environment names no
/// page of this list, so it is refused with the modelled `InvalidParameterException`
/// (HTTP 400) rather than served as a page.
#[tokio::test]
async fn list_forms_refuses_a_token_it_never_issued() {
    let router = emulator();
    for index in 0..3 {
        create(&router, minimal_create(&format!("token-{index}"))).await;
    }
    for index in 0..3 {
        create_in(
            &router,
            APP_ID,
            "production",
            minimal_create(&format!("other-{index}")),
        )
        .await;
    }

    let issued = list(&router, "?maxResults=1")
        .await
        .next_token()
        .expect("a first page of one of three forms carries a nextToken");
    let elsewhere = list_in(&router, APP_ID, "production", "?maxResults=1")
        .await
        .next_token()
        .expect("a first page of one of three forms carries a nextToken");
    assert_ne!(
        issued, elsewhere,
        "a page token is scoped to the app environment it was issued for, got {issued:?} for both"
    );

    // A token this list did issue for this app environment is accepted.
    let resumed = list(&router, &format!("?maxResults=1&nextToken={issued}")).await;
    assert_eq!(
        resumed.status, 200,
        "case nextToken={issued:?}: a token this list issued must be accepted, got {} with {:?}",
        resumed.status, resumed.body
    );

    // The rows differ in how the token fails to name a page of this list: a shape the
    // service never emits, a token issued while listing another app environment, and a
    // correctly scoped token whose cursor was never handed out — above the highest one
    // handed out and below the first one alike.
    assert!(
        issued.len() > TOKEN_CURSOR_HEX_WIDTH
            && issued.is_char_boundary(issued.len() - TOKEN_CURSOR_HEX_WIDTH),
        "a page token must carry a {TOKEN_CURSOR_HEX_WIDTH}-digit cursor half after its scope \
         half, got {issued:?}"
    );
    let scope_half = &issued[..issued.len() - TOKEN_CURSOR_HEX_WIDTH];
    let never_issued_cursor = format!("{scope_half}{}", "f".repeat(TOKEN_CURSOR_HEX_WIDTH));
    let cursor_below_the_first = format!("{scope_half}{}", "0".repeat(TOKEN_CURSOR_HEX_WIDTH));
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
            "case nextToken={token:?}: a token ListForms never issued for this app environment \
             must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "",
            "case nextToken={token:?}: a refusal must name no error type, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["entities"],
            Value::Null,
            "case nextToken={token:?}: a refusal must carry no page, got {:?}",
            refused.body
        );
    }
}
