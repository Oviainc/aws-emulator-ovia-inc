// SPDX-License-Identifier: CC0-1.0

//! `ExportForms`, end to end over HTTP.
//!
//! The rule under test: an export of an app environment reports the FULL `Form` record —
//! the very record `GetForm` serves, not the `FormSummary` projection `ListForms` serves —
//! of every form stored for the addressed (appId, environmentName) and of nothing else, in
//! the service's one creation order; it reports whatever the create or a later update
//! stored and changes nothing, and following `nextToken` enumerates each stored form
//! exactly once.
//!
//! `smithy_query(query="operation", operation="ExportForms")` is where that comes from:
//! the operation is `@readonly`, `@paginated(inputToken: nextToken, outputToken: nextToken,
//! items: entities)`, binds `appId`/`environmentName` as `@httpLabel` and `nextToken` as
//! `@httpQuery`, and its `entities` member targets `FormList` (a list of `Form`) where
//! `ListForms$entities` targets `FormSummaryList`.
//!
//! Each case is driven through the router, so it exercises routing, deserialization, the
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

/// The app and backend environment the cases address, unless a case is about addressing
/// another one.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The longest export page this emulator serves (`EXPORT_PAGE_LIMIT` in `src/state.rs`,
/// set to the maximum of the model's `ListEntityLimit`, `@range(min: 1, max: 100)`).
/// `ExportFormsRequest` models no page-size member, so this bound is the service's own; it
/// is spelled out here rather than imported so a page longer than the operation documents
/// fails this test.
const EXPORT_PAGE_LIMIT: usize = 100;

/// Every value `FormActionType` admits, from
/// `smithy_query(query="shape", shape="FormActionType")`.
const FORM_ACTION_TYPES: [&str; 2] = ["create", "update"];

/// Every value `FormDataSourceType` admits, from
/// `smithy_query(query="shape", shape="FormDataSourceType")`.
const FORM_DATA_SOURCE_TYPES: [&str; 2] = ["DataStore", "Custom"];

/// Every value `LabelDecorator` admits, from
/// `smithy_query(query="shape", shape="LabelDecorator")`.
const LABEL_DECORATORS: [&str; 3] = ["required", "optional", "none"];

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

    /// The `entities` the answer carries, as a list of JSON values.
    fn entities(&self) -> Vec<Value> {
        self.body["entities"]
            .as_array()
            .unwrap_or_else(|| panic!("an export must carry an entities list, got {:?}", self.body))
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

fn forms_uri(app_id: &str, environment: &str) -> String {
    format!("/app/{app_id}/environment/{environment}/forms")
}

/// The `@http` URI `ExportForms` is bound to.
fn export_uri(app_id: &str, environment: &str) -> String {
    format!("/export/app/{app_id}/environment/{environment}/forms")
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

async fn update(router: &MultiServiceRouter, id: &str, patch: Value) -> Answer {
    let uri = format!("{}/{id}", forms_uri(APP_ID, ENVIRONMENT));
    call(router, "PATCH", &uri, Some(patch)).await
}

async fn delete(router: &MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("{}/{id}", forms_uri(APP_ID, ENVIRONMENT));
    call(router, "DELETE", &uri, None).await
}

async fn export_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    token: Option<&str>,
) -> Answer {
    let uri = match token {
        Some(token) => format!("{}?nextToken={token}", export_uri(app_id, environment)),
        None => export_uri(app_id, environment),
    };
    call(router, "GET", &uri, None).await
}

async fn export(router: &MultiServiceRouter) -> Answer {
    export_in(router, APP_ID, ENVIRONMENT, None).await
}

/// Every entity of a complete walk of the export, following `nextToken` to the end, with
/// the length of each page it took.
async fn walk_export(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
) -> (Vec<Value>, Vec<usize>) {
    let mut entities = Vec::new();
    let mut pages = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let answer = export_in(router, app_id, environment, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case nextToken={token:?}: expected HTTP 200 from ExportForms, got {} with {:?}",
            answer.status, answer.body
        );
        let page = answer.entities();
        pages.push(page.len());
        entities.extend(page);
        assert!(
            pages.len() <= 64,
            "ExportForms pagination did not terminate: page lengths {pages:?}"
        );
        match answer.next_token() {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    (entities, pages)
}

/// The seven members `CreateFormData` requires, as a starting point for a case that is
/// about something other than those values.
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

/// The create payloads the export cases drive, differing along what the rule generalises
/// over: which members are present at all, how many map and list entries they carry, which
/// union member a style config or a field position takes, how deeply a field input nests,
/// and each published value of the enumerated members.
fn create_cases() -> Vec<(&'static str, Value)> {
    let mut cases = vec![
        ("only the required members", minimal_create("Blank")),
        (
            "two fields, a validation list and a fixed position",
            json!({
                "name": "PostCreateForm",
                "formActionType": "create",
                "dataType": {"dataSourceType": "DataStore", "dataTypeName": "Post"},
                "fields": {
                    "title": {
                        "label": "Title",
                        "position": {"fixed": "first"},
                        "inputType": {"type": "TextField", "required": true},
                        "validations": [
                            {"type": "Required", "validationMessage": "Title is required"},
                            {"type": "StartWith", "strValues": ["a", "b"]},
                        ],
                    },
                    "rating": {
                        "label": "Rating",
                        "position": {"below": "title"},
                        "excluded": false,
                        "inputType": {
                            "type": "SliderField",
                            "minValue": 0.0,
                            "maxValue": 10.0,
                            "step": 0.5,
                        },
                    },
                },
                "style": {"horizontalGap": {"value": "16px"}},
                "sectionalElements": {},
                "schemaVersion": "1.0",
            }),
        ),
        (
            "both style union members, three sectional elements and a full CTA",
            json!({
                "name": "StyledForm",
                "formActionType": "update",
                "dataType": {"dataSourceType": "Custom", "dataTypeName": "Order"},
                "fields": {},
                "style": {
                    "horizontalGap": {"tokenReference": "space.small"},
                    "verticalGap": {"value": "8px"},
                    "outerPadding": {"tokenReference": "space.large"},
                },
                "sectionalElements": {
                    "heading": {"type": "Heading", "level": 1, "text": "Order"},
                    "divider": {"type": "Divider", "orientation": "horizontal", "excluded": false},
                    "spacer": {"type": "Text", "text": "note", "position": {"rightOf": "heading"}},
                },
                "schemaVersion": "2.1",
                "cta": {
                    "position": "top_and_bottom",
                    "clear": {"excluded": true},
                    "cancel": {"children": "Back", "position": {"fixed": "first"}},
                    "submit": {"excluded": false, "children": "Save"},
                },
            }),
        ),
        (
            "tags, a nested value-mapping field and a file uploader",
            json!({
                "name": "UploadForm",
                "formActionType": "create",
                "dataType": {"dataSourceType": "Custom", "dataTypeName": "Asset"},
                "fields": {
                    "kind": {
                        "inputType": {
                            "type": "SelectField",
                            "valueMappings": {
                                "values": [
                                    {"value": {"value": "image"}, "displayValue": {"value": "Image"}},
                                    {"value": {"concat": [{"value": "a"}, {"value": "b"}]}},
                                ],
                            },
                        },
                    },
                    "file": {
                        "inputType": {
                            "type": "StorageField",
                            "fileUploaderConfig": {
                                "accessLevel": "protected",
                                "acceptedFileTypes": ["image/png", "image/jpeg"],
                                "showThumbnails": true,
                                "maxFileCount": 3,
                            },
                        },
                    },
                },
                "style": {},
                "sectionalElements": {},
                "schemaVersion": "1.0",
                "tags": {"env": "test", "team": "studio"},
            }),
        ),
    ];

    // One row per published value of each enumerated member, so the export is exercised
    // over the whole catalogue rather than one value.
    for action in FORM_ACTION_TYPES {
        let mut data = minimal_create(&format!("ActionTyped-{action}"));
        data["formActionType"] = json!(action);
        cases.push(("every formActionType the model admits", data));
    }
    for source in FORM_DATA_SOURCE_TYPES {
        let mut data = minimal_create(&format!("SourceTyped-{source}"));
        data["dataType"] = json!({"dataSourceType": source, "dataTypeName": "Post"});
        cases.push(("every dataSourceType the model admits", data));
    }
    for decorator in LABEL_DECORATORS {
        let mut data = minimal_create(&format!("Decorated-{decorator}"));
        data["labelDecorator"] = json!(decorator);
        cases.push(("every labelDecorator the model admits", data));
    }

    cases
}

/// An export reports the full `Form` of every form of the addressed app environment,
/// exactly as `GetForm` reports it, in creation order — and only of that app environment.
#[tokio::test]
async fn export_forms_reports_the_stored_forms_in_full() {
    let router = emulator();

    // An app environment nothing was created in exports an empty list and no token: the
    // `entities` member is required, so it is `[]` rather than an omitted member.
    let empty = export(&router).await;
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
        "case empty environment: an export that ends the list must omit nextToken, got {:?}",
        empty.body
    );

    let mut created: Vec<(&'static str, String, Value)> = Vec::new();
    for (case, data) in create_cases() {
        let answer = create(&router, data).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from CreateForm, got {} with {:?}",
            answer.status, answer.body
        );
        created.push((case, answer.id(), answer.body.clone()));
    }

    // Forms that belong to another environment of the same app, and to another app:
    // neither is part of this export.
    let other_environment = create_in(&router, APP_ID, "production", minimal_create("Elsewhere"))
        .await
        .id();
    let other_app = create_in(&router, "d9999zzzz", ENVIRONMENT, minimal_create("Elsewhere"))
        .await
        .id();

    let exported = export(&router).await;
    assert_eq!(
        exported.status, 200,
        "expected HTTP 200 from ExportForms, got {} with {:?}",
        exported.status, exported.body
    );
    let expected_ids: Vec<String> = created.iter().map(|(_, id, _)| id.clone()).collect();
    assert_eq!(
        exported.entity_ids(),
        expected_ids,
        "an export must report every form of that app environment once, in creation order, \
         got {:?}",
        exported.entity_ids()
    );
    assert!(
        !exported.entity_ids().contains(&other_environment)
            && !exported.entity_ids().contains(&other_app),
        "an export must report only the addressed (appId, environmentName), got {:?}",
        exported.entity_ids()
    );

    // Each entity is the whole record, not the summary projection: it equals what GetForm
    // serves for that id.
    for (index, (case, id, create_body)) in created.iter().enumerate() {
        let entity = &exported.entities()[index];
        let read = get(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetForm, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            entity, &read.body,
            "case {case:?}: the exported entity must be the full form GetForm reports, got \
             {entity:?}"
        );
        assert_eq!(
            entity, create_body,
            "case {case:?}: the exported entity must be what CreateForm persisted, got \
             {entity:?}"
        );
    }

    // The export of the other app environment reports that environment's own form, and
    // only it.
    let elsewhere = export_in(&router, APP_ID, "production", None).await;
    assert_eq!(
        elsewhere.entity_ids(),
        vec![other_environment.clone()],
        "case appId={APP_ID:?} environmentName=\"production\": expected only that \
         environment's form, got {:?}",
        elsewhere.entity_ids()
    );

    // An export reports what is stored NOW: an update moves the exported record, and a
    // delete removes it.
    let (case, updated_id, _) = created[1].clone();
    let patched = update(
        &router,
        &updated_id,
        json!({"name": "PostCreateFormRenamed", "schemaVersion": "3.0"}),
    )
    .await;
    assert_eq!(
        patched.status, 200,
        "case {case:?}: expected HTTP 200 from UpdateForm, got {} with {:?}",
        patched.status, patched.body
    );
    let after_update = export(&router).await;
    let updated_entity = after_update
        .entities()
        .into_iter()
        .find(|entity| entity["id"] == json!(updated_id))
        .unwrap_or_else(|| panic!("case {case:?}: the updated form must still be exported"));
    assert_eq!(
        updated_entity, patched.body,
        "case {case:?}: the export must report the updated record, got {updated_entity:?}"
    );

    let (case, dropped, _) = created[0].clone();
    let deleted = delete(&router, &dropped).await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: expected an empty 2xx from DeleteForm, got {} with {:?}",
        deleted.status, deleted.body
    );
    let after_delete = export(&router).await;
    assert!(
        !after_delete.entity_ids().contains(&dropped),
        "case {case:?}: a deleted form must not be exported, got {:?}",
        after_delete.entity_ids()
    );
    assert_eq!(
        after_delete.entity_ids(),
        created
            .iter()
            .skip(1)
            .map(|(_, id, _)| id.clone())
            .collect::<Vec<String>>(),
        "after a delete, the export must report exactly the forms still stored"
    );

    // ExportForms is `@readonly`: exporting twice reports the same thing, and leaves what
    // a later read reports untouched.
    let before_get = get(&router, &created[1].1).await.body.clone();
    let again = export(&router).await;
    assert_eq!(
        again.body, after_delete.body,
        "a second export must report the same thing as the first, got {:?}",
        again.body
    );
    assert_eq!(
        get(&router, &created[1].1).await.body,
        before_get,
        "an export must not change what a later GetForm reports"
    );
}

/// `ExportForms` is `@paginated(items: entities)` with no page-size member: following
/// `nextToken` to the end enumerates every stored form exactly once in creation order, and
/// no page is longer than the bound the service serves.
#[tokio::test]
async fn export_forms_pages_over_one_stable_order() {
    let router = emulator();

    // Rows around the service's page bound: fewer than a page, exactly a page, and more
    // than a page (which is the only count that forces a token).
    for count in [1usize, 7, EXPORT_PAGE_LIMIT, EXPORT_PAGE_LIMIT + 3] {
        let environment = format!("env-{count}");
        let mut created = Vec::new();
        for index in 0..count {
            created.push(
                create_in(
                    &router,
                    APP_ID,
                    &environment,
                    minimal_create(&format!("page-{index}")),
                )
                .await
                .id(),
            );
        }

        let (entities, pages) = walk_export(&router, APP_ID, &environment).await;
        let walked: Vec<String> = entities
            .iter()
            .map(|entity| entity["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            walked, created,
            "case {count} forms: walking every page must enumerate each form once, in \
             creation order, got {walked:?}"
        );
        for (page, length) in pages.iter().enumerate() {
            assert!(
                *length <= EXPORT_PAGE_LIMIT,
                "case {count} forms: page {page} carried {length} entities, past the \
                 {EXPORT_PAGE_LIMIT} an export page serves"
            );
        }
        let expected_pages = count.div_euclid(EXPORT_PAGE_LIMIT)
            + usize::from(count % EXPORT_PAGE_LIMIT != 0 || count == 0);
        assert_eq!(
            pages.len(),
            expected_pages,
            "case {count} forms: expected {expected_pages} page(s) of at most \
             {EXPORT_PAGE_LIMIT}, got page lengths {pages:?}"
        );
    }
}

/// A `nextToken` this service never issued for the addressed app environment names no page
/// of the export, so it is refused with the modelled `InvalidParameterException` (HTTP 400)
/// rather than served as an empty page.
#[tokio::test]
async fn export_forms_refuses_a_token_it_never_issued() {
    let router = emulator();
    for index in 0..EXPORT_PAGE_LIMIT + 2 {
        create(&router, minimal_create(&format!("token-{index}"))).await;
    }
    for index in 0..EXPORT_PAGE_LIMIT + 2 {
        create_in(
            &router,
            APP_ID,
            "production",
            minimal_create(&format!("other-{index}")),
        )
        .await;
    }

    let issued = export(&router)
        .await
        .next_token()
        .expect("an export of more than one page carries a nextToken");
    let elsewhere = export_in(&router, APP_ID, "production", None)
        .await
        .next_token()
        .expect("an export of more than one page carries a nextToken");
    assert_ne!(
        issued, elsewhere,
        "a page token is scoped to the app environment it was issued for, got {issued:?} for \
         both"
    );

    // A token the service did issue for this app environment is accepted.
    let resumed = export_in(&router, APP_ID, ENVIRONMENT, Some(&issued)).await;
    assert_eq!(
        resumed.status, 200,
        "case nextToken={issued:?}: a token this export issued must be accepted, got {} with \
         {:?}",
        resumed.status, resumed.body
    );

    // The rows differ in how the token fails to name a page of this export: a shape the
    // service never emits, a token issued for another app environment, and a correctly
    // scoped token whose cursor was never handed out — above the highest one handed out
    // and below the first one alike.
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
        let refused = export_in(&router, APP_ID, ENVIRONMENT, Some(token)).await;
        assert_eq!(
            refused.status, 400,
            "case nextToken={token:?}: a token ExportForms never issued for this app \
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
}
