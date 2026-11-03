// SPDX-License-Identifier: CC0-1.0

//! The one page walk every paginated operation of this service serves a page from, end to
//! end over HTTP.
//!
//! The rule under test, which all six of them implement: a page of an app environment's
//! records starts at the head of that environment's creation order when the request names
//! no `nextToken`; a token is honoured only when it is one the service issued for that app
//! environment, and anything else is refused with the modelled `InvalidParameterException`
//! (HTTP 400) naming the operation and the token rather than served as a page; a honoured
//! token resumes at the first record AT OR AFTER the cursor it carries, so the rest of the
//! list is still served when the record that cursor named was deleted meanwhile; and
//! following the tokens to the end enumerates every record of that app environment exactly
//! once, in that one order.
//!
//! Each row drives the walk through a different operation — `ListComponents`,
//! `ExportComponents`, `ListThemes`, `ExportThemes`, `ExportForms` and `ListCodegenJobs` —
//! over the record kind that operation pages and the projection it serves, so the shared
//! walk is exercised from every operation that reaches it. Every case goes through the
//! router: routing, deserialization, the handler, the persisted state and serialization.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::state::page_token;
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app environment the cases address, and a second one whose tokens must not be
/// honoured against the first. `AppId` is `@length(min: 1, max: 20)` with pattern
/// `^d[a-z0-9]+$`.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";
const OTHER_ENVIRONMENT: &str = "production";

/// How many records each row creates in the addressed app environment: more than one page
/// of the smallest `maxResults` a list admits, so a walk takes several pages, and fewer
/// than the bound an export page serves, so an export walk takes one.
const RECORDS: usize = 5;

/// The smallest page `maxResults` admits, from `ListEntityLimit` / `ListCodegenJobsLimit`
/// (`@range(min: 1, max: 100)` on both): the page size the rows below walk a list with, so
/// the walk crosses several pages.
const SMALLEST_PAGE: usize = 1;

/// A page size well inside that range, so a row also walks pages longer than one record.
const SMALL_PAGE: usize = 2;

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
    /// carries none. AmplifyUIBuilder names no exception on the wire, so a refusal of one
    /// of this service's modelled exceptions must carry none of them.
    fn error_code(&self) -> &str {
        ["__type", "code", "Code"]
            .iter()
            .find_map(|member| self.body[*member].as_str())
            .unwrap_or_default()
    }

    /// The ids the `entities` of this answer carry, in the order they are reported.
    fn entity_ids(&self) -> Vec<String> {
        self.body["entities"]
            .as_array()
            .map(|entities| {
                entities
                    .iter()
                    .map(|entity| entity["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether the answer carries an `entities` list at all.
    fn has_entities(&self) -> bool {
        self.body["entities"].is_array()
    }

    /// The `nextToken` the answer carries, if any.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }

    /// The `message` the answer carries, or the empty string.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The `id` a create answered.
    fn id(&self) -> String {
        self.body["id"]
            .as_str()
            .unwrap_or_else(|| panic!("a created record must carry an id, got {:?}", self.body))
            .to_string()
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

/// The record kinds this service pages over, each with the collection it is created in and
/// the payload its create requires.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Components,
    Forms,
    Themes,
    CodegenJobs,
}

impl Kind {
    /// The last path segment of the collection, as the `@http` URIs of this service spell
    /// it.
    fn segment(self) -> &'static str {
        match self {
            Kind::Components => "components",
            Kind::Forms => "forms",
            Kind::Themes => "themes",
            Kind::CodegenJobs => "codegen-jobs",
        }
    }

    /// The smallest payload the create of this kind admits — the members its request shape
    /// requires, which is all a case about paging needs.
    fn create_payload(self, index: usize) -> Value {
        let name = format!("page-{index}");
        match self {
            Kind::Components => json!({
                "name": name,
                "componentType": "View",
                "properties": {},
                "variants": [],
                "overrides": {},
                "bindingProperties": {},
            }),
            Kind::Forms => json!({
                "name": name,
                "formActionType": "create",
                "dataType": {"dataSourceType": "DataStore", "dataTypeName": "Post"},
                "fields": {},
                "style": {},
                "sectionalElements": {},
                "schemaVersion": "1.0",
            }),
            Kind::Themes => json!({"name": name, "values": []}),
            // `StartCodegenJobData` requires only `renderConfig`, a union with one variant.
            Kind::CodegenJobs => json!({"renderConfig": {"react": {}}}),
        }
    }

    /// Whether this service models a delete for the kind, so a row can prove the walk
    /// resumes past a record the token named and a delete removed.
    fn has_delete(self) -> bool {
        !matches!(self, Kind::CodegenJobs)
    }
}

/// Where a kind of this app environment is created, listed and addressed by id.
fn collection_uri(app_id: &str, environment: &str, kind: Kind) -> String {
    format!(
        "/app/{app_id}/environment/{environment}/{segment}",
        segment = kind.segment()
    )
}

/// Where a kind of this app environment is exported: the `@http` URI the `Export*`
/// operations are bound to.
fn export_uri(app_id: &str, environment: &str, kind: Kind) -> String {
    format!(
        "/export/app/{app_id}/environment/{environment}/{segment}",
        segment = kind.segment()
    )
}

/// One paginated operation: the kind it pages over, where it is bound, and the page size a
/// request may ask it for (`None` for an export, which models no page-size member).
struct Paginated {
    operation: &'static str,
    kind: Kind,
    page_uri: fn(&str, &str, Kind) -> String,
    page_size: Option<usize>,
}

/// Every operation of this crate that serves a page, one row each: the two component
/// listings the duplicated walk lived in, and the theme, form and codegen-job pages that
/// now share it too.
fn paginated_operations() -> Vec<Paginated> {
    vec![
        Paginated {
            operation: "ListComponents",
            kind: Kind::Components,
            page_uri: collection_uri,
            page_size: Some(SMALLEST_PAGE),
        },
        Paginated {
            operation: "ExportComponents",
            kind: Kind::Components,
            page_uri: export_uri,
            page_size: None,
        },
        Paginated {
            operation: "ListThemes",
            kind: Kind::Themes,
            page_uri: collection_uri,
            page_size: Some(SMALL_PAGE),
        },
        Paginated {
            operation: "ExportThemes",
            kind: Kind::Themes,
            page_uri: export_uri,
            page_size: None,
        },
        Paginated {
            operation: "ExportForms",
            kind: Kind::Forms,
            page_uri: export_uri,
            page_size: None,
        },
        Paginated {
            operation: "ListCodegenJobs",
            kind: Kind::CodegenJobs,
            page_uri: collection_uri,
            page_size: Some(SMALL_PAGE),
        },
    ]
}

impl Paginated {
    /// Ask this operation for one page, resumed at `token` when one is given.
    async fn page(&self, router: &MultiServiceRouter, token: Option<&str>) -> Answer {
        let base = (self.page_uri)(APP_ID, ENVIRONMENT, self.kind);
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = self.page_size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(token) = token {
            query.push(format!("nextToken={token}"));
        }
        let uri = if query.is_empty() {
            base
        } else {
            format!("{base}?{}", query.join("&"))
        };
        call(router, "GET", &uri, None).await
    }

    /// Every id a complete walk of this operation reports, following `nextToken` to the
    /// end, with the length of each page it took.
    async fn walk(&self, router: &MultiServiceRouter) -> (Vec<String>, Vec<usize>) {
        let mut ids: Vec<String> = Vec::new();
        let mut pages: Vec<usize> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let answer = self.page(router, token.as_deref()).await;
            assert_eq!(
                answer.status,
                200,
                "case {} walk page {}: a page of a token this operation issued must be served, \
                 got {} with {:?}",
                self.operation,
                pages.len(),
                answer.status,
                answer.body
            );
            let page = answer.entity_ids();
            pages.push(page.len());
            ids.extend(page);
            match answer.next_token() {
                Some(next) => token = Some(next),
                None => break,
            }
            assert!(
                pages.len() <= RECORDS + 1,
                "case {}: a walk of {RECORDS} records must end within {} pages, took {pages:?}",
                self.operation,
                RECORDS + 1
            );
        }
        (ids, pages)
    }
}

/// Create one record of `kind` in an app environment and answer the id it was minted under.
async fn create(
    router: &MultiServiceRouter,
    environment: &str,
    kind: Kind,
    index: usize,
) -> String {
    let answer = call(
        router,
        "POST",
        &collection_uri(APP_ID, environment, kind),
        Some(kind.create_payload(index)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "creating a {kind:?} record in {environment} must answer HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.id()
}

/// Every paginated operation serves its page from the one shared walk: it starts at the
/// head with no token, resumes at or after the cursor a token it issued carries — past a
/// record a delete removed meanwhile — enumerates every record of the addressed app
/// environment exactly once in creation order, and refuses a token it never issued.
///
/// The rows differ in the operation driving the walk, and so in the record kind it pages,
/// the projection it serves and the page bound it uses.
#[tokio::test]
async fn every_paginated_operation_serves_the_same_page_walk() {
    for row in paginated_operations() {
        let Paginated {
            operation, kind, ..
        } = row;
        // A fresh store per row, so the cursors this app environment's records hold are
        // exactly the first `RECORDS` the store hands out, in creation order.
        let router = emulator();
        let mut created: Vec<String> = Vec::new();
        for index in 0..RECORDS {
            created.push(create(&router, ENVIRONMENT, kind, index).await);
        }
        // A second app environment, created after the first, so its records hold the
        // cursors above them and its own tokens are the wrong-scope row below.
        for index in 0..2 {
            create(&router, OTHER_ENVIRONMENT, kind, index).await;
        }

        // With no token the page starts at the head of the creation order, and reports only
        // the addressed app environment's records.
        let head = row.page(&router, None).await;
        assert_eq!(
            head.status, 200,
            "case {operation} first page: a page with no token must be served, got {} with {:?}",
            head.status, head.body
        );
        let expected_head: Vec<String> = created
            .iter()
            .take(row.page_size.unwrap_or(RECORDS).min(RECORDS))
            .cloned()
            .collect();
        assert_eq!(
            head.entity_ids(),
            expected_head,
            "case {operation} first page: a page with no token must start at the head of the \
             creation order, got {:?}",
            head.entity_ids()
        );

        // Following the tokens to the end enumerates every record once, in that one order,
        // and no page is longer than the size the request asked for.
        let (walked, pages) = row.walk(&router).await;
        assert_eq!(
            walked, created,
            "case {operation}: walking every page must enumerate each record once, in creation \
             order, got {walked:?}"
        );
        if let Some(size) = row.page_size {
            for (page, length) in pages.iter().enumerate() {
                assert!(
                    *length <= size,
                    "case {operation} page {page}: a page must carry at most the {size} records \
                     asked for, got {length}"
                );
            }
        }

        // A token the service issued for this app environment resumes at the record its
        // cursor names: the cursors of this environment's records run from the first one
        // the store hands out, in creation order.
        let key = (APP_ID.to_string(), ENVIRONMENT.to_string());
        for (index, _) in created.iter().enumerate() {
            let cursor = u64::try_from(index + 1).expect("a record index fits in u64");
            let token = page_token(&key, cursor);
            let resumed = row.page(&router, Some(&token)).await;
            assert_eq!(
                resumed.status, 200,
                "case {operation} resume at cursor {cursor}: a token the service issued must be \
                 served, got {} with {:?}",
                resumed.status, resumed.body
            );
            let expected: Vec<String> = created
                .iter()
                .skip(index)
                .take(row.page_size.unwrap_or(RECORDS))
                .cloned()
                .collect();
            assert_eq!(
                resumed.entity_ids(),
                expected,
                "case {operation} resume at cursor {cursor}: the page must resume at the record \
                 that cursor names, got {:?}",
                resumed.entity_ids()
            );
        }

        // A token the service never issued for this app environment names no page, so it is
        // refused rather than served. The rows differ in HOW it fails to name one: a shape
        // the service never emits, a cursor below the first one ever minted, a cursor far
        // above the highest handed out, and a token of another app environment.
        let other_key = (APP_ID.to_string(), OTHER_ENVIRONMENT.to_string());
        let below_the_first = page_token(&key, 0);
        let past_the_highest = page_token(&key, u64::MAX);
        let from_elsewhere = page_token(&other_key, 1);
        for (case, token) in [
            ("a shape the service never emits", "not-a-token"),
            ("a prefix with no digits", "c-"),
            ("a cursor below the first minted", below_the_first.as_str()),
            ("a cursor past the highest handed out", past_the_highest.as_str()),
            ("a token of another app environment", from_elsewhere.as_str()),
        ] {
            let refused = row.page(&router, Some(token)).await;
            assert_eq!(
                refused.status, 400,
                "case {operation} / {case} ({token:?}): a token the service never issued must be \
                 refused, got {} with {:?}",
                refused.status, refused.body
            );
            assert!(
                !refused.has_entities(),
                "case {operation} / {case} ({token:?}): a refusal must carry no entities, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.error_code(),
                "",
                "case {operation} / {case} ({token:?}): a refusal must name no error type, got \
                 {:?}",
                refused.body
            );
            let message = refused.message();
            assert!(
                message.contains(operation) && message.contains(token),
                "case {operation} / {case} ({token:?}): the refusal must name the operation and \
                 the token it refuses, got {message:?}"
            );
        }

        // The resume is AT OR AFTER the cursor, so deleting the record a token names leaves
        // the rest of the list served rather than ending the walk.
        if kind.has_delete() {
            let dropped = created.remove(1);
            let cursor = 2;
            let deleted = call(
                &router,
                "DELETE",
                &format!(
                    "{}/{dropped}",
                    collection_uri(APP_ID, ENVIRONMENT, kind)
                ),
                None,
            )
            .await;
            assert_eq!(
                deleted.status, 200,
                "case {operation} delete of the record at cursor {cursor}: the delete must \
                 answer HTTP 200, got {} with {:?}",
                deleted.status, deleted.body
            );
            let resumed = row
                .page(&router, Some(&page_token(&key, cursor)))
                .await;
            assert_eq!(
                resumed.status, 200,
                "case {operation} resume past a deleted record: the token must still be served, \
                 got {} with {:?}",
                resumed.status, resumed.body
            );
            let expected: Vec<String> = created
                .iter()
                .skip(1)
                .take(row.page_size.unwrap_or(RECORDS))
                .cloned()
                .collect();
            assert_eq!(
                resumed.entity_ids(),
                expected,
                "case {operation} resume past a deleted record: the page must serve the records \
                 after the deleted one, got {:?}",
                resumed.entity_ids()
            );
            let (walked, _) = row.walk(&router).await;
            assert_eq!(
                walked, created,
                "case {operation} after a delete: the walk must enumerate exactly the records \
                 still stored, got {walked:?}"
            );
        }
    }
}
