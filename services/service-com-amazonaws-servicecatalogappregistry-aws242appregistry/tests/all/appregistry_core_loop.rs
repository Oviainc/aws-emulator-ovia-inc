// SPDX-License-Identifier: CC0-1.0

//! The AppRegistry application + attribute-group core loop, driven end to end over HTTP.
//!
//! Every case goes through the real front door: a [`MultiServiceRouter`] with this service
//! mounted on ONE shared engine, dispatching real restJson1 requests. So each assertion
//! covers routing, deserialization, the handler, state and serialization together — the
//! same path a client takes.
//!
//! The cases are tables, because the rules under test are general: an application is
//! stored and read back whatever its name, description and tag count; `attributes` comes
//! back byte-for-byte whatever document it holds; a listing pages identically at every
//! page size. One recorded request is one row.

use std::collections::BTreeSet;
use std::sync::Arc;

use service_com_amazonaws_servicecatalogappregistry_aws242appregistry::state::{
    ApplicationRecord, AttributeGroupRecord, ComAmazonawsServicecatalogappregistryAws242appregistryState as AppRegistryState,
    ResourceKind, APPLICATION_QUOTA, ATTRIBUTE_GROUP_QUOTA, IDEMPOTENCY_TOKEN_LIMIT,
};
use service_com_amazonaws_servicecatalogappregistry_aws242appregistry::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for. Not `us-east-1`, so a region read
/// from the request cannot be confused with a hardcoded default.
///
/// `pub(crate)` because the other suites in this test target sign for the same region and
/// drive the same front door; see [`call`].
pub(crate) const REGION: &str = "us-west-2";

/// A router with this service mounted on one shared engine — what the production router
/// builds, narrowed to the service under test.
fn router() -> MultiServiceRouter {
    router_on(Arc::new(engine::InMemoryEngine::new()))
}

/// The same router over an engine the caller already holds, so a case can seed the store
/// directly instead of issuing thousands of requests to reach a bound.
fn router_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one request and return its status and JSON body.
///
/// The `Authorization` header carries the SigV4 credential scope the router routes on and
/// the handlers read the region from; it is not a real signature (this emulator verifies
/// signatures only with authentication switched on).
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://servicecatalog-appregistry.{REGION}.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{REGION}/servicecatalog/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(
            body.map(|value| {
                bytes::Bytes::from(serde_json::to_vec(&value).expect("a JSON body serializes"))
            })
            .unwrap_or_default(),
        )
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let json = if payload.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|error| {
            panic!(
                "{method} {path} answered {status} with a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, json)
}

/// The error code a failure response carries, from restJson1's `__type`.
pub(crate) fn error_code(body: &serde_json::Value) -> String {
    body.get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Percent-encode a path label the way an AWS SDK encodes a non-greedy `@httpLabel`:
/// everything outside the unreserved set, so an ARN's `:` and `/` stay inside one segment
/// instead of inventing path segments.
pub(crate) fn encode_label(label: &str) -> String {
    let mut encoded = String::with_capacity(label.len());
    for byte in label.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The raw JSON value at `pointer`.
///
/// Used for the timestamp members: how a `date-time` timestamp is rendered on the wire is
/// the framework's schema-driven business, so these cases assert the VALUES (equal to each
/// other, and surviving the round trip) rather than a lexical form.
fn json_at(body: &serde_json::Value, pointer: &str) -> serde_json::Value {
    body.pointer(pointer)
        .unwrap_or_else(|| panic!("expected a value at {pointer} in {body}"))
        .clone()
}

pub(crate) fn as_str(body: &serde_json::Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("expected a string at {pointer} in {body}"))
        .to_string()
}

/// Whether `id` is what `ApplicationId` (smithy_query shape=ApplicationId: len 26..26,
/// `^[a-z0-9]+$`) accepts.
fn is_application_id(id: &str) -> bool {
    id.len() == 26 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Whether `token` is what the model's `nextToken` member accepts
/// (`^[A-Za-z0-9+/=]+$`, len 1..2024).
fn is_next_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 2024
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

/// The ARN this emulator must mint for an application, built the one way ARNs are built.
fn application_arn(id: &str) -> String {
    format!(
        "arn:aws:servicecatalog:{REGION}:{}:/applications/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

fn attribute_group_arn(id: &str) -> String {
    format!(
        "arn:aws:servicecatalog:{REGION}:{}:/attribute-groups/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// One row of the create/read-back table: a name, a description and a tag set that differ
/// along everything the rule generalises over (name character class, description present
/// or absent, tag count 0/1/many).
struct ApplicationCase {
    label: &'static str,
    name: &'static str,
    description: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

fn application_cases() -> Vec<ApplicationCase> {
    // `name` is `len 1..256, ^[-.\w]+$` (OPERATIONS.md, CreateApplication): letters,
    // digits, underscore, dot and dash. One row per character class, plus the published
    // length bounds (1 and 256) and a 0-tag, 1-tag and many-tag row.
    vec![
        ApplicationCase {
            label: "letters only, no description, no tags",
            name: "plainname",
            description: None,
            tags: vec![],
        },
        ApplicationCase {
            label: "dashes and digits, one tag",
            name: "order-service-2",
            description: Some("the order service"),
            tags: vec![("env", "dev")],
        },
        ApplicationCase {
            label: "dots and underscores, three tags",
            name: "team.platform_core",
            description: Some(""),
            tags: vec![("env", "prod"), ("owner", "platform"), ("tier", "1")],
        },
        ApplicationCase {
            label: "shortest name the model allows",
            name: "a",
            description: Some("one character"),
            tags: vec![("only", "")],
        },
        ApplicationCase {
            label: "longest name the model allows",
            // 256 `x`s: `Name`'s published upper length bound.
            name: "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\
                   xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\
                   xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\
                   xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            description: None,
            tags: vec![],
        },
    ]
}

fn create_application_body(case: &ApplicationCase, client_token: &str) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert("name".to_string(), case.name.into());
    body.insert("clientToken".to_string(), client_token.into());
    if let Some(description) = case.description {
        body.insert("description".to_string(), description.into());
    }
    if !case.tags.is_empty() {
        body.insert(
            "tags".to_string(),
            serde_json::Value::Object(
                case.tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), serde_json::Value::from(*value)))
                    .collect(),
            ),
        );
    }
    serde_json::Value::Object(body)
}

/// A created application is read back, through every label form, exactly as it was
/// created: minted id and ARN, equal timestamps, the caller's description and tags, and a
/// resource count computed from state.
#[tokio::test]
async fn a_created_application_reads_back_through_name_id_and_arn() {
    let router = router();

    for (row, case) in application_cases().iter().enumerate() {
        let (status, created) = call(
            &router,
            "POST",
            "/applications",
            Some(create_application_body(case, &format!("token-{row}"))),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateApplication, got {status} with {created}",
            case.label
        );

        let id = as_str(&created, "/application/id");
        assert!(
            is_application_id(&id),
            "case {:?}: expected a 26-character lowercase-alnum id, got {id:?}",
            case.label
        );
        assert_eq!(
            as_str(&created, "/application/arn"),
            application_arn(&id),
            "case {:?}: the ARN must address the minted id in the request's region",
            case.label
        );
        assert_eq!(
            json_at(&created, "/application/creationTime"),
            json_at(&created, "/application/lastUpdateTime"),
            "case {:?}: a newly created application has never been updated, so its \
             lastUpdateTime is its creationTime",
            case.label
        );
        assert_eq!(
            as_str(&created, "/application/applicationTag/awsApplication"),
            application_arn(&id),
            "case {:?}: applicationTag must identify this application's resources by its \
             own ARN",
            case.label
        );

        // All three label forms address the one stored record.
        let arn = application_arn(&id);
        let mut bodies = Vec::new();
        for label in [case.name, id.as_str(), arn.as_str()] {
            let (status, read) = call(
                &router,
                "GET",
                &format!("/applications/{}", encode_label(label)),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected 200 reading the application back by {label:?}, got \
                 {status} with {read}",
                case.label
            );
            bodies.push(read);
        }
        for (label, read) in ["name", "id", "arn"].iter().zip(&bodies) {
            assert_eq!(
                read, &bodies[0],
                "case {:?}: the {label} label resolved to a different record than the name \
                 did: {read:?}",
                case.label
            );
        }

        let read = &bodies[0];
        assert_eq!(
            as_str(read, "/id"),
            id,
            "case {:?}: the read-back id must be the minted one",
            case.label
        );
        assert_eq!(
            as_str(read, "/name"),
            case.name,
            "case {:?}: the read-back name must be the created one",
            case.label
        );
        assert_eq!(
            read.get("description").and_then(serde_json::Value::as_str),
            case.description,
            "case {:?}: the read-back description must be the created one, got {:?}",
            case.label,
            read.get("description")
        );
        assert_eq!(
            read.get("associatedResourceCount")
                .and_then(serde_json::Value::as_i64),
            Some(0),
            "case {:?}: an application with no associated resources counts 0, got {:?}",
            case.label,
            read.get("associatedResourceCount")
        );
        assert_eq!(
            json_at(read, "/creationTime"),
            json_at(&created, "/application/creationTime"),
            "case {:?}: the stored creationTime must survive the round trip",
            case.label
        );
        let expected_tags: Option<serde_json::Value> = if case.tags.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(
                case.tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), serde_json::Value::from(*value)))
                    .collect(),
            ))
        };
        assert_eq!(
            read.get("tags").cloned(),
            expected_tags,
            "case {:?}: the read-back tags must be the created ones",
            case.label
        );
    }
}

/// A label nothing is stored under is a 404 `ResourceNotFoundException`, whichever form it
/// takes.
#[tokio::test]
async fn an_unknown_application_label_is_not_found() {
    let router = router();
    let absent_arn = application_arn("aaaaaaaaaaaaaaaaaaaaaaaaaa");
    for label in [
        "no-such-application",
        "aaaaaaaaaaaaaaaaaaaaaaaaaa",
        absent_arn.as_str(),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/applications/{}", encode_label(label)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 for an application nothing is stored under, got \
             {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// Replaying a `clientToken` with the same parameters returns the SAME application;
/// replaying it with different parameters, or taking a name another application holds, is
/// a 409 `ConflictException`.
#[tokio::test]
async fn a_client_token_replays_rather_than_creating_a_second_application() {
    let router = router();

    for (row, case) in application_cases().iter().enumerate() {
        let token = format!("replay-{row}");
        let body = create_application_body(case, &token);

        let (first_status, first) =
            call(&router, "POST", "/applications", Some(body.clone())).await;
        assert_eq!(
            first_status, 201,
            "case {:?}: expected 201 creating the application, got {first_status} with \
             {first}",
            case.label
        );
        let (replay_status, replay) = call(&router, "POST", "/applications", Some(body)).await;
        assert!(
            (200..300).contains(&replay_status),
            "case {:?}: a retry with the same token and parameters must succeed, got \
             {replay_status} with {replay}",
            case.label
        );
        assert_eq!(
            replay, first,
            "case {:?}: a retry with the same token must replay the stored application",
            case.label
        );

        // Same token, a different description: a conflict, not a second application.
        let mut altered = create_application_body(case, &token);
        altered["description"] = serde_json::Value::from("something else entirely");
        let (status, conflict) = call(&router, "POST", "/applications", Some(altered)).await;
        assert_eq!(
            status, 409,
            "case {:?}: the same token with different parameters must conflict, got \
             {status} with {conflict}",
            case.label
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {:?}: expected ConflictException, got {conflict}",
            case.label
        );

        // A different token cannot take a name that is already in use.
        let mut same_name = create_application_body(case, &format!("other-{row}"));
        same_name
            .as_object_mut()
            .expect("an object body")
            .remove("description");
        let (status, conflict) = call(&router, "POST", "/applications", Some(same_name)).await;
        assert_eq!(
            status, 409,
            "case {:?}: a name already in use must conflict, got {status} with {conflict}",
            case.label
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {:?}: expected ConflictException, got {conflict}",
            case.label
        );
    }

    // Exactly one application per row survived all of that.
    let (status, listed) = call(&router, "GET", "/applications", None).await;
    assert_eq!(status, 200, "expected 200 from ListApplications, got {listed}");
    let count = listed
        .pointer("/applications")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    assert_eq!(
        count,
        application_cases().len(),
        "expected one application per row and no replays or conflicts stored, got {listed}"
    );
}

/// Paging a listing at ANY page size walks every application exactly once, in one stable
/// order, and only the non-final pages carry a token.
#[tokio::test]
async fn list_applications_pages_every_application_exactly_once() {
    let router = router();

    let names = ["alpha", "bravo", "charlie", "delta", "echo"];
    let mut created = BTreeSet::new();
    for (row, name) in names.iter().enumerate() {
        let (status, body) = call(
            &router,
            "POST",
            "/applications",
            Some(serde_json::json!({ "name": name, "clientToken": format!("page-{row}") })),
        )
        .await;
        assert_eq!(status, 201, "expected 201 creating {name}, got {body}");
        created.insert(as_str(&body, "/application/id"));
    }

    let (status, unpaged) = call(&router, "GET", "/applications", None).await;
    assert_eq!(status, 200, "expected 200 from ListApplications, got {unpaged}");
    let full_order: Vec<String> = unpaged
        .pointer("/applications")
        .and_then(serde_json::Value::as_array)
        .expect("applications is a list")
        .iter()
        .map(|summary| as_str(summary, "/id"))
        .collect();
    assert_eq!(
        full_order.iter().cloned().collect::<BTreeSet<_>>(),
        created,
        "an unpaged listing must report every created application, got {unpaged}"
    );

    // Every page size from below the item count to above it, including the published
    // bounds of `maxResults` (1..100).
    for page_size in [1_usize, 2, 3, 4, 5, 6, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let path = match &token {
                None => format!("/applications?maxResults={page_size}"),
                Some(token) => format!("/applications?maxResults={page_size}&nextToken={token}"),
            };
            let (status, body) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected 200, got {status} with {body}"
            );
            let page = body
                .pointer("/applications")
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| panic!("case page size {page_size}: no applications list in {body}"));
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page must hold at most maxResults items, got {}",
                page.len()
            );
            seen.extend(page.iter().map(|summary| as_str(summary, "/id")));

            pages += 1;
            assert!(
                pages <= names.len() + 2,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );

            match body.get("nextToken").and_then(serde_json::Value::as_str) {
                Some(next) => {
                    assert!(
                        is_next_token(next),
                        "case page size {page_size}: nextToken must match the model's \
                         ^[A-Za-z0-9+/=]+$, got {next:?}"
                    );
                    assert!(
                        seen.len() < names.len(),
                        "case page size {page_size}: the last page must carry no nextToken, \
                         got {next:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, full_order,
            "case page size {page_size}: paging must walk the unpaged order exactly once"
        );
    }

    // A token this listing never issued is refused, not silently treated as page one.
    let (status, body) = call(&router, "GET", "/applications?nextToken=notatoken", None).await;
    assert_eq!(
        status, 400,
        "expected 400 for a nextToken this listing never issued, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException for a foreign nextToken, got {body}"
    );
}

/// One row of the attribute-group table: `attributes` is a caller-supplied document, so
/// the rows differ in shape, whitespace and character range.
struct AttributeGroupCase {
    label: &'static str,
    name: &'static str,
    attributes: &'static str,
}

fn attribute_group_cases() -> Vec<AttributeGroupCase> {
    // `attributes` is `len 1..8000, ^[\u0009\u000A\u000D\u0020-\u00FF]+$` (OPERATIONS.md,
    // CreateAttributeGroup): tab, newline, carriage return and U+0020..U+00FF. One row per
    // feature that survival byte-for-byte depends on.
    vec![
        AttributeGroupCase {
            label: "compact object",
            name: "compact",
            attributes: r#"{"team":"platform"}"#,
        },
        AttributeGroupCase {
            label: "pretty-printed with tabs and newlines",
            name: "pretty.printed",
            attributes: "{\n\t\"team\" : \"platform\",\r\n\t\"tier\" : 1\n}",
        },
        AttributeGroupCase {
            label: "nested arrays and objects",
            name: "nested-doc",
            attributes: r#"{"a":{"b":[1,2,{"c":null}]},"d":true}"#,
        },
        AttributeGroupCase {
            label: "latin-1 characters the pattern allows",
            name: "latin_1",
            attributes: "{\"owner\":\"café-niño\",\"sign\":\"£\u{00ff}\"}",
        },
        AttributeGroupCase {
            label: "shortest document the model allows",
            name: "one-char",
            attributes: "x",
        },
    ]
}

/// A created attribute group reads back with its `attributes` byte-for-byte, through every
/// label form.
#[tokio::test]
async fn a_created_attribute_group_reads_back_with_its_attributes_verbatim() {
    let router = router();

    for (row, case) in attribute_group_cases().iter().enumerate() {
        let (status, created) = call(
            &router,
            "POST",
            "/attribute-groups",
            Some(serde_json::json!({
                "name": case.name,
                "attributes": case.attributes,
                "description": "a group",
                "tags": { "env": "dev" },
                "clientToken": format!("ag-{row}"),
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateAttributeGroup, got {status} with {created}",
            case.label
        );

        let id = as_str(&created, "/attributeGroup/id");
        assert!(
            is_application_id(&id),
            "case {:?}: expected a minted 26-character lowercase-alnum id, got {id:?}",
            case.label
        );
        assert_eq!(
            as_str(&created, "/attributeGroup/arn"),
            attribute_group_arn(&id),
            "case {:?}: the ARN must address the minted id in the request's region",
            case.label
        );
        assert_eq!(
            json_at(&created, "/attributeGroup/creationTime"),
            json_at(&created, "/attributeGroup/lastUpdateTime"),
            "case {:?}: a newly created attribute group's lastUpdateTime is its \
             creationTime",
            case.label
        );

        let arn = attribute_group_arn(&id);
        for label in [case.name, id.as_str(), arn.as_str()] {
            let (status, read) = call(
                &router,
                "GET",
                &format!("/attribute-groups/{}", encode_label(label)),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected 200 reading the group back by {label:?}, got {status} \
                 with {read}",
                case.label
            );
            assert_eq!(
                as_str(&read, "/attributes"),
                case.attributes,
                "case {:?}: attributes must come back byte-for-byte as stored",
                case.label
            );
            assert_eq!(
                as_str(&read, "/id"),
                id,
                "case {:?}: the {label:?} label must resolve to the created group",
                case.label
            );
            assert_eq!(
                read.get("tags").cloned(),
                Some(serde_json::json!({ "env": "dev" })),
                "case {:?}: the read-back tags must be the created ones",
                case.label
            );
        }

        // The name is taken now, whatever token asks for it.
        let (status, conflict) = call(
            &router,
            "POST",
            "/attribute-groups",
            Some(serde_json::json!({
                "name": case.name,
                "attributes": case.attributes,
                "clientToken": format!("ag-dup-{row}"),
            })),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {:?}: an attribute group name already in use must conflict, got {status} \
             with {conflict}",
            case.label
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {:?}: expected ConflictException, got {conflict}",
            case.label
        );
    }
}

/// Associating attribute groups records the links in state: the response ARNs come from
/// the stored records, the listing reports every linked group at any page size, and an
/// unknown side of the pair is a 404.
#[tokio::test]
async fn associating_attribute_groups_links_them_and_lists_them() {
    let router = router();

    let (status, application) = call(
        &router,
        "POST",
        "/applications",
        Some(serde_json::json!({ "name": "linked-app", "clientToken": "link-app" })),
    )
    .await;
    assert_eq!(status, 201, "expected 201 creating the application, got {application}");
    let application_id = as_str(&application, "/application/id");

    // Three groups, associated by three different label forms — name, id and ARN.
    let mut group_ids = Vec::new();
    for (row, name) in ["group-a", "group.b", "group_c"].iter().enumerate() {
        let (status, group) = call(
            &router,
            "POST",
            "/attribute-groups",
            Some(serde_json::json!({
                "name": name,
                "attributes": format!("{{\"n\":{row}}}"),
                "clientToken": format!("link-ag-{row}"),
            })),
        )
        .await;
        assert_eq!(status, 201, "expected 201 creating {name}, got {group}");
        group_ids.push(as_str(&group, "/attributeGroup/id"));
    }

    let application_labels = [
        "linked-app".to_string(),
        application_id.clone(),
        application_arn(&application_id),
    ];
    let group_labels = [
        "group-a".to_string(),
        group_ids[1].clone(),
        attribute_group_arn(&group_ids[2]),
    ];
    for (row, (application_label, group_label)) in application_labels
        .iter()
        .zip(group_labels.iter())
        .enumerate()
    {
        let (status, associated) = call(
            &router,
            "PUT",
            &format!(
                "/applications/{}/attribute-groups/{}",
                encode_label(application_label),
                encode_label(group_label)
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {application_label:?} + {group_label:?}: expected 200 from \
             AssociateAttributeGroup, got {status} with {associated}"
        );
        assert_eq!(
            as_str(&associated, "/applicationArn"),
            application_arn(&application_id),
            "case {application_label:?}: applicationArn must be resolved from the stored \
             application, not echoed from the path"
        );
        assert_eq!(
            as_str(&associated, "/attributeGroupArn"),
            attribute_group_arn(&group_ids[row]),
            "case {group_label:?}: attributeGroupArn must be resolved from the stored \
             attribute group, not echoed from the path"
        );
    }

    let expected: BTreeSet<String> = group_ids.iter().cloned().collect();
    for page_size in [1_usize, 2, 3, 4] {
        let mut seen = BTreeSet::new();
        let mut token: Option<String> = None;
        loop {
            let path = match &token {
                None => format!("/applications/linked-app/attribute-groups?maxResults={page_size}"),
                Some(token) => format!(
                    "/applications/linked-app/attribute-groups?maxResults={page_size}&nextToken={token}"
                ),
            };
            let (status, body) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected 200 from \
                 ListAssociatedAttributeGroups, got {status} with {body}"
            );
            let page = body
                .pointer("/attributeGroups")
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| panic!("case page size {page_size}: no list in {body}"));
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page must hold at most maxResults items, got {}",
                page.len()
            );
            for id in page {
                let id = id.as_str().expect("an attribute group id is a string").to_string();
                assert!(
                    seen.insert(id.clone()),
                    "case page size {page_size}: {id} was listed twice"
                );
            }
            match body.get("nextToken").and_then(serde_json::Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case page size {page_size}: the listing must report exactly the associated \
             groups"
        );
    }

    // Associating the same pair again leaves one link, not two.
    let (status, _) = call(
        &router,
        "PUT",
        "/applications/linked-app/attribute-groups/group-a",
        None,
    )
    .await;
    assert_eq!(status, 200, "re-associating an existing pair must succeed");
    let (_, body) = call(
        &router,
        "GET",
        "/applications/linked-app/attribute-groups",
        None,
    )
    .await;
    assert_eq!(
        body.pointer("/attributeGroups")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len),
        group_ids.len(),
        "re-associating an existing pair must not add a second link: {body}"
    );

    // Either side missing is a 404.
    for (label, path) in [
        (
            "unknown attribute group",
            "/applications/linked-app/attribute-groups/does-not-exist".to_string(),
        ),
        (
            "unknown application",
            "/applications/no-such-app/attribute-groups/group-a".to_string(),
        ),
    ] {
        let (status, body) = call(&router, "PUT", &path, None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
    }
    let (status, body) = call(
        &router,
        "GET",
        "/applications/no-such-app/attribute-groups",
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "listing the attribute groups of an application that does not exist must be 404, \
         got {status} with {body}"
    );
}

/// Deleting an application answers with the summary of the record as it was, and removes
/// it from every reader: the delete is not reversible and not repeatable.
#[tokio::test]
async fn deleting_an_application_removes_it_from_every_reader() {
    let router = router();

    for (row, case) in application_cases().iter().enumerate() {
        let (status, created) = call(
            &router,
            "POST",
            "/applications",
            Some(create_application_body(case, &format!("delete-{row}"))),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 creating the application, got {created}",
            case.label
        );
        let id = as_str(&created, "/application/id");

        let (status, deleted) = call(
            &router,
            "DELETE",
            &format!("/applications/{}", case.name),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from DeleteApplication, got {status} with {deleted}",
            case.label
        );
        assert_eq!(
            as_str(&deleted, "/application/id"),
            id,
            "case {:?}: the summary returned must be the application that was removed",
            case.label
        );
        assert_eq!(
            as_str(&deleted, "/application/arn"),
            application_arn(&id),
            "case {:?}: the summary must carry the stored ARN",
            case.label
        );
        assert_eq!(
            json_at(&deleted, "/application/creationTime"),
            json_at(&created, "/application/creationTime"),
            "case {:?}: the summary must be the record as it was before removal",
            case.label
        );

        // Every reader now agrees it is gone, by every label form.
        for label in [case.name, id.as_str()] {
            let (status, body) =
                call(&router, "GET", &format!("/applications/{label}"), None).await;
            assert_eq!(
                status, 404,
                "case {:?}: reading {label:?} back after delete must be 404, got {status} \
                 with {body}",
                case.label
            );
        }
        let (status, body) = call(
            &router,
            "DELETE",
            &format!("/applications/{}", case.name),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {:?}: a second delete must be 404, got {status} with {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException, got {body}",
            case.label
        );

        let (_, listed) = call(&router, "GET", "/applications", None).await;
        let still_listed = listed
            .pointer("/applications")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|applications| {
                applications
                    .iter()
                    .any(|summary| summary.get("id").and_then(serde_json::Value::as_str) == Some(id.as_str()))
            });
        assert!(
            !still_listed,
            "case {:?}: a deleted application must not be listed: {listed}",
            case.label
        );

        // The name is free again, and the token that minted it creates a new application.
        let (status, recreated) = call(
            &router,
            "POST",
            "/applications",
            Some(create_application_body(case, &format!("delete-{row}"))),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: the name must be free after the delete, got {status} with \
             {recreated}",
            case.label
        );
        assert_ne!(
            as_str(&recreated, "/application/id"),
            id,
            "case {:?}: re-creating after a delete must mint a new id, not resurrect the \
             deleted one",
            case.label
        );
    }
}

/// An application that still holds associations is NOT deleted: the model's own
/// documentation for DeleteApplication says "All associated attribute groups and resources
/// must be disassociated from it before deleting an application", so the request is refused
/// with the operation's modelled client-input error and the application survives.
#[tokio::test]
async fn an_application_with_associations_is_not_deleted() {
    let router = router();

    let (status, application) = call(
        &router,
        "POST",
        "/applications",
        Some(serde_json::json!({ "name": "still-linked", "clientToken": "linked-1" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 creating the application, got {application}"
    );
    let application_id = as_str(&application, "/application/id");

    let (status, group) = call(
        &router,
        "POST",
        "/attribute-groups",
        Some(serde_json::json!({
            "name": "still-linked-ag",
            "attributes": r#"{"k":"v"}"#,
            "clientToken": "linked-ag-1",
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 creating the attribute group, got {group}"
    );

    let (status, associated) = call(
        &router,
        "PUT",
        "/applications/still-linked/attribute-groups/still-linked-ag",
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 associating the pair, got {associated}"
    );

    for label in ["still-linked", application_id.as_str()] {
        let (status, refused) = call(
            &router,
            "DELETE",
            &format!("/applications/{}", encode_label(label)),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: deleting an application that still holds associations must be \
             refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {refused}"
        );
    }

    // And the refusal left the record exactly where it was.
    let (status, read) = call(&router, "GET", "/applications/still-linked", None).await;
    assert_eq!(
        status, 200,
        "a refused delete must leave the application readable, got {status} with {read}"
    );
    assert_eq!(
        as_str(&read, "/id"),
        application_id,
        "a refused delete must not change the stored record: {read}"
    );
}

/// Both resource stores are BOUNDED: at the cap a create answers the modelled
/// `ServiceQuotaExceededException` instead of growing the store, and stores nothing.
///
/// The store is seeded directly through the shared engine rather than by issuing a thousand
/// requests, because what is under test is the bound, not the create path.
#[tokio::test]
async fn a_full_store_answers_the_modelled_quota_error() {
    let now = std::time::SystemTime::now();

    // Row 1: the application store at its cap. Row 2: the attribute group store at its cap.
    for (label, fill_applications) in [("applications", true), ("attribute-groups", false)] {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        {
            let handle = engine.state::<AppRegistryState>();
            let mut store = handle.lock();
            if fill_applications {
                for index in 0..APPLICATION_QUOTA {
                    let id = format!("seed{index:022}");
                    store.applications.insert(
                        id.clone(),
                        ApplicationRecord {
                            arn: application_arn(&id),
                            id,
                            name: format!("seed-{index}"),
                            description: None,
                            creation_time: now,
                            last_update_time: now,
                            tags: std::collections::BTreeMap::new(),
                            attribute_group_ids: BTreeSet::new(),
                            resources: BTreeSet::new(),
                            application_tag: std::collections::BTreeMap::new(),
                        },
                    );
                }
            } else {
                for index in 0..ATTRIBUTE_GROUP_QUOTA {
                    let id = format!("seed{index:022}");
                    store.attribute_groups.insert(
                        id.clone(),
                        AttributeGroupRecord {
                            arn: attribute_group_arn(&id),
                            id,
                            name: format!("seed-{index}"),
                            description: None,
                            attributes: "{}".to_string(),
                            creation_time: now,
                            last_update_time: now,
                            tags: std::collections::BTreeMap::new(),
                            created_by: None,
                        },
                    );
                }
            }
        }
        let router = router_on(Arc::clone(&engine));

        let (path, body) = if fill_applications {
            (
                "/applications",
                serde_json::json!({ "name": "one-too-many", "clientToken": "quota-1" }),
            )
        } else {
            (
                "/attribute-groups",
                serde_json::json!({
                    "name": "one-too-many",
                    "attributes": "{}",
                    "clientToken": "quota-1",
                }),
            )
        };
        let (status, refused) = call(&router, "POST", path, Some(body)).await;
        assert_eq!(
            status, 402,
            "case {label:?}: a full store must answer the modelled quota error, got \
             {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ServiceQuotaExceededException",
            "case {label:?}: expected ServiceQuotaExceededException, got {refused}"
        );

        let handle = engine.state::<AppRegistryState>();
        let store = handle.lock();
        assert_eq!(
            (store.applications.len(), store.attribute_groups.len()),
            if fill_applications {
                (APPLICATION_QUOTA, 0)
            } else {
                (0, ATTRIBUTE_GROUP_QUOTA)
            },
            "case {label:?}: a refused create must store nothing"
        );
    }

    // The idempotency map is bounded independently of the resource stores, because its
    // keys are caller-chosen: past the cap the oldest record is forgotten and the newest
    // is kept, so the map cannot grow with the number of distinct tokens seen.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let handle = engine.state::<AppRegistryState>();
    let over_the_cap = IDEMPOTENCY_TOKEN_LIMIT + 500;
    {
        let mut store = handle.lock();
        for index in 0..over_the_cap {
            store.remember_token(
                format!("token-{index}"),
                ResourceKind::Application,
                format!("seed{index:022}"),
                index as u64,
            );
        }
    }
    let store = handle.lock();
    assert_eq!(
        store.idempotency.len(),
        IDEMPOTENCY_TOKEN_LIMIT,
        "the idempotency map must stay at its cap after {over_the_cap} distinct tokens"
    );
    assert!(
        store
            .idempotency
            .contains_key(&format!("token-{}", over_the_cap - 1)),
        "the newest token must be the one that is kept"
    );
    assert!(
        !store.idempotency.contains_key("token-0"),
        "the oldest token must be the one that is forgotten"
    );
}

