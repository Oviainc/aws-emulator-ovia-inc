// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource`, end to end over the wire.
//!
//! The rule under test: ECR's tag reads address their resource by ARN — "currently, the
//! only supported resource is an Amazon ECR repository" — so the answer is exactly the
//! tag set persisted on the repository that ARN names, whatever that set is, while an
//! ARN that is not an ECR repository ARN of this registry is refused as an invalid
//! parameter and a well-formed ARN for a repository the store does not hold is refused
//! as a missing repository.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service. It is what routes a request to ECR,
/// and its region is what a repository ARN is built for.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/ecr/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
const REGION: &str = "eu-west-1";

/// The account id every generated emulator vends — the registry id of every repository
/// here. Spelled out rather than imported so a wrong registry id in the implementation
/// fails this test.
const REGISTRY_ID: &str = "000000000000";

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// "Tag keys can have a maximum character length of 128 characters, and tag values can
/// have a maximum length of 256 characters" — `InvalidTagParameterException`'s own
/// documentation in this service's Smithy model.
const MAX_TAG_KEY_LEN: usize = 128;
/// See [`MAX_TAG_KEY_LEN`].
const MAX_TAG_VALUE_LEN: usize = 256;

/// "The maximum number of tags that can be applied to a repository is 50"
/// (`TooManyTagsException`'s own documentation in this service's Smithy model).
const MAX_TAGS: usize = 50;

/// One row of the tag table: the case name, the repository name, and the tags the create
/// carries — `None` omitting the member entirely, `Some(pairs)` sending exactly those.
type TagCase = (&'static str, &'static str, Option<Vec<(String, String)>>);

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
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
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

/// The wire form of a tag list: the model's members are `Key` and `Value`.
fn tag_list(pairs: &[(String, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// The `(Key, Value)` pairs a `ListTagsForResource` answer reports, as a set, plus how
/// many entries the list carried (so a dropped duplicate is visible).
fn reported_tags(case: &str, body: &Value) -> (BTreeSet<(String, String)>, usize) {
    let entries = body["tags"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: expected a `tags` list in the answer, got {body:?}")
    });
    let pairs = entries
        .iter()
        .map(|entry| {
            let key = entry["Key"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a reported tag carries no Key: {entry:?}")
            });
            let value = entry["Value"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a reported tag carries no Value: {entry:?}")
            });
            (key.to_string(), value.to_string())
        })
        .collect();
    (pairs, entries.len())
}

/// Create `name` with `tags` and answer the ARN the create reported, which is the ARN a
/// client holds for the repository.
async fn given_repository(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    tags: &Option<Vec<(String, String)>>,
) -> String {
    let mut request = json!({"repositoryName": name});
    if let Some(pairs) = tags {
        request["tags"] = tag_list(pairs);
    }
    let created = ok(router, case, "CreateRepository", &request).await;
    let arn = created["repository"]["repositoryArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create reported no repositoryArn: {created:?}"))
        .to_string();
    assert_eq!(
        arn,
        format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/{name}"),
        "case {case:?}: expected the repository ARN to name this registry and region, got {arn:?}"
    );
    arn
}

/// The tags reported are the tags stored, for every shape of tag set a create can carry:
/// none at all, an explicitly empty list, keys and values at the documented maxima,
/// non-ASCII text, a colon-bearing value, and the documented maximum of 50 tags.
#[tokio::test]
async fn the_tags_reported_are_the_ones_the_repository_was_created_with() {
    let router = emulator();

    let max_length = vec![(
        "k".repeat(MAX_TAG_KEY_LEN),
        "v".repeat(MAX_TAG_VALUE_LEN),
    )];
    let fifty: Vec<(String, String)> = (0..MAX_TAGS)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let pairs = |rows: &[(&str, &str)]| -> Vec<(String, String)> {
        rows.iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    };

    let cases: Vec<TagCase> = vec![
        ("no tags member at all", "untagged", None),
        ("an explicitly empty tag list", "empty-tag-list", Some(Vec::new())),
        (
            "a single tag",
            "single-tag",
            Some(pairs(&[("env", "production")])),
        ),
        (
            "several tags, their keys not in sorted order",
            "several-tags",
            Some(pairs(&[
                ("zone", "b"),
                ("Application", "Payments"),
                ("cost-centre", "4711"),
            ])),
        ),
        (
            "a tag whose value is the empty string",
            "empty-value",
            Some(pairs(&[("retired", "")])),
        ),
        (
            "a key and a value at the documented maxima (128 / 256 characters)",
            "boundary-lengths",
            Some(max_length),
        ),
        (
            "non-ASCII key and value",
            "unicode-tags",
            Some(pairs(&[("проект", "données-🗄")])),
        ),
        (
            "a value that looks like an ARN, which must not be re-parsed",
            "colon-value",
            Some(pairs(&[(
                "source",
                "arn:aws:ecr:eu-west-1:000000000000:repository/elsewhere",
            )])),
        ),
        (
            "a namespaced repository name, so the ARN's resource carries a slash",
            "project-a/nginx-web-app",
            Some(pairs(&[("team", "platform")])),
        ),
        ("the documented maximum of 50 tags", "fifty-tags", Some(fifty)),
    ];

    for (case, name, tags) in &cases {
        let arn = given_repository(&router, case, name, tags).await;
        let expected: BTreeSet<(String, String)> =
            tags.clone().unwrap_or_default().into_iter().collect();

        let body = ok(
            &router,
            case,
            "ListTagsForResource",
            &json!({"resourceArn": arn}),
        )
        .await;
        let (reported, count) = reported_tags(case, &body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the stored tags {expected:?}, got {reported:?}"
        );
        assert_eq!(
            count,
            expected.len(),
            "case {case:?}: expected {} reported tags, got {count}",
            expected.len()
        );

        // A read: a second call answers the same thing, and the repository is still
        // described exactly as it was created.
        let again = ok(
            &router,
            case,
            "ListTagsForResource",
            &json!({"resourceArn": arn}),
        )
        .await;
        let (repeated, repeated_count) = reported_tags(case, &again);
        assert_eq!(
            repeated, reported,
            "case {case:?}: a second read reported {repeated:?}, not {reported:?}"
        );
        assert_eq!(
            repeated_count, count,
            "case {case:?}: a second read reported {repeated_count} tags, not {count}"
        );
    }

    // Reading one repository's tags left every other repository's alone.
    let described = ok(
        &router,
        "every created repository is still described",
        "DescribeRepositories",
        &json!({}),
    )
    .await;
    let described_names: BTreeSet<String> = described["repositories"]
        .as_array()
        .expect("DescribeRepositories reports a list")
        .iter()
        .map(|repo| repo["repositoryName"].as_str().unwrap_or_default().to_string())
        .collect();
    let created_names: BTreeSet<String> =
        cases.iter().map(|(_, name, _)| (*name).to_string()).collect();
    assert_eq!(
        described_names, created_names,
        "expected the created repositories {created_names:?} to survive the reads, got \
         {described_names:?}"
    );
}

/// A well-formed ECR repository ARN whose repository this registry does not hold is
/// refused with the modelled `RepositoryNotFoundException`, including after the
/// repository it named was deleted — the answer tracks live state, not the ARN's shape.
#[tokio::test]
async fn an_arn_naming_no_stored_repository_is_refused() {
    let router = emulator();
    let tags = Some(vec![("env".to_string(), "staging".to_string())]);
    let arn = given_repository(&router, "a repository to delete", "doomed", &tags).await;

    let listed = ok(
        &router,
        "before the delete",
        "ListTagsForResource",
        &json!({"resourceArn": arn}),
    )
    .await;
    let (reported, _) = reported_tags("before the delete", &listed);
    assert_eq!(
        reported,
        BTreeSet::from([("env".to_string(), "staging".to_string())]),
        "case \"before the delete\": expected the stored tag, got {reported:?}"
    );

    ok(
        &router,
        "delete the repository",
        "DeleteRepository",
        &json!({"repositoryName": "doomed"}),
    )
    .await;

    let cases: Vec<(&str, String)> = vec![
        ("the ARN of a deleted repository", arn),
        (
            "an ARN for a repository that was never created",
            format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/never-created"),
        ),
        (
            "an ARN for a namespaced repository that was never created",
            format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/team-a/never-created"),
        ),
        (
            "an ARN in another region for a repository this registry does not hold",
            format!("arn:aws:ecr:us-east-2:{REGISTRY_ID}:repository/never-created"),
        ),
    ];

    for (case, resource_arn) in cases {
        let answer = call(
            &router,
            "ListTagsForResource",
            &json!({"resourceArn": resource_arn}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "RepositoryNotFoundException",
            "case {case:?}: expected RepositoryNotFoundException, got {:?}",
            answer.body
        );
    }
}

/// An ARN that is not an ECR repository ARN of this registry is refused with the
/// modelled `InvalidParameterException`: the only supported resource is an Amazon ECR
/// repository, and the only registry served here is this emulator's.
#[tokio::test]
async fn an_arn_that_is_not_this_registrys_repository_is_refused() {
    let router = emulator();
    given_repository(&router, "a repository that does exist", "present", &None).await;

    let cases: Vec<(&str, String)> = vec![
        ("the empty string", String::new()),
        ("text that is not an ARN at all", "present".to_string()),
        (
            "an ARN with too few fields",
            format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}"),
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::my-bucket".to_string(),
        ),
        (
            "another ECR resource type",
            format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:image/present"),
        ),
        (
            "the repository resource type with no name",
            format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/"),
        ),
        (
            "a resource field carrying no resource type",
            format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:present"),
        ),
        (
            "an empty region",
            format!("arn:aws:ecr::{REGISTRY_ID}:repository/present"),
        ),
        (
            "an empty partition",
            format!("arn::ecr:{REGION}:{REGISTRY_ID}:repository/present"),
        ),
        (
            "another account's registry",
            format!("arn:aws:ecr:{REGION}:123456789012:repository/present"),
        ),
    ];

    for (case, resource_arn) in cases {
        let answer = call(
            &router,
            "ListTagsForResource",
            &json!({"resourceArn": resource_arn}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {:?}",
            answer.body
        );
    }
}
