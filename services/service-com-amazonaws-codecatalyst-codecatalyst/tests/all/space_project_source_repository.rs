// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the space -> project -> source-repository loop.
//!
//! Every case drives the real HTTP path: a request is dispatched through a
//! `MultiServiceRouter` holding this service over ONE shared engine, so routing,
//! restJson1 deserialization, the handler, state and serialization are all exercised
//! exactly as the emulator serves them. Nothing here reaches into the state struct
//! directly — a value only counts as persisted if an API read reports it back.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The emulator under test: one engine, one mounted service, authentication off
/// (the signature path is the framework's, not this service's).
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codecatalyst_codecatalyst::EmulatedService::new(engine),
    );
    router
}

/// The space the state seeds (`crate::state::DEFAULT_SPACE_NAME`); the API models no
/// `CreateSpace`, so every test starts from it.
const SPACE: &str = "my-space";

/// Dispatch one request and return its status and parsed JSON body (`Value::Null`
/// for an empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("serialize the request body")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://codecatalyst.us-west-2.amazonaws.com{path}"))
        // Host-style routing: CodeCatalyst signs with bearer tokens, so the endpoint
        // host is what names the service (`resolve_route_key`).
        .header("host", "codecatalyst.us-west-2.amazonaws.com")
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A member of a JSON object as a string, or `""` when absent — so an assertion
/// message prints what was actually there.
fn member<'a>(body: &'a Value, key: &str) -> &'a str {
    body.get(key).and_then(Value::as_str).unwrap_or_default()
}

async fn create_project(
    router: &MultiServiceRouter,
    space: &str,
    display_name: &str,
    description: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({ "displayName": display_name });
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    call(
        router,
        "PUT",
        &format!("/v1/spaces/{space}/projects"),
        Some(body),
    )
    .await
}

async fn get_project(router: &MultiServiceRouter, space: &str, name: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/v1/spaces/{space}/projects/{name}"),
        None,
    )
    .await
}

async fn list_projects(router: &MultiServiceRouter, space: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/spaces/{space}/projects"),
        Some(body),
    )
    .await
}

async fn create_source_repository(
    router: &MultiServiceRouter,
    space: &str,
    project: &str,
    name: &str,
    description: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({});
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    call(
        router,
        "PUT",
        &format!("/v1/spaces/{space}/projects/{project}/sourceRepositories/{name}"),
        Some(body),
    )
    .await
}

async fn get_source_repository(
    router: &MultiServiceRouter,
    space: &str,
    project: &str,
    name: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/v1/spaces/{space}/projects/{project}/sourceRepositories/{name}"),
        None,
    )
    .await
}

/// `ListSpaces` reports the seeded space FROM STATE, with the members
/// `SpaceSummary` declares, and emits no `nextToken` for an untruncated page.
#[tokio::test]
async fn list_spaces_reports_the_seeded_space() {
    let router = emulator();
    let (status, body) = call(&router, "POST", "/v1/spaces", Some(json!({}))).await;

    assert_eq!(status, 200, "ListSpaces: expected 200, got {status} ({body})");
    let items = body
        .get("items")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListSpaces: no items in {body}"));
    assert_eq!(items.len(), 1, "ListSpaces: expected one space, got {body}");
    assert_eq!(
        member(&items[0], "name"),
        SPACE,
        "ListSpaces: unexpected space name in {body}"
    );
    // `RegionString`: len 3..16, pattern
    // ^(us(?:-gov)?|af|ap|ca|cn|eu|sa)-(central|(?:north|south)?(?:east|west)?)-(\d+)$
    let region = member(&items[0], "regionName");
    assert!(
        region.starts_with("us-") && region.len() >= 3 && region.len() <= 16,
        "ListSpaces: regionName {region:?} is not a RegionString in {body}"
    );
    assert!(
        body.get("nextToken").is_none() || body["nextToken"].is_null(),
        "ListSpaces: a page that was not truncated must emit no nextToken: {body}"
    );
}

/// The RULE: `CreateProject` derives the project `name` from `displayName` by
/// replacing each separating space (which `ProjectDisplayName` admits and
/// `NameString` does not) with a hyphen, stores the project, and every read reports
/// exactly what was stored.
///
/// The rows differ along what the rule generalises over: whether the display name
/// contains spaces, which separators it uses, its length, and whether a description
/// was given.
#[tokio::test]
async fn create_project_derives_its_name_and_persists_what_it_reports() {
    // (displayName, description, expected derived name)
    let cases: &[(&str, Option<&str>, &str)] = &[
        ("my-project", Some("demo one"), "my-project"),
        ("My Project", Some("demo two"), "My-Project"),
        ("a b c", None, "a-b-c"),
        ("dots.and_underscores", Some("demo 3"), "dots.and_underscores"),
        ("Many Words In A Name", None, "Many-Words-In-A-Name"),
        // `ProjectDisplayName` admits 3..63 characters; the longest one still derives
        // a `NameString` of the same length.
        (
            "aaaaaaaaaa bbbbbbbbbb cccccccccc dddddddddd eeeeeeeeee ffffffff",
            Some("longest display name"),
            "aaaaaaaaaa-bbbbbbbbbb-cccccccccc-dddddddddd-eeeeeeeeee-ffffffff",
        ),
    ];

    let router = emulator();
    for case in cases {
        let (display_name, description, expected_name) = *case;
        let (status, body) = create_project(&router, SPACE, display_name, description).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateProject expected 201, got {status} ({body})"
        );
        assert_eq!(
            member(&body, "name"),
            expected_name,
            "case {case:?}: CreateProject derived the wrong name ({body})"
        );
        assert_eq!(
            member(&body, "spaceName"),
            SPACE,
            "case {case:?}: CreateProject reported the wrong space ({body})"
        );
        assert_eq!(
            member(&body, "displayName"),
            display_name,
            "case {case:?}: CreateProject reported the wrong displayName ({body})"
        );
        assert_eq!(
            body.get("description").and_then(Value::as_str),
            description,
            "case {case:?}: CreateProject reported the wrong description ({body})"
        );

        // Round trip: the read reports the STORED project, not the request path.
        let (status, read) = get_project(&router, SPACE, expected_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetProject expected 200, got {status} ({read})"
        );
        assert_eq!(
            member(&read, "name"),
            expected_name,
            "case {case:?}: GetProject returned the wrong name ({read})"
        );
        assert_eq!(
            member(&read, "displayName"),
            display_name,
            "case {case:?}: GetProject lost the stored displayName ({read})"
        );
        assert_eq!(
            read.get("description").and_then(Value::as_str),
            description,
            "case {case:?}: GetProject lost the stored description ({read})"
        );
    }

    // Every created project is listed, once.
    let (status, body) = list_projects(&router, SPACE, json!({})).await;
    assert_eq!(status, 200, "ListProjects expected 200, got {status}");
    let mut listed: Vec<&str> = body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("ListProjects: no items in {body}"))
        .iter()
        .map(|item| member(item, "name"))
        .collect();
    listed.sort_unstable();
    let mut expected: Vec<&str> = cases.iter().map(|(_, _, name)| *name).collect();
    expected.sort_unstable();
    assert_eq!(
        listed, expected,
        "ListProjects must enumerate exactly the created projects, got {body}"
    );
}

/// `@idempotent`: re-creating a project exactly as stored reports the stored project
/// and creates nothing new; a request that would have to CHANGE it conflicts (409).
#[tokio::test]
async fn create_project_is_idempotent_and_conflicts_on_a_change() {
    let router = emulator();
    let (status, _) = create_project(&router, SPACE, "repeat-me", Some("first")).await;
    assert_eq!(status, 201, "CreateProject expected 201, got {status}");

    for repeat in 0..2 {
        let (status, body) = create_project(&router, SPACE, "repeat-me", Some("first")).await;
        assert_eq!(
            status, 201,
            "repeat {repeat}: an identical CreateProject must succeed, got {status} ({body})"
        );
        assert_eq!(
            member(&body, "description"),
            "first",
            "repeat {repeat}: the stored project changed ({body})"
        );
    }

    // (what differs, description sent)
    let conflicting: &[(&str, Option<&str>)] =
        &[("a different description", Some("second")), ("no description at all", None)];
    for case in conflicting {
        let (status, body) = create_project(&router, SPACE, "repeat-me", case.1).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 ConflictException, got {status} ({body})"
        );
        assert_eq!(
            member(&body, "__type"),
            "ConflictException",
            "case {case:?}: wrong error shape ({body})"
        );
    }

    // ...and the stored project is untouched by the refused requests.
    let (_, read) = get_project(&router, SPACE, "repeat-me").await;
    assert_eq!(
        member(&read, "description"),
        "first",
        "a refused CreateProject must not change the stored project ({read})"
    );
    let (_, body) = list_projects(&router, SPACE, json!({})).await;
    assert_eq!(
        body["items"].as_array().map(Vec::len),
        Some(1),
        "an idempotent repeat must not create a second project ({body})"
    );
}

/// The RULE: a list page holds at most `maxResults` items and emits a `nextToken`
/// ONLY when items remain; a returned token resumes strictly after the last item of
/// the previous page. Rows differ along the page size, including the published
/// bounds of `maxResults` (1..100) and "absent".
#[tokio::test]
async fn list_projects_pages_with_max_results_and_next_token() {
    let router = emulator();
    let created: Vec<String> = (0..7).map(|i| format!("proj-{i}")).collect();
    for name in &created {
        let (status, body) = create_project(&router, SPACE, name, None).await;
        assert_eq!(status, 201, "seeding {name}: got {status} ({body})");
    }

    for page_size in [None, Some(1), Some(2), Some(3), Some(7), Some(100)] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(size) = page_size {
                request["maxResults"] = json!(size);
            }
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = list_projects(&router, SPACE, request).await;
            assert_eq!(
                status, 200,
                "maxResults {page_size:?}: ListProjects expected 200, got {status} ({body})"
            );
            let items = body["items"]
                .as_array()
                .unwrap_or_else(|| panic!("maxResults {page_size:?}: no items in {body}"));
            if let Some(size) = page_size {
                assert!(
                    items.len() <= size as usize,
                    "maxResults {page_size:?}: page holds {} items ({body})",
                    items.len()
                );
            }
            seen.extend(items.iter().map(|item| member(item, "name").to_string()));
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= created.len() + 2,
                "maxResults {page_size:?}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                assert_eq!(
                    seen.len(),
                    created.len(),
                    "maxResults {page_size:?}: the last page must end the listing, saw {seen:?}"
                );
                break;
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seen.len(),
            "maxResults {page_size:?}: an item was returned twice: {seen:?}"
        );
        assert_eq!(
            sorted, created,
            "maxResults {page_size:?}: paging lost or invented projects: {seen:?}"
        );
        assert_eq!(
            seen, created,
            "maxResults {page_size:?}: pages must follow the sorted key order: {seen:?}"
        );
    }
}

/// A `nextToken` this service did not issue is refused with the modelled
/// `ValidationException` (400), never answered with the first page.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let router = emulator();
    let (status, _) = create_project(&router, SPACE, "only-one", None).await;
    assert_eq!(status, 201, "seeding: got {status}");

    // Not a cursor this service issued — these are the forms a caller can get wrong:
    // a project name passed straight through, text that is not an encoded cursor at
    // all, and encoded bytes that are not a key.
    for token in ["only-one", "!!!not-a-cursor!!!", "ff"] {
        for path in ["/v1/spaces", &format!("/v1/spaces/{SPACE}/projects")] {
            let (status, body) =
                call(&router, "POST", path, Some(json!({ "nextToken": token }))).await;
            assert_eq!(
                status, 400,
                "token {token:?} on {path}: expected 400, got {status} ({body})"
            );
            assert_eq!(
                member(&body, "__type"),
                "ValidationException",
                "token {token:?} on {path}: wrong error shape ({body})"
            );
        }
    }
}

/// The RULE: a `ProjectListFilter` narrows the listing — filters are ANDed, each
/// filter's `values` are ORed, and the `comparisonOperator` (default `EQ`) decides
/// how a project's field is compared. Rows cover every operator the model declares.
#[tokio::test]
async fn list_projects_honours_every_modelled_filter_operator() {
    let router = emulator();
    for name in ["alpha", "alphabet", "beta", "gamma"] {
        let (status, body) = create_project(&router, SPACE, name, None).await;
        assert_eq!(status, 201, "seeding {name}: got {status} ({body})");
    }

    // (filter, expected project names)
    let cases: &[(Value, &[&str])] = &[
        (json!({"key": "name", "values": ["beta"]}), &["beta"]),
        (
            json!({"key": "name", "values": ["alpha", "gamma"]}),
            &["alpha", "gamma"],
        ),
        (
            json!({"key": "name", "values": ["beta"], "comparisonOperator": "EQ"}),
            &["beta"],
        ),
        (
            json!({"key": "name", "values": ["alpha"], "comparisonOperator": "BEGINS_WITH"}),
            &["alpha", "alphabet"],
        ),
        (
            json!({"key": "name", "values": ["beta"], "comparisonOperator": "GT"}),
            &["gamma"],
        ),
        (
            json!({"key": "name", "values": ["beta"], "comparisonOperator": "GE"}),
            &["beta", "gamma"],
        ),
        (
            json!({"key": "name", "values": ["beta"], "comparisonOperator": "LT"}),
            &["alpha", "alphabet"],
        ),
        (
            json!({"key": "name", "values": ["beta"], "comparisonOperator": "LE"}),
            &["alpha", "alphabet", "beta"],
        ),
        // `hasAccessTo` needs space membership this emulator does not model, so it
        // is answered with a 501 that names it — never with an unfiltered list.
    ];

    for case in cases {
        let (filter, expected) = case;
        let (status, body) =
            list_projects(&router, SPACE, json!({ "filters": [filter] })).await;
        assert_eq!(
            status, 200,
            "filter {filter}: expected 200, got {status} ({body})"
        );
        let listed: Vec<&str> = body["items"]
            .as_array()
            .unwrap_or_else(|| panic!("filter {filter}: no items in {body}"))
            .iter()
            .map(|item| member(item, "name"))
            .collect();
        assert_eq!(
            listed, *expected,
            "filter {filter}: wrong projects listed ({body})"
        );
    }

    // Two filters are ANDed.
    let (_, body) = list_projects(
        &router,
        SPACE,
        json!({"filters": [
            {"key": "name", "values": ["alpha"], "comparisonOperator": "BEGINS_WITH"},
            {"key": "name", "values": ["alphabet"]},
        ]}),
    )
    .await;
    let listed: Vec<&str> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| member(item, "name"))
        .collect();
    assert_eq!(
        listed,
        vec!["alphabet"],
        "two filters must both hold, got {body}"
    );
}

/// A filter key the emulator cannot evaluate is answered HONESTLY: `hasAccessTo`
/// needs space membership this emulator does not model, so the request gets a 501
/// naming it rather than a 200 carrying an unfiltered list.
#[tokio::test]
async fn the_filter_key_the_emulator_cannot_evaluate_is_not_answered_with_a_list() {
    let router = emulator();
    for name in ["alpha", "beta"] {
        create_project(&router, SPACE, name, None).await;
    }

    // Alone, and alongside a key that IS evaluated: either way the request is refused.
    let requests = [
        json!({"filters": [{"key": "hasAccessTo", "values": ["any-user"]}]}),
        json!({"filters": [
            {"key": "name", "values": ["alpha"]},
            {"key": "hasAccessTo", "values": ["any-user"]},
        ]}),
        json!({"filters": [{"key": "hasAccessTo", "values": ["a", "b"], "comparisonOperator": "EQ"}]}),
    ];
    for request in requests {
        let (status, body) = list_projects(&router, SPACE, request.clone()).await;
        assert_eq!(
            status, 501,
            "request {request}: expected 501 NotImplemented, got {status} ({body})"
        );
        assert_eq!(
            member(&body, "__type"),
            "NotImplemented",
            "request {request}: wrong error shape ({body})"
        );
        assert!(
            member(&body, "message").contains("hasAccessTo"),
            "request {request}: the refusal must name the filter it cannot evaluate ({body})"
        );
    }
}

/// A filter list bigger than this emulator states it evaluates is refused with the
/// modelled `ValidationException`, so the matching work one request can ask for is
/// bounded (the model declares no length limit on `filters` or its `values`).
#[tokio::test]
async fn an_oversized_filter_list_is_refused() {
    let router = emulator();
    create_project(&router, SPACE, "alpha", None).await;

    let many_filters: Vec<Value> = (0..11)
        .map(|i| json!({"key": "name", "values": [format!("p{i}")]}))
        .collect();
    let many_values: Vec<String> = (0..101).map(|i| format!("p{i}")).collect();
    let cases = [
        ("11 filters", json!({ "filters": many_filters })),
        (
            "101 values",
            json!({ "filters": [{"key": "name", "values": many_values}] }),
        ),
    ];
    for (case, request) in cases {
        let (status, body) = list_projects(&router, SPACE, request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 ValidationException, got {status} ({body})"
        );
        assert_eq!(
            member(&body, "__type"),
            "ValidationException",
            "case {case}: wrong error shape ({body})"
        );
    }

    // The largest list the emulator states it evaluates is still served.
    let at_the_limit: Vec<Value> = (0..10)
        .map(|i| json!({"key": "name", "values": [format!("p{i}")]}))
        .collect();
    let (status, body) = list_projects(&router, SPACE, json!({ "filters": at_the_limit })).await;
    assert_eq!(
        status, 200,
        "a filter list at the stated maximum must be served, got {status} ({body})"
    );
}

/// The server-side page maximum: a listing never answers with more than
/// `crate::state::MAX_PAGE_SIZE` items, whatever (or whether) the caller asks, and
/// the rest is reachable only by following the token.
#[tokio::test]
async fn a_listing_is_capped_by_the_server_side_page_maximum() {
    // The handler's own cap, not a copy of it, so this cannot drift from the code.
    let max_page_size = usize::try_from(
        service_com_amazonaws_codecatalyst_codecatalyst::state::MAX_PAGE_SIZE,
    )
    .expect("MAX_PAGE_SIZE is positive");
    let stored = max_page_size + 5;

    let router = emulator();
    for i in 0..stored {
        let (status, body) = create_project(&router, SPACE, &format!("p{i:03}"), None).await;
        assert_eq!(status, 201, "seeding p{i:03}: got {status} ({body})");
    }

    // Absent and at the cap: the page is bounded the same way and the remainder
    // needs the token.
    for request in [json!({}), json!({ "maxResults": max_page_size })] {
        let (status, body) = list_projects(&router, SPACE, request.clone()).await;
        assert_eq!(
            status, 200,
            "request {request}: expected 200, got {status} ({body})"
        );
        let items = body["items"].as_array().expect("items");
        assert_eq!(
            items.len(),
            max_page_size,
            "request {request}: a page must hold at most {max_page_size} items"
        );
        let token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("request {request}: a truncated page must carry a token"));
        let (status, rest) = list_projects(&router, SPACE, json!({ "nextToken": token })).await;
        assert_eq!(status, 200, "request {request}: resuming failed ({rest})");
        assert_eq!(
            rest["items"].as_array().map(Vec::len),
            Some(stored - max_page_size),
            "request {request}: the token must resume with the remainder ({rest})"
        );
    }
}

/// The RULE: `CreateSourceRepository` stores the repository under its project with
/// the timestamps the reads report, and `GetSourceRepository` reports exactly that.
/// Rows differ along the repository name's form and length and whether a description
/// was given — `SourceRepositoryNameString` is len 1..100, pattern
/// `^(?!.*[.]git$)[\w\-.]*$`.
#[tokio::test]
async fn create_source_repository_persists_what_the_read_reports() {
    let router = emulator();
    let (status, _) = create_project(&router, SPACE, "host-project", Some("holds repos")).await;
    assert_eq!(status, 201, "seeding the project: got {status}");

    // (repository name, description)
    let cases: &[(&str, Option<&str>)] = &[
        ("my-repo", Some("src")),
        ("a", None),
        ("repo.v2", Some("dotted name")),
        ("under_scored-and.dotted", Some("every separator")),
        // The longest name the model admits (100 characters).
        (
            "r000000001r000000002r000000003r000000004r000000005r000000006r000000007r000000008r000000009r0000000",
            None,
        ),
    ];

    for case in cases {
        let (name, description) = *case;
        let (status, body) =
            create_source_repository(&router, SPACE, "host-project", name, description).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateSourceRepository expected 201, got {status} ({body})"
        );
        assert_eq!(
            member(&body, "name"),
            name,
            "case {case:?}: wrong name reported ({body})"
        );
        assert_eq!(
            member(&body, "projectName"),
            "host-project",
            "case {case:?}: wrong projectName reported ({body})"
        );
        assert_eq!(
            member(&body, "spaceName"),
            SPACE,
            "case {case:?}: wrong spaceName reported ({body})"
        );
        assert_eq!(
            body.get("description").and_then(Value::as_str),
            description,
            "case {case:?}: wrong description reported ({body})"
        );

        let (status, read) = get_source_repository(&router, SPACE, "host-project", name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetSourceRepository expected 200, got {status} ({read})"
        );
        assert_eq!(
            member(&read, "name"),
            name,
            "case {case:?}: GetSourceRepository returned the wrong repository ({read})"
        );
        assert_eq!(
            read.get("description").and_then(Value::as_str),
            description,
            "case {case:?}: GetSourceRepository lost the stored description ({read})"
        );
        // Both timestamps are `@required` on the output, and a repository that was
        // never updated carries createdTime == lastUpdatedTime.
        let created = read
            .get("createdTime")
            .unwrap_or_else(|| panic!("case {case:?}: no createdTime in {read}"));
        let updated = read
            .get("lastUpdatedTime")
            .unwrap_or_else(|| panic!("case {case:?}: no lastUpdatedTime in {read}"));
        assert_eq!(
            created, updated,
            "case {case:?}: a freshly created repository must report equal timestamps ({read})"
        );

        // The stored timestamps are STABLE across reads: reading does not re-stamp.
        let (_, again) = get_source_repository(&router, SPACE, "host-project", name).await;
        assert_eq!(
            again.get("createdTime"),
            Some(created),
            "case {case:?}: createdTime changed between reads ({again})"
        );
    }
}

/// `@idempotent`: re-creating a source repository exactly as stored reports it with
/// its original `createdTime`; a request that would have to change it conflicts.
#[tokio::test]
async fn create_source_repository_is_idempotent_and_conflicts_on_a_change() {
    let router = emulator();
    create_project(&router, SPACE, "repo-host", None).await;
    let (status, _) = create_source_repository(&router, SPACE, "repo-host", "repo", Some("src")).await;
    assert_eq!(status, 201, "seeding the repository: got {status}");
    let (_, first) = get_source_repository(&router, SPACE, "repo-host", "repo").await;

    let (status, body) =
        create_source_repository(&router, SPACE, "repo-host", "repo", Some("src")).await;
    assert_eq!(
        status, 201,
        "an identical CreateSourceRepository must succeed, got {status} ({body})"
    );
    let (_, second) = get_source_repository(&router, SPACE, "repo-host", "repo").await;
    assert_eq!(
        first.get("createdTime"),
        second.get("createdTime"),
        "an idempotent repeat must not re-stamp createdTime ({second})"
    );

    for description in [Some("different"), None] {
        let (status, body) =
            create_source_repository(&router, SPACE, "repo-host", "repo", description).await;
        assert_eq!(
            status, 409,
            "description {description:?}: expected 409, got {status} ({body})"
        );
        assert_eq!(
            member(&body, "__type"),
            "ConflictException",
            "description {description:?}: wrong error shape ({body})"
        );
    }
}

/// The RULE: a request naming a space, project or source repository that does not
/// exist is refused with the modelled `ResourceNotFoundException` (404) — never a
/// 200 with a synthesized resource, and never a 501.
#[tokio::test]
async fn a_reference_to_a_missing_parent_is_a_modelled_404() {
    let router = emulator();
    create_project(&router, SPACE, "real-project", None).await;

    // (what is missing, method, path, body)
    let cases: &[(&str, &str, String, Option<Value>)] = &[
        (
            "CreateProject in an unknown space",
            "PUT",
            "/v1/spaces/no-such-space/projects".to_string(),
            Some(json!({"displayName": "whatever"})),
        ),
        (
            "GetProject in an unknown space",
            "GET",
            "/v1/spaces/no-such-space/projects/real-project".to_string(),
            None,
        ),
        (
            "GetProject of an unknown project",
            "GET",
            format!("/v1/spaces/{SPACE}/projects/no-such-project"),
            None,
        ),
        (
            "ListProjects in an unknown space",
            "POST",
            "/v1/spaces/no-such-space/projects".to_string(),
            Some(json!({})),
        ),
        (
            "DeleteProject of an unknown project",
            "DELETE",
            format!("/v1/spaces/{SPACE}/projects/no-such-project"),
            None,
        ),
        (
            "DeleteProject in an unknown space",
            "DELETE",
            "/v1/spaces/no-such-space/projects/real-project".to_string(),
            None,
        ),
        (
            "CreateSourceRepository under an unknown project",
            "PUT",
            format!("/v1/spaces/{SPACE}/projects/no-such-project/sourceRepositories/repo"),
            Some(json!({})),
        ),
        (
            "CreateSourceRepository in an unknown space",
            "PUT",
            "/v1/spaces/no-such-space/projects/real-project/sourceRepositories/repo".to_string(),
            Some(json!({})),
        ),
        (
            "GetSourceRepository of an unknown repository",
            "GET",
            format!("/v1/spaces/{SPACE}/projects/real-project/sourceRepositories/no-such-repo"),
            None,
        ),
    ];

    for case in cases {
        let (what, method, path, body) = case;
        let (status, response) = call(&router, method, path, body.clone()).await;
        assert_eq!(
            status, 404,
            "{what}: expected 404 ResourceNotFoundException, got {status} ({response})"
        );
        assert_eq!(
            member(&response, "__type"),
            "ResourceNotFoundException",
            "{what}: wrong error shape ({response})"
        );
        assert!(
            !member(&response, "message").is_empty(),
            "{what}: the modelled message member is @required ({response})"
        );
    }
}

/// `DeleteProject` removes the project AND the source repositories it owns, so every
/// later read of either fails and the listing no longer carries it.
#[tokio::test]
async fn delete_project_removes_the_project_and_its_repositories() {
    let router = emulator();
    for display_name in ["doomed-project", "surviving-project"] {
        let (status, _) = create_project(&router, SPACE, display_name, Some("demo")).await;
        assert_eq!(status, 201, "seeding {display_name}: got {status}");
    }
    let (status, _) =
        create_source_repository(&router, SPACE, "doomed-project", "doomed-repo", None).await;
    assert_eq!(status, 201, "seeding the repository: got {status}");

    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/v1/spaces/{SPACE}/projects/doomed-project"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DeleteProject expected 200, got {status} ({body})");
    assert_eq!(
        member(&body, "name"),
        "doomed-project",
        "DeleteProject must report the project it removed ({body})"
    );
    assert_eq!(
        member(&body, "spaceName"),
        SPACE,
        "DeleteProject must report the space ({body})"
    );
    assert_eq!(
        member(&body, "displayName"),
        "doomed-project",
        "DeleteProject must report the stored displayName ({body})"
    );

    let (status, read) = get_project(&router, SPACE, "doomed-project").await;
    assert_eq!(
        status, 404,
        "GetProject after DeleteProject must fail, got {status} ({read})"
    );
    let (status, read) = get_source_repository(&router, SPACE, "doomed-project", "doomed-repo").await;
    assert_eq!(
        status, 404,
        "a repository cannot outlive its project, got {status} ({read})"
    );

    let (_, body) = list_projects(&router, SPACE, json!({})).await;
    let listed: Vec<&str> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| member(item, "name"))
        .collect();
    assert_eq!(
        listed,
        vec!["surviving-project"],
        "ListProjects must no longer list the deleted project ({body})"
    );

    // Deleting twice is not a silent success: the project is gone.
    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/v1/spaces/{SPACE}/projects/doomed-project"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "a second DeleteProject must report the project missing, got {status} ({body})"
    );
}

/// The RULE: a create that would take a container past the most resources it
/// holds is refused with the modelled `ServiceQuotaExceededException` (402), so the
/// store cannot be grown without bound one request at a time. Both caller-driven
/// growth sites are covered: projects in a space and source repositories in a
/// project (`crate::state::EMULATOR_MAX_PROJECTS_PER_SPACE` /
/// `EMULATOR_MAX_SOURCE_REPOSITORIES_PER_PROJECT`).
#[tokio::test]
async fn a_create_past_the_stated_maximum_is_refused() {
    // The handler's own caps, not copies of them, so this cannot drift from the code.
    use service_com_amazonaws_codecatalyst_codecatalyst::state::{
        EMULATOR_MAX_PROJECTS_PER_SPACE as MAX_PROJECTS_PER_SPACE,
        EMULATOR_MAX_SOURCE_REPOSITORIES_PER_PROJECT as MAX_SOURCE_REPOSITORIES_PER_PROJECT,
    };

    let router = emulator();

    // Fill the space to its maximum, one project per request.
    for i in 0..MAX_PROJECTS_PER_SPACE {
        let (status, body) = create_project(&router, SPACE, &format!("p{i:04}"), None).await;
        assert_eq!(status, 201, "seeding project {i}: got {status} ({body})");
    }
    let (status, body) = create_project(&router, SPACE, "one-too-many", None).await;
    assert_eq!(
        status, 402,
        "the project past the maximum must be refused, got {status} ({body})"
    );
    assert_eq!(
        member(&body, "__type"),
        "ServiceQuotaExceededException",
        "wrong error shape for a project past the maximum ({body})"
    );
    // The refused request stored nothing: the listing still ends at the maximum.
    let (_, read) = get_project(&router, SPACE, "one-too-many").await;
    assert_eq!(
        member(&read, "__type"),
        "ResourceNotFoundException",
        "a refused CreateProject must store nothing ({read})"
    );
    // ...and an idempotent repeat of an ALREADY STORED project still succeeds at the
    // cap, because it adds nothing.
    let (status, body) = create_project(&router, SPACE, "p0000", None).await;
    assert_eq!(
        status, 201,
        "an idempotent repeat adds no project and must still succeed, got {status} ({body})"
    );

    // The same rule for source repositories under one project.
    for i in 0..MAX_SOURCE_REPOSITORIES_PER_PROJECT {
        let (status, body) =
            create_source_repository(&router, SPACE, "p0000", &format!("r{i:04}"), None).await;
        assert_eq!(status, 201, "seeding repository {i}: got {status} ({body})");
    }
    let (status, body) =
        create_source_repository(&router, SPACE, "p0000", "one-too-many", None).await;
    assert_eq!(
        status, 402,
        "the repository past the maximum must be refused, got {status} ({body})"
    );
    assert_eq!(
        member(&body, "__type"),
        "ServiceQuotaExceededException",
        "wrong error shape for a repository past the maximum ({body})"
    );
    let (status, read) = get_source_repository(&router, SPACE, "p0000", "one-too-many").await;
    assert_eq!(
        status, 404,
        "a refused CreateSourceRepository must store nothing, got {status} ({read})"
    );
}

/// State is shared across requests, not per request: what one call stored, a later
/// call on the same emulator reads back. (The failure this pins is a handler that
/// builds its own engine — which compiles and silently drops every write.)
#[tokio::test]
async fn writes_are_visible_to_later_requests_and_isolated_per_emulator() {
    let first = emulator();
    create_project(&first, SPACE, "shared-project", Some("demo")).await;
    let (status, _) = get_project(&first, SPACE, "shared-project").await;
    assert_eq!(status, 200, "the write must be visible to a later request");

    let second = emulator();
    let (status, body) = get_project(&second, SPACE, "shared-project").await;
    assert_eq!(
        status, 404,
        "a separate emulator must not see another's projects, got {status} ({body})"
    );
    // ...but its own seeded space is there.
    let (status, _) = call(&second, "POST", "/v1/spaces", Some(json!({}))).await;
    assert_eq!(status, 200, "a fresh emulator seeds its own space");
}
