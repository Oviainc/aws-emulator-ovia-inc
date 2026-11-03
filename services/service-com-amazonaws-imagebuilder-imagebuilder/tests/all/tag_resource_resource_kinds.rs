// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `TagResource` over the resource kinds a build produces: an
//! image pipeline and an image build version.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. The resource ARN rides in the URI path, as the
//! modelled `@httpLabel` puts it, percent-encoded the way a non-greedy label is.
//!
//! The rule under test: a tagging call writes the `tags` member of the ONE stored record
//! its ARN names, whichever kind of resource that is, merging the keys the request carries
//! into the tags that record already holds — a key it already holds taking the new value —
//! and touching nothing else: no other resource's tags, and no other tag-valued member of
//! the same record (an image pipeline's `imageTags`, the tags it applies to the images its
//! runs create, is a different member of one record). Every later read of that resource —
//! the tag listing and the resource's own read — reports exactly the tags the record then
//! carries, and an ARN the calling Region holds no resource under names nothing to tag.
//!
//! The rows span what the rule generalises over: which resource kind the ARN names, how the
//! tags first got onto the record (a create, a pipeline run, or a tagging call), how many
//! keys one call carries and what they look like, and whether a key is new or already
//! present.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// A second Region, for the cases that check a regional endpoint reaches only its own
/// Region's resources.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A build component document: one `build` phase, which is all the recipe a pipeline here
/// references needs in order to report a platform.
const BUILD_DOCUMENT: &str = "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: Greet\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hello\n";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in `region` — the signal the router routes on and
/// the handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Percent-encode one path segment, as an AWS SDK encodes a NON-greedy `@httpLabel`, so
/// the `:` and `/` of an ARN travel as `%3A` and `%2F` and the whole ARN stays ONE segment.
fn encode_label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// Read `name` out of an answer, panicking with the case when it is absent.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// Assert the call succeeded, and answer its body.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// The `__type` of an error answer, with any shape-id prefix dropped.
fn error_code(case: &str, body: &Value) -> String {
    let rendered = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a __type member in {body}"));
    rendered.rsplit('#').next().unwrap_or(rendered).to_string()
}

/// The `message` of an error answer.
fn error_message(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message member in {body}"))
        .to_string()
}

/// Assert the answer is this service's not-found error, naming `resource_type` and `arn`.
fn assert_missing(case: &str, status: u16, body: &Value, resource_type: &str, arn: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {code:?} with {body}"
    );
    let expected =
        format!("The following required resource '{resource_type}' cannot be found: '{arn}'.");
    let actual = error_message(case, body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the message {expected:?}, got {actual:?}"
    );
}

/// The pairs as the JSON object a `tags` member carries.
fn tag_object(tags: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert((*key).to_string(), json!(value));
    }
    Value::Object(map)
}

/// The pairs as the map an assertion compares against, so key order never matters.
fn tag_map(tags: &[(&str, &str)]) -> BTreeMap<String, String> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The tag map held in the `member` of `body`, as a map.
fn tags_in(case: &str, body: &Value, member: &str) -> BTreeMap<String, String> {
    let tags = body
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member in {body}"));
    let object = tags
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} to be a map, got {tags}"));
    object
        .iter()
        .map(|(key, value)| {
            let text = value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected text for tag {key}, got {value}"))
                .to_string();
            (key.clone(), text)
        })
        .collect()
}

/// `ListTagsForResource` for `arn`, in `region`.
async fn list_tags(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/tags/{}", encode_label(arn)),
        None,
    )
    .await
}

/// The tags `ListTagsForResource` reports for `arn` in [`REGION`].
async fn reported_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, body) = list_tags(router, REGION, arn).await;
    let body = assert_ok(case, status, body);
    tags_in(case, &body, "tags")
}

/// `TagResource` writing `tags` onto `arn`, in `region`.
async fn tag_resource_in(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    tags: &[(&str, &str)],
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/tags/{}", encode_label(arn)),
        Some(&json!({ "tags": tag_object(tags) })),
    )
    .await
}

/// `TagResource` writing `tags` onto `arn` in [`REGION`], asserting it was served.
async fn tag_resource(router: &MultiServiceRouter, case: &str, arn: &str, tags: &[(&str, &str)]) {
    let (status, body) = tag_resource_in(router, REGION, arn, tags).await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: expected the tagging call to be served, got {status} with {body}"
    );
}

/// `CreateComponent` for one build version of `name` in `region`, answering its build
/// version ARN.
async fn create_component(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "platform": "Linux",
        "clientToken": format!("component-{region}-{name}"),
        "data": BUILD_DOCUMENT,
    });
    let case = format!("create the component {name} in {region}");
    let (status, answer) = call_in(router, region, "PUT", "/CreateComponent", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "componentBuildVersionArn")
}

/// `CreateImageRecipe` for `name` in `region`, including `component_arn`, answering the
/// recipe's own ARN.
async fn create_recipe(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    component_arn: &str,
) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "parentImage": "ami-0123456789abcdef0",
        "clientToken": format!("recipe-{region}-{name}"),
        "components": [{ "componentArn": component_arn }],
    });
    let case = format!("create the image recipe {name} in {region}");
    let (status, answer) = call_in(router, region, "PUT", "/CreateImageRecipe", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imageRecipeArn")
}

/// `CreateInfrastructureConfiguration` for `name` in `region`, answering its ARN.
async fn create_configuration(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let body = json!({
        "name": name,
        "clientToken": format!("configuration-{region}-{name}"),
        "instanceProfileName": "EC2InstanceProfileForImageBuilder",
    });
    let case = format!("create the infrastructure configuration {name} in {region}");
    let (status, answer) = call_in(
        router,
        region,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&body),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "infrastructureConfigurationArn")
}

/// Create everything one pipeline needs in `region` and the pipeline itself, carrying
/// `tags` (the pipeline's own) and `image_tags` (the tags its runs put on the images they
/// create), and answer the pipeline's own ARN.
async fn pipeline_in(
    router: &MultiServiceRouter,
    region: &str,
    slug: &str,
    tags: &[(&str, &str)],
    image_tags: &[(&str, &str)],
) -> String {
    let component = create_component(router, region, &format!("{slug} component")).await;
    let recipe = create_recipe(router, region, &format!("{slug} recipe"), &component).await;
    let configuration = create_configuration(router, region, &format!("{slug} infra")).await;
    let mut body = Map::new();
    body.insert("name".to_string(), json!(format!("{slug} pipeline")));
    body.insert(
        "clientToken".to_string(),
        json!(format!("pipeline-{region}-{slug}")),
    );
    body.insert("imageRecipeArn".to_string(), json!(recipe));
    body.insert(
        "infrastructureConfigurationArn".to_string(),
        json!(configuration),
    );
    if !tags.is_empty() {
        body.insert("tags".to_string(), tag_object(tags));
    }
    if !image_tags.is_empty() {
        body.insert("imageTags".to_string(), tag_object(image_tags));
    }
    let case = format!("create the image pipeline {slug} in {region}");
    let (status, answer) = call_in(
        router,
        region,
        "PUT",
        "/CreateImagePipeline",
        Some(&Value::Object(body)),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imagePipelineArn")
}

/// `StartImagePipelineExecution` for `arn` in `region`, with `token` and the `tags` the run
/// puts on the image it queues, answering the build version's ARN.
async fn start_pipeline(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    token: &str,
    tags: &[(&str, &str)],
) -> String {
    let mut body = Map::new();
    body.insert("imagePipelineArn".to_string(), json!(arn));
    body.insert("clientToken".to_string(), json!(token));
    if !tags.is_empty() {
        body.insert("tags".to_string(), tag_object(tags));
    }
    let case = format!("start the pipeline run {token} in {region}");
    let (status, answer) = call_in(
        router,
        region,
        "PUT",
        "/StartImagePipelineExecution",
        Some(&Value::Object(body)),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imageBuildVersionArn")
}

/// The `imagePipeline` one `GetImagePipeline` answer carries, read in [`REGION`].
async fn read_pipeline(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, answer) = call_in(
        router,
        REGION,
        "GET",
        &format!("/GetImagePipeline?imagePipelineArn={}", encode_label(arn)),
        None,
    )
    .await;
    let answer = assert_ok(case, status, answer);
    answer
        .get("imagePipeline")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an imagePipeline member in {answer}"))
}

/// The tags the image build version `arn` reports through its OWN read: the summary
/// `ListImageBuildVersions` carries for it in [`REGION`].
async fn tags_from_build_version_listing(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, answer) = call_in(
        router,
        REGION,
        "POST",
        "/ListImageBuildVersions",
        Some(&json!({})),
    )
    .await;
    let answer = assert_ok(case, status, answer);
    let summaries = answer
        .get("imageSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected an imageSummaryList in {answer}"));
    let summary = summaries
        .iter()
        .find(|summary| summary.get("arn").and_then(Value::as_str) == Some(arn))
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary for {arn} in {answer}"));
    tags_in(case, summary, "tags")
}

/// Which kind of resource a row tags, and how its own read reports the tags it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The pipeline itself, whose create may carry the tags it starts with.
    ImagePipeline,
    /// One image build version a pipeline run queued, which starts with the tags that run
    /// carried.
    Image,
}

impl Kind {
    /// A resource of this kind in [`REGION`], starting with `tags`, and its own ARN.
    async fn create(
        self,
        router: &MultiServiceRouter,
        slug: &str,
        tags: &[(&str, &str)],
    ) -> String {
        match self {
            Self::ImagePipeline => pipeline_in(router, REGION, slug, tags, &[]).await,
            Self::Image => {
                let pipeline = pipeline_in(router, REGION, slug, &[], &[]).await;
                start_pipeline(router, REGION, &pipeline, &format!("run-{slug}"), tags).await
            }
        }
    }

    /// The tags this kind's OWN read reports for `arn`.
    async fn tags_from_own_read(
        self,
        router: &MultiServiceRouter,
        case: &str,
        arn: &str,
    ) -> BTreeMap<String, String> {
        match self {
            Self::ImagePipeline => {
                let pipeline = read_pipeline(router, case, arn).await;
                tags_in(case, &pipeline, "tags")
            }
            Self::Image => tags_from_build_version_listing(router, case, arn).await,
        }
    }

    /// The resource-type token a not-found report names a resource of this kind by.
    fn resource_type(self) -> &'static str {
        match self {
            Self::ImagePipeline => "ImagePipeline",
            Self::Image => "Image",
        }
    }

    /// A well-formed `ImageBuilderArn` of this kind that nothing ever created: the resource
    /// field each kind takes in the modelled pattern
    /// (`smithy_query(query="shape",shape="ImageBuilderArn")`), where an image build
    /// version carries three assigned version nodes and a build node.
    fn absent_arn(self) -> String {
        let resource = match self {
            Self::ImagePipeline => "image-pipeline/absent-pipeline".to_string(),
            Self::Image => "image/absent-image/1.0.0/1".to_string(),
        };
        format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:{resource}")
    }
}

/// One row of the tagging table: a resource kind, the tags the resource started with, and
/// the two tagging calls made against it afterwards.
struct Row {
    case: &'static str,
    kind: Kind,
    slug: &'static str,
    started_with: &'static [(&'static str, &'static str)],
    first_call: &'static [(&'static str, &'static str)],
    second_call: &'static [(&'static str, &'static str)],
    /// The tags the resource carries once both calls have been served.
    expected: &'static [(&'static str, &'static str)],
}

/// The tagging table. The rows differ along what the rule generalises over: the resource
/// kind, whether the record started out tagged and by which call, how many keys one call
/// carries, the shape of a key and a value (the modelled `TagKey` pattern admits spaces,
/// `.`, `:`, `/`, `=`, `+`, `-` and `@`, and `TagValue` admits the empty string), and
/// whether a later call adds a key or replaces one.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one tag on an untagged image pipeline",
            kind: Kind::ImagePipeline,
            slug: "one",
            started_with: &[],
            first_call: &[("Owner", "platform")],
            second_call: &[],
            expected: &[("Owner", "platform")],
        },
        Row {
            case: "the tags a pipeline's create carried are merged into, not replaced",
            kind: Kind::ImagePipeline,
            slug: "merged",
            started_with: &[("Source", "create"), ("Stage", "dev")],
            first_call: &[("Stage", "prod")],
            second_call: &[("Team", "builders")],
            expected: &[("Source", "create"), ("Stage", "prod"), ("Team", "builders")],
        },
        Row {
            case: "the tags a pipeline run put on its image are merged into, not replaced",
            kind: Kind::Image,
            slug: "run-tagged",
            started_with: &[("Run", "first")],
            first_call: &[("Owner", "platform"), ("Stage", "dev")],
            second_call: &[("Stage", "prod")],
            expected: &[("Run", "first"), ("Owner", "platform"), ("Stage", "prod")],
        },
        Row {
            case: "keys and values across the modelled character set on an image",
            kind: Kind::Image,
            slug: "characters",
            started_with: &[],
            first_call: &[
                ("cost center", "eng/platform"),
                ("app.version", "1.2.3+build"),
                ("empty", ""),
                ("contact@example", "a=b"),
            ],
            second_call: &[("empty", "now-set")],
            expected: &[
                ("cost center", "eng/platform"),
                ("app.version", "1.2.3+build"),
                ("empty", "now-set"),
                ("contact@example", "a=b"),
            ],
        },
    ]
}

/// The tags a call writes onto a resource of any stored kind are the tags every later read
/// of that resource reports — the tag listing and the resource's own read alike.
#[tokio::test]
async fn tags_written_onto_any_stored_resource_kind_are_what_every_later_read_reports() {
    for row in rows() {
        let router = emulator();
        let arn = row.kind.create(&router, row.slug, row.started_with).await;

        for (step, call) in [row.first_call, row.second_call].into_iter().enumerate() {
            if call.is_empty() {
                continue;
            }
            tag_resource(&router, &format!("{} (call {step})", row.case), &arn, call).await;
        }

        let expected = tag_map(row.expected);
        let listed = reported_tags(&router, row.case, &arn).await;
        assert_eq!(
            listed, expected,
            "case {:?}: expected the tag listing to report {expected:?}, got {listed:?}",
            row.case
        );

        // The tags live on the resource's own record, so the resource's own read reports
        // the same map.
        let from_own_read = row
            .kind
            .tags_from_own_read(&router, row.case, &arn)
            .await;
        assert_eq!(
            from_own_read, expected,
            "case {:?}: expected the resource's own read to report {expected:?}, got \
             {from_own_read:?}",
            row.case
        );
    }
}

/// A tagging call writes the record its ARN names and nothing else: no sibling resource's
/// tags, and not the pipeline member that carries the tags its runs apply to the images
/// they create.
#[tokio::test]
async fn a_tagging_call_touches_only_the_resource_its_arn_names() {
    let router = emulator();
    let pipeline = pipeline_in(
        &router,
        REGION,
        "isolation",
        &[("Owner", "platform")],
        &[("Build", "from-pipeline")],
    )
    .await;
    let first_run = start_pipeline(&router, REGION, &pipeline, "run-one", &[("Run", "a")]).await;
    let second_run = start_pipeline(&router, REGION, &pipeline, "run-two", &[("Run", "b")]).await;

    // Tag ONE of the two build versions.
    tag_resource(&router, "tag the first run", &first_run, &[("Checked", "yes")]).await;

    for (case, arn, expected) in [
        (
            "the tagged run",
            &first_run,
            tag_map(&[("Run", "a"), ("Checked", "yes")]),
        ),
        ("the untagged run", &second_run, tag_map(&[("Run", "b")])),
        (
            "the pipeline the runs belong to",
            &pipeline,
            tag_map(&[("Owner", "platform")]),
        ),
    ] {
        let actual = reported_tags(&router, case, arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // Tagging the PIPELINE writes the pipeline's own tags and leaves the tags its runs
    // apply to the images they create exactly as its create set them.
    tag_resource(&router, "tag the pipeline", &pipeline, &[("Stage", "prod")]).await;

    let read = read_pipeline(&router, "the tagged pipeline", &pipeline).await;
    let own_tags = tags_in("the tagged pipeline", &read, "tags");
    assert_eq!(
        own_tags,
        tag_map(&[("Owner", "platform"), ("Stage", "prod")]),
        "case \"the tagged pipeline\": expected the pipeline's own tags to carry the new key, \
         got {own_tags:?}"
    );
    let image_tags = tags_in("the tagged pipeline", &read, "imageTags");
    assert_eq!(
        image_tags,
        tag_map(&[("Build", "from-pipeline")]),
        "case \"the tagged pipeline\": expected the tags its runs apply to be untouched, got \
         {image_tags:?}"
    );

    // And the runs still carry what they carried.
    for (case, arn, expected) in [
        (
            "the tagged run after the pipeline was tagged",
            &first_run,
            tag_map(&[("Run", "a"), ("Checked", "yes")]),
        ),
        (
            "the untagged run after the pipeline was tagged",
            &second_run,
            tag_map(&[("Run", "b")]),
        ),
    ] {
        let actual = reported_tags(&router, case, arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// Writing a key that is already present with the value it already has leaves the resource
/// holding exactly the tags it held: the write is a merge, so repeating it changes nothing.
#[tokio::test]
async fn repeating_a_tagging_call_leaves_the_tags_it_already_wrote() {
    let router = emulator();
    let pipeline = pipeline_in(&router, REGION, "idempotent", &[], &[]).await;
    let run = start_pipeline(&router, REGION, &pipeline, "run-idempotent", &[]).await;

    for (case, arn) in [("the pipeline", &pipeline), ("the build version", &run)] {
        let written = [("Owner", "platform"), ("Stage", "dev")];
        tag_resource(&router, case, arn, &written).await;
        let once = reported_tags(&router, case, arn).await;
        assert_eq!(
            once,
            tag_map(&written),
            "case {case:?}: expected the tags the call wrote, got {once:?}"
        );

        for repeat in ["the first repeat", "the second repeat"] {
            tag_resource(&router, &format!("{case}, {repeat}"), arn, &written).await;
            let after = reported_tags(&router, case, arn).await;
            assert_eq!(
                after, once,
                "case {case:?} {repeat}: expected the tags to be unchanged at {once:?}, got \
                 {after:?}"
            );
        }
    }
}

/// An ARN the calling Region holds no resource under names nothing to tag, whatever kind of
/// resource it names — and the refusal leaves the resource its own Region holds untouched.
#[tokio::test]
async fn an_arn_this_region_holds_no_resource_under_is_reported_missing() {
    let router = emulator();
    let pipeline = pipeline_in(&router, REGION, "regional", &[], &[]).await;
    let run = start_pipeline(&router, REGION, &pipeline, "run-regional", &[]).await;
    tag_resource(&router, "tag the pipeline", &pipeline, &[("Owner", "platform")]).await;
    tag_resource(&router, "tag the run", &run, &[("Run", "only")]).await;

    for (kind, arn) in [(Kind::ImagePipeline, &pipeline), (Kind::Image, &run)] {
        // The same resource, reached from the OTHER Region's endpoint: a regional endpoint
        // serves only its own Region's resources.
        let case = format!("a {} of another Region", kind.resource_type());
        let (status, body) =
            tag_resource_in(&router, OTHER_REGION, arn, &[("Stage", "prod")]).await;
        assert_missing(&case, status, &body, kind.resource_type(), arn);

        // A never-created resource of the same kind, in the calling Region.
        let absent = kind.absent_arn();
        let case = format!("a never-created {}", kind.resource_type());
        let (status, body) = tag_resource_in(&router, REGION, &absent, &[("Stage", "prod")]).await;
        assert_missing(&case, status, &body, kind.resource_type(), &absent);
    }

    // Neither refusal wrote anything: each resource still carries the one tag it was given.
    for (case, arn, expected) in [
        ("the pipeline after the refusals", &pipeline, tag_map(&[("Owner", "platform")])),
        ("the build version after the refusals", &run, tag_map(&[("Run", "only")])),
    ] {
        let actual = reported_tags(&router, case, arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}
