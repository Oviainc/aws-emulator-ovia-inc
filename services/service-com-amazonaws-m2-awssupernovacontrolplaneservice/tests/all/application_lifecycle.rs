// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for m2's application lifecycle, driven over the wire.
//!
//! Every case goes through the same front door a client uses: a buffered HTTP request
//! dispatched by [`MultiServiceRouter`], so the URI bindings, the restJson1 body and the
//! modelled error envelopes are all exercised. No test reaches into the state struct —
//! what a create persisted is read back through `GetApplication`,
//! `GetApplicationVersion` and `ListApplications`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_m2_awssupernovacontrolplaneservice::ops::types::{
    APPLICATION_LIFECYCLE_VALUES, APPLICATION_VERSION_LIFECYCLE_VALUES, ENGINE_TYPE_VALUES,
};
use service_com_amazonaws_m2_awssupernovacontrolplaneservice::state::{
    APPLICATION_CREATED, APPLICATION_CREATING, CREATION_SETTLES_AFTER, DEFAULT_LIST_PAGE_SIZE,
    MAX_APPLICATIONS, MAX_VERSIONS_PER_APPLICATION, VERSION_AVAILABLE, VERSION_CREATING,
};
use service_com_amazonaws_m2_awssupernovacontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for, so the minted ARN is checked against the region the
/// caller used rather than against a default.
const REGION: &str = "eu-west-1";

/// A SigV4 `Authorization` header naming m2 in [`REGION`]: it is what routes the request
/// to this service and what the ARN's region segment comes from. Signatures are not
/// checked (authentication is off for this router), so the signature bytes are filler.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTM2EXAMPLE/20260101/eu-west-1/m2/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// A router serving ONE m2 service over ONE shared engine — the same assembly
/// `core/router` performs, minus the other services.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one request and return its status and parsed body (`Value::Null` when the
/// response has no body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://m2.{REGION}.amazonaws.com{path}"))
        .header("authorization", AUTHORIZATION)
        .header("host", format!("m2.{REGION}.amazonaws.com"));
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let bytes = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("a test body always serializes"))
    });
    let response = router
        .dispatch(request.body(bytes).expect("a valid test request"))
        .await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The modelled error code of a failure response: restJson1 carries it as the body's
/// `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// `&str` field of a JSON object, for assertion messages that print what came back.
fn field<'a>(body: &'a Value, name: &str) -> &'a str {
    body.get(name).and_then(Value::as_str).unwrap_or("<absent>")
}

/// A `CreateApplication` body with an inline definition.
fn create_body(name: &str, engine_type: &str, definition: &str) -> Value {
    json!({
        "name": name,
        "engineType": engine_type,
        "definition": {"content": definition},
    })
}

/// One row of the create/read-back table: the inputs a create carries, so the read-back
/// is checked against something other than the one shape the issue cited.
struct CreateCase {
    name: &'static str,
    engine_type: &'static str,
    definition: &'static str,
    description: Option<&'static str>,
    tags: Option<Value>,
    kms_key_id: Option<&'static str>,
    role_arn: Option<&'static str>,
}

/// Create -> read back: every member a create persisted comes back from
/// `GetApplication` and `GetApplicationVersion`, for each engine type the model declares
/// and with the optional members both present and absent.
#[tokio::test]
async fn a_created_application_reads_back_from_state() {
    // `engineType`'s value set comes from the model: ENGINE_TYPE_VALUES.
    assert_eq!(
        ENGINE_TYPE_VALUES,
        &["bluage", "microfocus"],
        "the model's EngineType values moved; update this table"
    );

    let cases = [
        CreateCase {
            name: "BootstrapApp",
            engine_type: "bluage",
            definition: r#"{"template-version":"2.0"}"#,
            description: Some("first app"),
            tags: Some(json!({"env": "dev"})),
            kms_key_id: Some("arn:aws:kms:eu-west-1:000000000000:key/abcd-1234"),
            role_arn: Some("arn:aws:iam::000000000000:role/m2-app"),
        },
        CreateCase {
            name: "Micro_Focus-App2",
            engine_type: "microfocus",
            definition: r#"{"template-version":"2.0","source-locations":[]}"#,
            description: None,
            tags: None,
            kms_key_id: None,
            role_arn: None,
        },
        CreateCase {
            name: "A1",
            engine_type: "microfocus",
            // A definition with no JSON structure at all is still stored verbatim.
            definition: "inline-definition-text",
            description: Some(""),
            tags: Some(json!({"team": "mainframe", "cost-centre": "42"})),
            kms_key_id: None,
            role_arn: None,
        },
    ];

    let router = router();
    for case in &cases {
        let mut body = create_body(case.name, case.engine_type, case.definition);
        if let Some(description) = case.description {
            body["description"] = json!(description);
        }
        if let Some(tags) = case.tags.clone() {
            body["tags"] = tags;
        }
        if let Some(kms) = case.kms_key_id {
            body["kmsKeyId"] = json!(kms);
        }
        if let Some(role) = case.role_arn {
            body["roleArn"] = json!(role);
        }

        let (status, created) = call(&router, "POST", "/applications", Some(body)).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 from CreateApplication, got {status} {created}",
            case.name
        );
        let application_id = field(&created, "applicationId").to_string();
        assert!(
            !application_id.is_empty()
                && application_id.len() <= 80
                && !application_id.contains(char::is_whitespace),
            "case {}: applicationId must match ^\\S{{1,80}}$, got {application_id:?}",
            case.name
        );
        assert_eq!(
            field(&created, "applicationArn"),
            format!("arn:aws:m2:{REGION}:{ACCOUNT}:app/{application_id}"),
            "case {}: unexpected applicationArn, got {:?}",
            case.name,
            field(&created, "applicationArn")
        );
        assert_eq!(
            created.get("applicationVersion").and_then(Value::as_i64),
            Some(1),
            "case {}: a create records version 1, got {created}",
            case.name
        );

        // Read back through GetApplication.
        let (status, got) =
            call(&router, "GET", &format!("/applications/{application_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 from GetApplication, got {status} {got}",
            case.name
        );
        assert_eq!(
            field(&got, "name"),
            case.name,
            "case {}: GetApplication returned name {:?}",
            case.name,
            field(&got, "name")
        );
        assert_eq!(
            field(&got, "applicationId"),
            application_id,
            "case {}: GetApplication returned applicationId {:?}",
            case.name,
            field(&got, "applicationId")
        );
        assert_eq!(
            field(&got, "applicationArn"),
            field(&created, "applicationArn"),
            "case {}: GetApplication must return the ARN the create minted, got {:?}",
            case.name,
            field(&got, "applicationArn")
        );
        assert_eq!(
            field(&got, "engineType"),
            case.engine_type,
            "case {}: GetApplication returned engineType {:?}",
            case.name,
            field(&got, "engineType")
        );
        assert_eq!(
            got.get("description").and_then(Value::as_str),
            case.description,
            "case {}: GetApplication returned description {:?}",
            case.name,
            got.get("description")
        );
        assert_eq!(
            got.get("kmsKeyId").and_then(Value::as_str),
            case.kms_key_id,
            "case {}: GetApplication returned kmsKeyId {:?}",
            case.name,
            got.get("kmsKeyId")
        );
        assert_eq!(
            got.get("roleArn").and_then(Value::as_str),
            case.role_arn,
            "case {}: GetApplication returned roleArn {:?}",
            case.name,
            got.get("roleArn")
        );
        assert_eq!(
            got.get("tags").cloned(),
            case.tags.clone(),
            "case {}: GetApplication returned tags {:?}",
            case.name,
            got.get("tags")
        );
        assert!(
            got.get("creationTime").is_some(),
            "case {}: GetApplication must report a creationTime, got {got}",
            case.name
        );
        assert!(
            APPLICATION_LIFECYCLE_VALUES.contains(&field(&got, "status")),
            "case {}: status {:?} is not an ApplicationLifecycle value",
            case.name,
            field(&got, "status")
        );
        let latest = got
            .get("latestVersion")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            latest.get("applicationVersion").and_then(Value::as_i64),
            Some(1),
            "case {}: latestVersion must be version 1, got {latest}",
            case.name
        );
        assert!(
            APPLICATION_VERSION_LIFECYCLE_VALUES.contains(&field(&latest, "status")),
            "case {}: latestVersion.status {:?} is not an ApplicationVersionLifecycle value",
            case.name,
            field(&latest, "status")
        );

        // Read back the stored definition through GetApplicationVersion.
        let (status, version) = call(
            &router,
            "GET",
            &format!("/applications/{application_id}/versions/1"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 from GetApplicationVersion, got {status} {version}",
            case.name
        );
        assert_eq!(
            field(&version, "definitionContent"),
            case.definition,
            "case {}: GetApplicationVersion returned definitionContent {:?}",
            case.name,
            field(&version, "definitionContent")
        );
        assert_eq!(
            version.get("description").and_then(Value::as_str),
            case.description,
            "case {}: GetApplicationVersion returned description {:?}",
            case.name,
            version.get("description")
        );
        assert_eq!(
            field(&version, "name"),
            case.name,
            "case {}: GetApplicationVersion returned name {:?}",
            case.name,
            field(&version, "name")
        );
    }
}

/// The status transition the clock owes a new application and its first version: both are
/// reported in their non-terminal status straight after the create, and settle without
/// any further call.
#[tokio::test]
async fn a_new_application_settles_on_the_clock() {
    let router = router();
    let (status, created) = call(
        &router,
        "POST",
        "/applications",
        Some(create_body("SettlingApp", "bluage", "definition")),
    )
    .await;
    assert_eq!(status, 200, "expected 200 from CreateApplication, got {status} {created}");
    let id = field(&created, "applicationId").to_string();

    let (_, immediately) = call(&router, "GET", &format!("/applications/{id}"), None).await;
    assert_eq!(
        field(&immediately, "status"),
        APPLICATION_CREATING,
        "a just-created application is still {APPLICATION_CREATING}, got {:?}",
        field(&immediately, "status")
    );
    let (_, version_immediately) = call(
        &router,
        "GET",
        &format!("/applications/{id}/versions/1"),
        None,
    )
    .await;
    assert_eq!(
        field(&version_immediately, "status"),
        VERSION_CREATING,
        "a just-recorded version is still {VERSION_CREATING}, got {:?}",
        field(&version_immediately, "status")
    );

    // Nothing is called in between: the transition is owed by the clock alone.
    std::thread::sleep(CREATION_SETTLES_AFTER + Duration::from_millis(100));

    let (_, settled) = call(&router, "GET", &format!("/applications/{id}"), None).await;
    assert_eq!(
        field(&settled, "status"),
        APPLICATION_CREATED,
        "after the settle span the application is {APPLICATION_CREATED}, got {:?}",
        field(&settled, "status")
    );
    let (_, version_settled) = call(
        &router,
        "GET",
        &format!("/applications/{id}/versions/1"),
        None,
    )
    .await;
    assert_eq!(
        field(&version_settled, "status"),
        VERSION_AVAILABLE,
        "after the settle span the version is {VERSION_AVAILABLE}, got {:?}",
        field(&version_settled, "status")
    );
    // A read never moves a status on: the settled answer is stable.
    let (_, again) = call(&router, "GET", &format!("/applications/{id}"), None).await;
    assert_eq!(
        field(&again, "status"),
        field(&settled, "status"),
        "reading an application must not change what a later caller sees, got {:?}",
        field(&again, "status")
    );
}

/// A second application under a name already taken is a 409 `ConflictException`, for
/// every name shape the pattern allows — and the name is free again after a delete.
#[tokio::test]
async fn a_duplicate_name_conflicts() {
    let router = router();
    for name in ["BootstrapApp", "a_1-B", "App-With-A-Rather-Long-But-Legal-Name12"] {
        let (status, first) = call(
            &router,
            "POST",
            "/applications",
            Some(create_body(name, "bluage", "definition")),
        )
        .await;
        assert_eq!(status, 200, "case {name}: first create must succeed, got {status} {first}");

        let (status, conflict) = call(
            &router,
            "POST",
            "/applications",
            Some(create_body(name, "microfocus", "other-definition")),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {name}: a duplicate name must be 409, got {status} {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {name}: expected ConflictException, got {:?}",
            error_code(&conflict)
        );

        // The name is released with the application.
        let id = field(&first, "applicationId").to_string();
        let (status, _) = call(&router, "DELETE", &format!("/applications/{id}"), None).await;
        assert_eq!(status, 200, "case {name}: delete must succeed, got {status}");
        let (status, recreated) = call(
            &router,
            "POST",
            "/applications",
            Some(create_body(name, "bluage", "definition")),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name}: the name must be free after a delete, got {status} {recreated}"
        );
    }
}

/// `clientToken` is an `@idempotencyToken`: the same token with the same application
/// replays the first response instead of creating a second application, and with a
/// different application it conflicts.
#[tokio::test]
async fn a_repeated_client_token_replays_the_first_create() {
    let router = router();
    for (token, name) in [
        ("token-one", "TokenAppOne"),
        ("~!@#$%^&*()_+", "TokenAppTwo"),
        ("3", "TokenAppThree"),
    ] {
        let mut body = create_body(name, "bluage", "definition-for-token");
        body["clientToken"] = json!(token);

        let (status, first) = call(&router, "POST", "/applications", Some(body.clone())).await;
        assert_eq!(status, 200, "case {token}: first create must succeed, got {status} {first}");
        let (status, replay) = call(&router, "POST", "/applications", Some(body.clone())).await;
        assert_eq!(status, 200, "case {token}: a replay must succeed, got {status} {replay}");
        assert_eq!(
            field(&replay, "applicationId"),
            field(&first, "applicationId"),
            "case {token}: a replay must return the first applicationId, got {:?}",
            field(&replay, "applicationId")
        );
        assert_eq!(
            replay.get("applicationVersion"),
            first.get("applicationVersion"),
            "case {token}: a replay must return the first applicationVersion, got {replay}"
        );

        // Same token, a different application: the token cannot name two of them.
        let mut different = create_body(&format!("{name}Other"), "bluage", "definition-for-token");
        different["clientToken"] = json!(token);
        let (status, conflict) = call(&router, "POST", "/applications", Some(different)).await;
        assert_eq!(
            status, 409,
            "case {token}: a reused token with a different request must be 409, got {status} {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {token}: expected ConflictException, got {:?}",
            error_code(&conflict)
        );
    }

    // Exactly one application exists per replayed token.
    let (_, listed) = call(&router, "GET", "/applications", None).await;
    let names: Vec<&str> = listed["applications"]
        .as_array()
        .expect("applications is a list")
        .iter()
        .map(|summary| field(summary, "name"))
        .collect();
    assert_eq!(
        names.len(),
        3,
        "three tokens created three applications, got {names:?}"
    );
}

/// `ListApplications` honours the `names` filter and the `@paginated` contract: a page is
/// at most `maxResults` long, a `nextToken` comes back only when more remain, and
/// following the tokens yields every application exactly once.
#[tokio::test]
async fn list_applications_filters_and_paginates() {
    let router = router();
    let wanted = ["ListAppA", "ListAppB", "ListAppC", "ListAppD", "ListAppE"];
    let mut ids = Vec::new();
    for name in wanted {
        let (status, created) = call(
            &router,
            "POST",
            "/applications",
            Some(create_body(name, "bluage", "definition")),
        )
        .await;
        assert_eq!(status, 200, "case {name}: create must succeed, got {status} {created}");
        ids.push(field(&created, "applicationId").to_string());
    }

    // The `names` filter: 1..10 names, matching and not.
    let name_cases: [(&[&str], usize); 5] = [
        (&[], 5),
        (&["ListAppA"], 1),
        (&["NoSuchApp"], 0),
        (&["ListAppB", "ListAppD"], 2),
        (&["ListAppE", "NoSuchApp"], 1),
    ];
    for (filter, expected) in name_cases {
        let query = filter
            .iter()
            .map(|name| format!("names={name}"))
            .collect::<Vec<_>>()
            .join("&");
        let path = if query.is_empty() {
            "/applications".to_string()
        } else {
            format!("/applications?{query}")
        };
        let (status, listed) = call(&router, "GET", &path, None).await;
        assert_eq!(status, 200, "case {filter:?}: expected 200, got {status} {listed}");
        let returned = listed["applications"]
            .as_array()
            .expect("applications is a list")
            .len();
        assert_eq!(
            returned, expected,
            "case {filter:?}: expected {expected} applications, got {returned} ({listed})"
        );
    }

    // Every summary member a listing must carry.
    let (_, listed) = call(&router, "GET", "/applications?names=ListAppA", None).await;
    let summary = listed["applications"][0].clone();
    let (_, got) = call(
        &router,
        "GET",
        &format!("/applications/{}", field(&summary, "applicationId")),
        None,
    )
    .await;
    for member in ["name", "applicationId", "applicationArn", "status", "engineType"] {
        assert_eq!(
            field(&summary, member),
            field(&got, member),
            "the summary's {member} must be the application's own, got {:?}",
            field(&summary, member)
        );
    }
    assert_eq!(
        summary.get("applicationVersion").and_then(Value::as_i64),
        Some(1),
        "the summary must report the latest version, got {summary}"
    );
    assert!(
        summary.get("creationTime").is_some(),
        "the summary must report a creationTime, got {summary}"
    );

    // Pagination, at every page size from one item to more than the whole collection.
    for page_size in [1_usize, 2, 4, 5, 6] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let path = match &token {
                Some(token) => {
                    format!("/applications?maxResults={page_size}&nextToken={token}")
                }
                None => format!("/applications?maxResults={page_size}"),
            };
            let (status, listed) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size}: expected 200, got {status} {listed}"
            );
            let page = listed["applications"]
                .as_array()
                .expect("applications is a list")
                .clone();
            assert!(
                page.len() <= page_size,
                "case maxResults={page_size}: a page returned {} items",
                page.len()
            );
            for summary in &page {
                seen.push(field(summary, "applicationId").to_string());
            }
            pages += 1;
            assert!(
                pages <= wanted.len() + 1,
                "case maxResults={page_size}: pagination did not terminate after {pages} pages"
            );
            match listed.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case maxResults={page_size}: pagination repeated an application ({seen:?})"
        );
        let mut expected = ids.clone();
        expected.sort();
        assert_eq!(
            unique, expected,
            "case maxResults={page_size}: pagination must yield every application exactly once, got {seen:?}"
        );
    }

    // A token this service never issued is a bad request, not an empty page.
    let (status, refused) = call(
        &router,
        "GET",
        "/applications?nextToken=not-a-token-we-issued",
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "a nextToken this service did not issue must be 400, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected ValidationException, got {:?}",
        error_code(&refused)
    );
}

/// `UpdateApplication` records a NEW version each time, guarded by
/// `currentApplicationVersion`.
#[tokio::test]
async fn update_application_records_a_new_version() {
    let router = router();
    let (_, created) = call(
        &router,
        "POST",
        "/applications",
        Some({
            let mut body = create_body("UpdatableApp", "bluage", "revision-1");
            body["description"] = json!("first rev");
            body
        }),
    )
    .await;
    let id = field(&created, "applicationId").to_string();

    // Each row updates from the version the previous row produced.
    let revisions = [
        (Some("revision-2"), Some("second rev")),
        (Some("revision-3"), None),
        (None, Some("fourth rev")),
    ];
    let mut current = 1_i64;
    let mut expected_definition = "revision-1".to_string();
    let mut expected_description = "first rev".to_string();
    for (definition, description) in revisions {
        let mut body = json!({"currentApplicationVersion": current});
        if let Some(definition) = definition {
            body["definition"] = json!({"content": definition});
            expected_definition = definition.to_string();
        }
        if let Some(description) = description {
            body["description"] = json!(description);
            expected_description = description.to_string();
        }
        let (status, updated) =
            call(&router, "PATCH", &format!("/applications/{id}"), Some(body)).await;
        assert_eq!(
            status, 200,
            "case {definition:?}/{description:?}: expected 200, got {status} {updated}"
        );
        let new_version = updated
            .get("applicationVersion")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        assert_eq!(
            new_version,
            current + 1,
            "case {definition:?}/{description:?}: the update must return the NEW version, got {updated}"
        );

        // The new version carries the merged definition and description.
        let (status, version) = call(
            &router,
            "GET",
            &format!("/applications/{id}/versions/{new_version}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {definition:?}/{description:?}: expected 200 from GetApplicationVersion, got {status} {version}"
        );
        assert_eq!(
            field(&version, "definitionContent"),
            expected_definition,
            "case {definition:?}/{description:?}: definitionContent is {:?}",
            field(&version, "definitionContent")
        );
        assert_eq!(
            field(&version, "description"),
            expected_description,
            "case {definition:?}/{description:?}: description is {:?}",
            field(&version, "description")
        );

        // The application now reports the new version as its latest, and the earlier
        // versions are still readable.
        let (_, got) = call(&router, "GET", &format!("/applications/{id}"), None).await;
        assert_eq!(
            got["latestVersion"]
                .get("applicationVersion")
                .and_then(Value::as_i64),
            Some(new_version),
            "case {definition:?}/{description:?}: latestVersion is {}",
            got["latestVersion"]
        );
        let (status, first) = call(
            &router,
            "GET",
            &format!("/applications/{id}/versions/1"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {definition:?}/{description:?}: version 1 must still be readable, got {status} {first}"
        );
        assert_eq!(
            field(&first, "definitionContent"),
            "revision-1",
            "case {definition:?}/{description:?}: version 1's definition changed to {:?}",
            field(&first, "definitionContent")
        );

        // A stale currentApplicationVersion — the one just superseded — conflicts.
        let (status, conflict) = call(
            &router,
            "PATCH",
            &format!("/applications/{id}"),
            Some(json!({"currentApplicationVersion": current})),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {definition:?}/{description:?}: a stale version must be 409, got {status} {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {definition:?}/{description:?}: expected ConflictException, got {:?}",
            error_code(&conflict)
        );

        current = new_version;
    }

    // A version ahead of the latest conflicts too.
    let (status, conflict) = call(
        &router,
        "PATCH",
        &format!("/applications/{id}"),
        Some(json!({"currentApplicationVersion": current + 7})),
    )
    .await;
    assert_eq!(
        status, 409,
        "a currentApplicationVersion ahead of the latest must be 409, got {status} {conflict}"
    );
}

/// Unknown ids and unknown versions are 404 `ResourceNotFoundException` — never a 501 and
/// never an empty 200.
#[tokio::test]
async fn unknown_ids_and_versions_are_not_found() {
    let router = router();
    let (_, created) = call(
        &router,
        "POST",
        "/applications",
        Some(create_body("NotFoundProbe", "bluage", "definition")),
    )
    .await;
    let id = field(&created, "applicationId").to_string();

    let cases: [(&str, &str, Option<Value>); 5] = [
        ("GET", "/applications/does-not-exist", None),
        ("GET", "/applications/does-not-exist/versions/1", None),
        ("GET", "/applications/0123456789abcdef/versions/3", None),
        (
            "PATCH",
            "/applications/does-not-exist",
            Some(json!({"currentApplicationVersion": 1})),
        ),
        ("GET", "/applications/x/versions/2000", None),
    ];
    for (method, path, body) in cases {
        let (status, failure) = call(&router, method, path, body).await;
        assert_eq!(
            status, 404,
            "case {method} {path}: expected 404, got {status} {failure}"
        );
        assert_eq!(
            error_code(&failure),
            "ResourceNotFoundException",
            "case {method} {path}: expected ResourceNotFoundException, got {:?}",
            error_code(&failure)
        );
    }

    // A version the application does not have yet is also not found.
    for version in [2, 99] {
        let (status, failure) = call(
            &router,
            "GET",
            &format!("/applications/{id}/versions/{version}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case version {version}: expected 404, got {status} {failure}"
        );
    }
}

/// `DeleteApplication` removes the application and its versions, and is idempotent.
#[tokio::test]
async fn delete_application_removes_it_from_state() {
    let router = router();
    let mut ids = Vec::new();
    for name in ["DeleteAppA", "DeleteAppB"] {
        let (_, created) = call(
            &router,
            "POST",
            "/applications",
            Some(create_body(name, "bluage", "definition")),
        )
        .await;
        ids.push(field(&created, "applicationId").to_string());
    }

    let (status, deleted) = call(&router, "DELETE", &format!("/applications/{}", ids[0]), None).await;
    assert_eq!(status, 200, "expected 200 from DeleteApplication, got {status} {deleted}");

    let (status, got) = call(&router, "GET", &format!("/applications/{}", ids[0]), None).await;
    assert_eq!(status, 404, "a deleted application must be 404, got {status} {got}");
    let (status, version) = call(
        &router,
        "GET",
        &format!("/applications/{}/versions/1", ids[0]),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "a deleted application's versions must be 404, got {status} {version}"
    );

    let (_, listed) = call(&router, "GET", "/applications", None).await;
    let remaining: Vec<&str> = listed["applications"]
        .as_array()
        .expect("applications is a list")
        .iter()
        .map(|summary| field(summary, "applicationId"))
        .collect();
    assert_eq!(
        remaining,
        vec![ids[1].as_str()],
        "only the surviving application must be listed, got {remaining:?}"
    );

    // @idempotent: deleting it again, or deleting one that never existed, answers the same.
    for path in [
        format!("/applications/{}", ids[0]),
        "/applications/never-existed".to_string(),
    ] {
        let (status, again) = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 200,
            "case {path}: a repeated delete must answer 200, got {status} {again}"
        );
    }
}

/// A definition the emulator cannot read — an Amazon S3 location — is refused with the
/// modelled `ValidationException`, never persisted as invented content.
#[tokio::test]
async fn an_s3_definition_is_refused_with_the_modelled_error() {
    let router = router();
    let cases = [
        ("S3DefinedApp", json!({"s3Location": "s3://my-bucket/app.json"})),
        ("S3DefinedApp2", json!({"s3Location": "s3://other/definition.json"})),
    ];
    for (name, definition) in cases {
        let (status, refused) = call(
            &router,
            "POST",
            "/applications",
            Some(json!({"name": name, "engineType": "bluage", "definition": definition})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name}: an S3 definition must be refused with 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {name}: expected ValidationException, got {:?}",
            error_code(&refused)
        );
    }

    // Nothing was persisted by a refused create.
    let (_, listed) = call(&router, "GET", "/applications", None).await;
    assert_eq!(
        listed["applications"].as_array().map(Vec::len),
        Some(0),
        "a refused create must persist nothing, got {listed}"
    );
}

/// The store has a ceiling: a create past `MAX_APPLICATIONS` is refused with the modelled
/// quota fault, and an update past `MAX_VERSIONS_PER_APPLICATION` with the modelled
/// validation fault — nothing in this emulator grows without a bound.
#[tokio::test]
async fn the_store_refuses_to_grow_past_its_ceiling() {
    let router = router();
    for index in 0..MAX_APPLICATIONS {
        let (status, created) = call(
            &router,
            "POST",
            "/applications",
            Some(create_body(&format!("CeilingApp{index}"), "bluage", "definition")),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {index}: a create below the ceiling must succeed, got {status} {created}"
        );
    }
    let (status, refused) = call(
        &router,
        "POST",
        "/applications",
        Some(create_body("CeilingAppOverflow", "bluage", "definition")),
    )
    .await;
    assert_eq!(
        status, 402,
        "a create past the {MAX_APPLICATIONS}-application ceiling must be refused, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException, got {:?}",
        error_code(&refused)
    );

    // An unqualified listing is clamped too: it returns a page and a token, never the
    // whole store.
    let (status, listed) = call(&router, "GET", "/applications", None).await;
    assert_eq!(status, 200, "expected 200 from ListApplications, got {status} {listed}");
    let returned = listed["applications"]
        .as_array()
        .expect("applications is a list")
        .len();
    assert_eq!(
        returned, DEFAULT_LIST_PAGE_SIZE,
        "a listing with no maxResults must be clamped to {DEFAULT_LIST_PAGE_SIZE}, got {returned}"
    );
    assert!(
        listed.get("nextToken").and_then(Value::as_str).is_some(),
        "a clamped listing must carry a nextToken, got {listed}"
    );

    // The version history of one application is capped the same way, on a fresh store.
    let router = crate::application_lifecycle::router();
    let (_, created) = call(
        &router,
        "POST",
        "/applications",
        Some(create_body("VersionCeilingApp", "bluage", "revision-1")),
    )
    .await;
    let id = field(&created, "applicationId").to_string();
    for version in 1..MAX_VERSIONS_PER_APPLICATION {
        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/applications/{id}"),
            Some(json!({"currentApplicationVersion": version})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case version {version}: an update below the ceiling must succeed, got {status} {updated}"
        );
    }
    let (status, refused) = call(
        &router,
        "PATCH",
        &format!("/applications/{id}"),
        Some(json!({"currentApplicationVersion": MAX_VERSIONS_PER_APPLICATION})),
    )
    .await;
    assert_eq!(
        status, 400,
        "an update past the {MAX_VERSIONS_PER_APPLICATION}-version ceiling must be refused, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected ValidationException, got {:?}",
        error_code(&refused)
    );
}

/// The statuses this service reports are values the MODEL declares.
#[test]
fn the_reported_statuses_are_model_values() {
    for status in [APPLICATION_CREATING, APPLICATION_CREATED] {
        assert!(
            APPLICATION_LIFECYCLE_VALUES.contains(&status),
            "{status:?} is not an ApplicationLifecycle value: {APPLICATION_LIFECYCLE_VALUES:?}"
        );
    }
    for status in [VERSION_CREATING, VERSION_AVAILABLE] {
        assert!(
            APPLICATION_VERSION_LIFECYCLE_VALUES.contains(&status),
            "{status:?} is not an ApplicationVersionLifecycle value: {APPLICATION_VERSION_LIFECYCLE_VALUES:?}"
        );
    }
}
