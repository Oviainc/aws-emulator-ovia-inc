// SPDX-License-Identifier: CC0-1.0

//! `PutImageTagMutability` over the wire: the repository's tag-mutability
//! configuration is whatever the last call stored.
//!
//! The rule under test: a put REPLACES the repository's tag-mutability configuration
//! with the setting and the exclusion filters the request carries (filters it omits are
//! gone), the answer is read back out of that record, and every later push decides
//! whether a tag may be overwritten from it.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service; it is what routes a request to ECR.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/ecr/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The account id every generated emulator vends, spelled out here rather than imported
/// so a wrong registry id in the implementation fails this test.
const REGISTRY_ID: &str = "000000000000";

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// Every value `ImageTagMutability` admits, from the model's enum
/// (`smithy_query(query="shape",shape="ImageTagMutability")`). Spelled out here so a
/// value the put never exercises is visible as a gap.
const IMAGE_TAG_MUTABILITY: [&str; 4] = [
    "MUTABLE",
    "IMMUTABLE",
    "MUTABLE_WITH_EXCLUSION",
    "IMMUTABLE_WITH_EXCLUSION",
];

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation of this service on the awsJson1_1 wire: `POST /` with
/// `X-Amz-Target: <prefix>.<operation>` and a JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "api.ecr.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A successful call, or a panic naming the case and what came back instead.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// The value at `pointer`, or `Null` when the member is absent.
fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// The one repository `DescribeRepositories` reports for `name`.
async fn described(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let body = ok(
        router,
        case,
        "DescribeRepositories",
        &json!({"repositoryNames": [name]}),
    )
    .await;
    at(&body, "/repositories/0")
}

/// Two exclusion filters: one wildcard pattern and one literal tag. `*` is the only
/// metacharacter `ImageTagMutabilityExclusionFilterValue`'s `^[0-9a-zA-Z._*-]{1,128}$`
/// admits.
fn two_filters() -> Value {
    json!([
        {"filterType": "WILDCARD", "filter": "release-*"},
        {"filterType": "WILDCARD", "filter": "dev"},
    ])
}

/// A put reports the configuration it stored, `DescribeRepositories` reports the same
/// one, and a second put replaces it — including dropping filters the new call omits.
///
/// The rows differ along what the rule generalises over: the setting put, how many
/// exclusion filters come with it, and the setting the repository was created with.
#[tokio::test]
async fn a_put_replaces_the_stored_tag_mutability_configuration() {
    let cases: Vec<(&str, Value, Value)> = vec![
        // case, the repository's create request, the put request's settings
        (
            "immutable over the created default",
            json!({}),
            json!({"imageTagMutability": "IMMUTABLE"}),
        ),
        (
            "mutable over a repository created immutable",
            json!({"imageTagMutability": "IMMUTABLE"}),
            json!({"imageTagMutability": "MUTABLE"}),
        ),
        (
            "mutable with one exclusion filter",
            json!({}),
            json!({
                "imageTagMutability": "MUTABLE_WITH_EXCLUSION",
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "prod"},
                ],
            }),
        ),
        (
            "immutable with the maximum of five exclusion filters",
            json!({"imageTagMutability": "MUTABLE_WITH_EXCLUSION",
                   "imageTagMutabilityExclusionFilters": [
                       {"filterType": "WILDCARD", "filter": "stale"},
                   ]}),
            json!({
                "imageTagMutability": "IMMUTABLE_WITH_EXCLUSION",
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "a-*"},
                    {"filterType": "WILDCARD", "filter": "b.*"},
                    {"filterType": "WILDCARD", "filter": "c_1"},
                    {"filterType": "WILDCARD", "filter": "*-nightly"},
                    {"filterType": "WILDCARD", "filter": "d"},
                ],
            }),
        ),
        (
            "the same setting put again, over a namespaced repository name",
            json!({"imageTagMutability": "IMMUTABLE"}),
            json!({"imageTagMutability": "IMMUTABLE"}),
        ),
    ];

    // Every modelled setting is put by some row.
    for value in IMAGE_TAG_MUTABILITY {
        assert!(
            cases
                .iter()
                .any(|(_, _, put)| put["imageTagMutability"] == json!(value)),
            "no case puts the {value} tag mutability setting"
        );
    }

    for (index, (case, create, put)) in cases.into_iter().enumerate() {
        let router = emulator();
        let name = format!("project-{index}/app");
        let mut create = create;
        create["repositoryName"] = json!(name);
        ok(&router, case, "CreateRepository", &create).await;

        let mut request = put.clone();
        request["repositoryName"] = json!(name);
        let answer = ok(&router, case, "PutImageTagMutability", &request).await;

        let expected_filters = put
            .get("imageTagMutabilityExclusionFilters")
            .cloned()
            .unwrap_or(Value::Null);
        for (pointer, expected) in [
            ("/registryId", json!(REGISTRY_ID)),
            ("/repositoryName", json!(name)),
            ("/imageTagMutability", put["imageTagMutability"].clone()),
            (
                "/imageTagMutabilityExclusionFilters",
                expected_filters.clone(),
            ),
        ] {
            assert_eq!(
                at(&answer, pointer),
                expected,
                "case {case:?}: expected {pointer} to be {expected:?} in the put's answer, \
                 got {:?}",
                at(&answer, pointer)
            );
        }

        // The read reports the record the put wrote, not the create's configuration.
        let repository = described(&router, case, &name).await;
        assert_eq!(
            at(&repository, "/imageTagMutability"),
            put["imageTagMutability"],
            "case {case:?}: DescribeRepositories must report the setting the put stored, \
             got {:?}",
            at(&repository, "/imageTagMutability")
        );
        assert_eq!(
            at(&repository, "/imageTagMutabilityExclusionFilters"),
            expected_filters,
            "case {case:?}: DescribeRepositories must report the filters the put stored, \
             got {:?}",
            at(&repository, "/imageTagMutabilityExclusionFilters")
        );
        // Nothing else about the repository moved.
        assert_eq!(
            at(&repository, "/repositoryName"),
            json!(name),
            "case {case:?}: the put must not rename the repository, got {:?}",
            at(&repository, "/repositoryName")
        );

        // A put that names no filters leaves none behind: the configuration is
        // replaced, not merged.
        ok(
            &router,
            case,
            "PutImageTagMutability",
            &json!({"repositoryName": name, "imageTagMutability": "MUTABLE"}),
        )
        .await;
        let repository = described(&router, case, &name).await;
        assert_eq!(
            at(&repository, "/imageTagMutability"),
            json!("MUTABLE"),
            "case {case:?}: the last put's setting is the repository's, got {:?}",
            at(&repository, "/imageTagMutability")
        );
        assert_eq!(
            at(&repository, "/imageTagMutabilityExclusionFilters"),
            Value::Null,
            "case {case:?}: a put naming no filters leaves none stored, got {:?}",
            at(&repository, "/imageTagMutabilityExclusionFilters")
        );
    }
}

/// What the put stored is what decides a later push: the tag moves only where the
/// configuration the put wrote allows that tag to be overwritten.
///
/// The rows span every setting, and for the two `*_WITH_EXCLUSION` ones both a tag the
/// filters match and one they do not.
#[tokio::test]
async fn the_setting_a_put_stored_decides_whether_a_later_push_may_move_a_tag() {
    let cases: Vec<(&str, &str, Value, &str, bool)> = vec![
        // case, the setting put, the filters put, the tag pushed, the tag may move
        ("mutable", "MUTABLE", Value::Null, "v1", true),
        ("immutable", "IMMUTABLE", Value::Null, "v1", false),
        (
            "mutable except a matching wildcard filter",
            "MUTABLE_WITH_EXCLUSION",
            two_filters(),
            "release-7",
            false,
        ),
        (
            "mutable except a matching literal filter",
            "MUTABLE_WITH_EXCLUSION",
            two_filters(),
            "dev",
            false,
        ),
        (
            "mutable, and the filters match the tag in neither form",
            "MUTABLE_WITH_EXCLUSION",
            two_filters(),
            "v1",
            true,
        ),
        (
            "immutable except a matching wildcard filter",
            "IMMUTABLE_WITH_EXCLUSION",
            two_filters(),
            "release-7",
            true,
        ),
        (
            "immutable, and the filters match nothing",
            "IMMUTABLE_WITH_EXCLUSION",
            two_filters(),
            "v1",
            false,
        ),
    ];

    for value in IMAGE_TAG_MUTABILITY {
        assert!(
            cases.iter().any(|(_, setting, ..)| *setting == value),
            "no case puts the {value} tag mutability setting"
        );
    }

    for (index, (case, setting, filters, tag, may_move)) in cases.into_iter().enumerate() {
        let router = emulator();
        let name = format!("pushed-{index}");
        // Created with the OPPOSITE kind of setting, so only the put can explain the
        // push's outcome.
        let created = if setting.starts_with("MUTABLE") {
            "IMMUTABLE"
        } else {
            "MUTABLE"
        };
        ok(
            &router,
            case,
            "CreateRepository",
            &json!({"repositoryName": name, "imageTagMutability": created}),
        )
        .await;

        let mut put = json!({"repositoryName": name, "imageTagMutability": setting});
        if !filters.is_null() {
            put["imageTagMutabilityExclusionFilters"] = filters;
        }
        ok(&router, case, "PutImageTagMutability", &put).await;

        let first = format!("{{\"schemaVersion\":2,\"image\":\"first-{index}\"}}");
        let second = format!("{{\"schemaVersion\":2,\"image\":\"second-{index}\"}}");
        ok(
            &router,
            case,
            "PutImage",
            &json!({"repositoryName": name, "imageManifest": first, "imageTag": tag}),
        )
        .await;
        let moved = call(
            &router,
            "PutImage",
            &json!({"repositoryName": name, "imageManifest": second, "imageTag": tag}),
        )
        .await;

        if may_move {
            assert_eq!(
                moved.status, 200,
                "case {case:?}: the put allows tag {tag:?} to move, got {} with {:?}",
                moved.status, moved.body
            );
        } else {
            assert_eq!(
                moved.status, 400,
                "case {case:?}: the put forbids tag {tag:?} from moving, got {} with {:?}",
                moved.status, moved.body
            );
            assert_eq!(
                moved.error_code(),
                "ImageTagAlreadyExistsException",
                "case {case:?}: expected ImageTagAlreadyExistsException, got {:?}",
                moved.body
            );
        }
    }
}

/// A put the registry cannot honour is refused with the error the model declares for
/// that kind of refusal, and stores nothing.
#[tokio::test]
async fn a_put_the_registry_cannot_honour_is_refused() {
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository the registry does not hold",
            json!({"repositoryName": "missing-repo", "imageTagMutability": "IMMUTABLE"}),
            "RepositoryNotFoundException",
        ),
        (
            "another account's registry",
            json!({
                "repositoryName": "present",
                "registryId": "123456789012",
                "imageTagMutability": "IMMUTABLE",
            }),
            "InvalidParameterException",
        ),
        (
            // The boundary validator refuses a value outside `ImageTagMutability`'s enum
            // before the handler runs, and this service states such a refusal with the
            // client-input error its model declares (`override_refusal` in
            // `src/service.rs`).
            "a setting outside the modelled enum",
            json!({"repositoryName": "present", "imageTagMutability": "immutable"}),
            "InvalidParameterException",
        ),
        (
            "more exclusion filters than the modelled maximum of five",
            json!({
                "repositoryName": "present",
                "imageTagMutability": "IMMUTABLE_WITH_EXCLUSION",
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "a"},
                    {"filterType": "WILDCARD", "filter": "b"},
                    {"filterType": "WILDCARD", "filter": "c"},
                    {"filterType": "WILDCARD", "filter": "d"},
                    {"filterType": "WILDCARD", "filter": "e"},
                    {"filterType": "WILDCARD", "filter": "f"},
                ],
            }),
            "InvalidParameterException",
        ),
        (
            "exclusion filters beside a setting that excludes nothing",
            json!({
                "repositoryName": "present",
                "imageTagMutability": "MUTABLE",
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "release-*"},
                ],
            }),
            "InvalidParameterException",
        ),
        (
            "exclusion filters beside the plain immutable setting",
            json!({
                "repositoryName": "present",
                "imageTagMutability": "IMMUTABLE",
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "release-*"},
                ],
            }),
            "InvalidParameterException",
        ),
    ];

    let router = emulator();
    // Created with the two `*_WITH_EXCLUSION` members set, so a refused put that left
    // them changed would show up below.
    ok(
        &router,
        "the repository under test",
        "CreateRepository",
        &json!({
            "repositoryName": "present",
            "imageTagMutability": "MUTABLE_WITH_EXCLUSION",
            "imageTagMutabilityExclusionFilters": [
                {"filterType": "WILDCARD", "filter": "keep-*"},
            ],
        }),
    )
    .await;

    for (case, request, expected) in cases {
        let answer = call(&router, "PutImageTagMutability", &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected,
            "case {case:?}: expected {expected}, got {:?}",
            answer.body
        );

        // A refused put changes no stored configuration.
        let repository = described(&router, case, "present").await;
        assert_eq!(
            at(&repository, "/imageTagMutability"),
            json!("MUTABLE_WITH_EXCLUSION"),
            "case {case:?}: a refused put must leave the setting alone, got {:?}",
            at(&repository, "/imageTagMutability")
        );
        assert_eq!(
            at(&repository, "/imageTagMutabilityExclusionFilters"),
            json!([{"filterType": "WILDCARD", "filter": "keep-*"}]),
            "case {case:?}: a refused put must leave the filters alone, got {:?}",
            at(&repository, "/imageTagMutabilityExclusionFilters")
        );
    }
}
