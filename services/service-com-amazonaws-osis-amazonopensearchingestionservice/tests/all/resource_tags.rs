// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the pipeline tag operations — `TagResource`, `UntagResource` and
//! `ListTagsForResource` — driven the way a client drives them: one HTTP request per call
//! through the same `MultiServiceRouter` the emulator boots.
//!
//! The rule under test is that a pipeline's tags are a KEY-UNIQUE set stored on the
//! pipeline record: a tag write adds a key it does not carry and replaces the value of one
//! it does, an untag removes exactly the keys it names (and nothing else, whether or not
//! the pipeline carried them), and every read of that pipeline — `ListTagsForResource`,
//! `GetPipeline`, `ListPipelines` — reports the same set.
//!
//! The rows of each table differ along what the rule generalises over: how many tags one
//! call carries, whether a key is new or already present, the key and value lengths
//! (including both published bounds), whether a key is repeated inside one call, how many
//! calls build the set up, and which subset an untag names.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A region that is NOT the emulator default, so nothing under test can depend on a
/// default-region code path.
const REGION: &str = "eu-west-2";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// Comfortably longer than the service's own transition span, so a pipeline is settled
/// before the tag calls run.
const PAST_TRANSITION: Duration =
    service_com_amazonaws_osis_amazonopensearchingestionservice::state::TRANSITION_SPAN
        .saturating_add(Duration::from_millis(400));

/// The tags one pipeline may carry, from the service's own ceiling, so this suite cannot
/// drift from the constant the handlers enforce.
const MAX_TAGS: usize =
    service_com_amazonaws_osis_amazonopensearchingestionservice::state::MAX_TAGS_PER_PIPELINE;

/// One row of the tag-write table: the case name, the pipeline, the tags its create
/// carried, the tag calls to make in order, and the set every read must then report.
type TagWriteCase = (
    &'static str,
    &'static str,
    Option<Value>,
    Vec<Value>,
    BTreeMap<String, String>,
);

/// One row of the untag table: the case name, the pipeline, the tags its create carried,
/// the keys to remove, and the set the removal leaves.
type UntagCase = (
    &'static str,
    &'static str,
    Value,
    Value,
    BTreeMap<String, String>,
);

/// What one call answered: the status, the modelled error code (empty on success) and the
/// parsed body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The error message the response carries, under either casing a protocol may render
    /// the `message` member with.
    fn message(&self) -> String {
        for key in ["message", "Message"] {
            if let Some(text) = self.body.get(key).and_then(Value::as_str) {
                return text.to_string();
            }
        }
        panic!("expected an error message in {:?}", self.body)
    }

    /// The `Tags` list of this answer, read from wherever the answering operation carries
    /// it: the top level for `ListTagsForResource`, the `Pipeline` for the pipeline reads.
    fn tag_list(&self, case: &str) -> Vec<Value> {
        for value in [&self.body["Tags"], &self.body["Pipeline"]["Tags"]] {
            if let Some(list) = value.as_array() {
                return list.clone();
            }
        }
        panic!("case {case:?}: expected a `Tags` list in {:?}", self.body)
    }

    /// The tag set of this answer as a key -> value map, which is what the rule is about:
    /// the ORDER a service reports a tag set in is not part of its contract.
    fn tags(&self, case: &str) -> BTreeMap<String, String> {
        let list = self.tag_list(case);
        let mut tags = BTreeMap::new();
        for tag in &list {
            let key = tag["Key"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag with no Key in {list:?}"))
                .to_string();
            let value = tag["Value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag with no Value in {list:?}"))
                .to_string();
            assert!(
                tags.insert(key.clone(), value).is_none(),
                "case {case:?}: tag keys must be unique for the pipeline, got {key:?} twice \
                 in {list:?}"
            );
        }
        tags
    }
}

fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_osis_amazonopensearchingestionservice::EmulatedService::new(engine),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://osis.{REGION}.amazonaws.com{path}"))
        // The credential scope is how a handler learns the region its ARNs name.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/osis/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// The `arn` query value. A pipeline ARN is made of partition, service, Region, account
/// and name segments joined by `:` and `/`, and RFC 3986 allows both characters in a
/// query component unescaped, so it needs no encoding to go on the wire as itself.
fn query_arn(arn: &str) -> String {
    arn.to_string()
}

/// The ARN of a pipeline in the signed region, as `CreatePipeline` mints it.
fn arn_of(pipeline_name: &str) -> String {
    format!("arn:aws:osis:{REGION}:{ACCOUNT}:pipeline/{pipeline_name}")
}

async fn create_pipeline(router: &MultiServiceRouter, request: Value) -> Answer {
    call(
        router,
        "POST",
        "/2022-01-01/osis/createPipeline",
        Some(request),
    )
    .await
}

async fn get_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/2022-01-01/osis/getPipeline/{name}"),
        None,
    )
    .await
}

async fn list_pipelines(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", "/2022-01-01/osis/listPipelines", None).await
}

/// `TagResource` is `POST /2022-01-01/osis/tagResource`: `Arn` is an `@httpQuery` member,
/// so the body carries only `Tags`.
async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/2022-01-01/osis/tagResource?arn={}", query_arn(arn)),
        Some(json!({ "Tags": tags })),
    )
    .await
}

/// `UntagResource` is `POST /2022-01-01/osis/untagResource`, with `Arn` on the query.
async fn untag_resource(router: &MultiServiceRouter, arn: &str, keys: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/2022-01-01/osis/untagResource?arn={}", query_arn(arn)),
        Some(json!({ "TagKeys": keys })),
    )
    .await
}

/// `ListTagsForResource` is `GET /2022-01-01/osis/listTagsForResource`, with `Arn` on the
/// query.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/2022-01-01/osis/listTagsForResource?arn={}",
            query_arn(arn)
        ),
        None,
    )
    .await
}

/// Create a pipeline (with the tags the case starts from) and let the `CREATING` ->
/// `ACTIVE` transition the clock owes come due.
async fn settled_pipeline(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    tags: Option<Value>,
) -> String {
    let mut request = json!({
        "PipelineName": name,
        "MinUnits": 1,
        "MaxUnits": 2,
        "PipelineConfigurationBody": "version: \"2\"\ntag-pipeline:\n  source:\n    http:\n",
    });
    if let Some(tags) = tags {
        request["Tags"] = tags;
    }
    let created = create_pipeline(router, request).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreatePipeline, got {} / {:?}",
        created.status, created.body
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    arn_of(name)
}

/// One tag as the wire carries it.
fn tag(key: &str, value: &str) -> Value {
    json!({ "Key": key, "Value": value })
}

/// The expected set, as a map, from a list of pairs.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Every read of a pipeline reports the same tag set: the tag listing, the pipeline read
/// and the pipeline summary cannot disagree, because all three answer from one record.
async fn assert_every_read_agrees(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    arn: &str,
    want: &BTreeMap<String, String>,
) {
    let listed = list_tags(router, arn).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {} / {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        &listed.tags(case),
        want,
        "case {case:?}: ListTagsForResource must report the stored set, got {:?}",
        listed.tags(case)
    );

    let read = get_pipeline(router, name).await;
    assert_eq!(
        &read.tags(case),
        want,
        "case {case:?}: GetPipeline must report the same tags, got {:?}",
        read.tags(case)
    );

    let summaries = list_pipelines(router).await;
    let summary = summaries.body["Pipelines"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Pipelines in {:?}", summaries.body))
        .iter()
        .find(|pipeline| pipeline["PipelineName"] == json!(name))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: {name:?} missing from {:?}", summaries.body));
    let summary_tags: BTreeMap<String, String> = summary["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Tags in the summary {summary:?}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        &summary_tags, want,
        "case {case:?}: the ListPipelines summary must report the same tags, got \
         {summary_tags:?}"
    );
}

/// A tag write is an upsert keyed by tag key, and what it leaves is what every read
/// reports.
#[tokio::test]
async fn tag_writes_add_or_replace_by_key_and_read_back() {
    // `TagKey` is `@length(1, 128)` and `TagValue` is `@length(0, 256)` (smithy_query:
    // shapes TagKey / TagValue), so the rows below sit on every published bound: a
    // one-character key, a 128-character key, an empty value and a 256-character value.
    let long_key = "k".repeat(128);
    let long_value = "v".repeat(256);

    // (case, pipeline name, the tags the create carried, the tag calls to make in order,
    //  the set every read must then report)
    let cases: Vec<TagWriteCase> = vec![
        (
            "first tag on an untagged pipeline",
            "tags-first-write",
            None,
            vec![json!([tag("team", "search")])],
            expected(&[("team", "search")]),
        ),
        (
            "several tags in one call",
            "tags-many-in-one-call",
            None,
            vec![json!([
                tag("team", "search"),
                tag("env", "prod"),
                tag("owner", "ingest"),
            ])],
            expected(&[("team", "search"), ("env", "prod"), ("owner", "ingest")]),
        ),
        (
            "a second call adds a new key and keeps the first",
            "tags-added-over-calls",
            None,
            vec![json!([tag("team", "search")]), json!([tag("env", "prod")])],
            expected(&[("team", "search"), ("env", "prod")]),
        ),
        (
            "a key already on the pipeline takes the new value",
            "tags-replace-value",
            Some(json!([tag("team", "search"), tag("env", "dev")])),
            vec![json!([tag("env", "prod")])],
            expected(&[("team", "search"), ("env", "prod")]),
        ),
        (
            "one call that both replaces and adds",
            "tags-replace-and-add",
            Some(json!([tag("team", "search")])),
            vec![json!([tag("team", "analytics"), tag("env", "prod")])],
            expected(&[("team", "analytics"), ("env", "prod")]),
        ),
        (
            // Keys are unique per pipeline, so a call that names one key twice leaves the
            // last value it gave rather than storing the key twice.
            "a key repeated inside one call",
            "tags-repeated-key",
            None,
            vec![json!([tag("env", "dev"), tag("env", "prod")])],
            expected(&[("env", "prod")]),
        ),
        (
            "the shortest legal key and the empty value",
            "tags-shortest-key",
            None,
            vec![json!([tag("k", "")])],
            expected(&[("k", "")]),
        ),
        (
            "the longest legal key and value",
            "tags-longest-key-and-value",
            None,
            vec![json!([tag(&long_key, &long_value)])],
            expected(&[(long_key.as_str(), long_value.as_str())]),
        ),
        (
            // An empty list names no key, so it leaves the set exactly as it was.
            "a tag call that carries no tag",
            "tags-empty-call",
            Some(json!([tag("team", "search")])),
            vec![json!([])],
            expected(&[("team", "search")]),
        ),
    ];

    let router = new_router();
    for (case, name, created_with, writes, want) in cases {
        let arn = settled_pipeline(&router, case, name, created_with).await;
        for tags in writes {
            let written = tag_resource(&router, &arn, tags.clone()).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected 200 from TagResource for {tags:?}, got {} / {:?}",
                written.status, written.body
            );
            // `TagResourceResponse` models no members: the answer is an empty document.
            assert!(
                written.body.is_null() || written.body == json!({}),
                "case {case:?}: TagResource answers no members, got {:?}",
                written.body
            );
        }
        assert_every_read_agrees(&router, case, name, &arn, &want).await;
    }
}

/// An untag removes exactly the keys it names, leaves every other key alone, and is
/// idempotent in a key the pipeline does not carry.
#[tokio::test]
async fn untagging_removes_only_the_keys_it_names() {
    // (case, pipeline name, the tags created, the keys to remove, the set left)
    let cases: Vec<UntagCase> = vec![
        (
            "one key out of several",
            "untag-one-of-three",
            json!([tag("team", "search"), tag("env", "prod"), tag("owner", "x")]),
            json!(["env"]),
            expected(&[("team", "search"), ("owner", "x")]),
        ),
        (
            "several keys at once",
            "untag-several",
            json!([tag("team", "search"), tag("env", "prod"), tag("owner", "x")]),
            json!(["team", "owner"]),
            expected(&[("env", "prod")]),
        ),
        (
            "every key the pipeline carries",
            "untag-everything",
            json!([tag("team", "search"), tag("env", "prod")]),
            json!(["env", "team"]),
            expected(&[]),
        ),
        (
            // Untagging is idempotent: a key nothing ever set leaves the set untouched.
            "a key the pipeline never carried",
            "untag-unknown-key",
            json!([tag("team", "search")]),
            json!(["cost-center"]),
            expected(&[("team", "search")]),
        ),
        (
            "a mix of a carried and an uncarried key",
            "untag-mixed-keys",
            json!([tag("team", "search"), tag("env", "prod")]),
            json!(["env", "never-set"]),
            expected(&[("team", "search")]),
        ),
        (
            "no key at all",
            "untag-no-keys",
            json!([tag("team", "search")]),
            json!([]),
            expected(&[("team", "search")]),
        ),
        (
            // The published key bounds again, this time on the removal side.
            "the longest legal key",
            "untag-longest-key",
            json!([tag(&"k".repeat(128), "value"), tag("k", "short")]),
            json!([&"k".repeat(128)]),
            expected(&[("k", "short")]),
        ),
    ];

    let router = new_router();
    for (case, name, created_with, keys, want) in cases {
        let arn = settled_pipeline(&router, case, name, Some(created_with)).await;
        let removed = untag_resource(&router, &arn, keys.clone()).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected 200 from UntagResource for {keys:?}, got {} / {:?}",
            removed.status, removed.body
        );
        assert_every_read_agrees(&router, case, name, &arn, &want).await;

        // Idempotent: the same removal again leaves the same set.
        let again = untag_resource(&router, &arn, keys.clone()).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: a repeated UntagResource must also answer 200, got {} / {:?}",
            again.status, again.body
        );
        assert_every_read_agrees(&router, case, name, &arn, &want).await;
    }
}

/// Every tag operation resolves the pipeline by its whole ARN, so an ARN no pipeline
/// carries is a 404 — and a tagged pipeline is not reachable through another account's,
/// another Region's or another name's ARN.
#[tokio::test]
async fn tag_operations_refuse_an_arn_no_pipeline_carries() {
    let router = new_router();
    let name = "tagged-pipeline";
    let arn = settled_pipeline(&router, "the one real pipeline", name, None).await;
    let tagged = tag_resource(&router, &arn, json!([tag("team", "search")])).await;
    assert_eq!(
        tagged.status, 200,
        "expected 200 from TagResource on the created pipeline, got {} / {:?}",
        tagged.status, tagged.body
    );

    // Each row is a well-formed pipeline ARN (`@length(46, 76)` and the `PipelineArn`
    // pattern, smithy_query: shape PipelineArn) that names no stored pipeline: a name
    // nothing was created under, and the created pipeline's own name in another Region,
    // another account and another partition.
    let unknown = vec![
        ("a name nothing was created under", arn_of("ghost-pipeline")),
        (
            "the same name in another Region",
            format!("arn:aws:osis:us-east-1:{ACCOUNT}:pipeline/{name}"),
        ),
        (
            "the same name in another account",
            format!("arn:aws:osis:{REGION}:210987654321:pipeline/{name}"),
        ),
        (
            "the same name in another partition",
            format!("arn:aws-us-gov:osis:{REGION}:{ACCOUNT}:pipeline/{name}"),
        ),
    ];

    for (case, missing) in unknown {
        assert!(
            (46..=76).contains(&missing.len()),
            "case {case:?}: the probe ARN must satisfy @length(46, 76), got {} chars in \
             {missing:?}",
            missing.len()
        );
        for (operation, refused) in [
            ("TagResource", tag_resource(&router, &missing, json!([tag("k", "v")])).await),
            ("UntagResource", untag_resource(&router, &missing, json!(["k"])).await),
            ("ListTagsForResource", list_tags(&router, &missing).await),
        ] {
            assert_eq!(
                refused.status, 404,
                "case {case:?}/{operation}: expected HTTP 404, got {} / {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type, "ResourceNotFoundException",
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {:?}",
                refused.error_type
            );
            assert_eq!(
                refused.message(),
                "The pipeline could not be found",
                "case {case:?}/{operation}: unexpected wording, got {:?}",
                refused.message()
            );
        }
    }

    // None of those refusals touched the pipeline that does exist.
    assert_every_read_agrees(
        &router,
        "the real pipeline is untouched",
        name,
        &arn,
        &expected(&[("team", "search")]),
    )
    .await;
}

/// A pipeline carries at most the allowed number of tags: a write that would take it past
/// the ceiling is refused and stores nothing, while a write that only replaces values is
/// not growth and is allowed at the ceiling.
#[tokio::test]
async fn a_tag_write_past_the_ceiling_is_refused_and_stores_nothing() {
    let router = new_router();
    let case = "fill the pipeline to the ceiling";
    let name = "tags-at-the-ceiling";
    let arn = settled_pipeline(&router, case, name, None).await;

    let full: Vec<Value> = (0..MAX_TAGS)
        .map(|index| tag(&format!("key-{index:03}"), &format!("value-{index:03}")))
        .collect();
    let filled = tag_resource(&router, &arn, json!(full)).await;
    assert_eq!(
        filled.status, 200,
        "case {case:?}: a write up to the ceiling of {MAX_TAGS} tags must be accepted, got \
         {} / {:?}",
        filled.status, filled.body
    );
    let at_ceiling = list_tags(&router, &arn).await.tags(case);
    assert_eq!(
        at_ceiling.len(),
        MAX_TAGS,
        "case {case:?}: expected {MAX_TAGS} stored tags, got {}",
        at_ceiling.len()
    );

    // One more KEY is growth past the ceiling, so it is refused — and leaves the set as
    // it was.
    let case = "one key past the ceiling";
    let refused = tag_resource(&router, &arn, json!([tag("one-too-many", "v")])).await;
    assert_eq!(
        refused.status, 409,
        "case {case:?}: expected HTTP 409, got {} / {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type, "LimitExceededException",
        "case {case:?}: expected LimitExceededException, got {:?}",
        refused.error_type
    );
    assert_eq!(
        refused.message(),
        "You attempted to create more than the allowed number of tags",
        "case {case:?}: unexpected wording, got {:?}",
        refused.message()
    );
    assert_eq!(
        list_tags(&router, &arn).await.tags(case),
        at_ceiling,
        "case {case:?}: a refused tag write must store nothing"
    );

    // Replacing the value of a key the pipeline already carries is not growth, so the
    // ceiling does not refuse it.
    let case = "replace a value at the ceiling";
    let replaced = tag_resource(&router, &arn, json!([tag("key-000", "rewritten")])).await;
    assert_eq!(
        replaced.status, 200,
        "case {case:?}: replacing a value at the ceiling must be accepted, got {} / {:?}",
        replaced.status, replaced.body
    );
    let after = list_tags(&router, &arn).await.tags(case);
    assert_eq!(
        after.len(),
        MAX_TAGS,
        "case {case:?}: the set must still hold {MAX_TAGS} tags, got {}",
        after.len()
    );
    assert_eq!(
        after.get("key-000").map(String::as_str),
        Some("rewritten"),
        "case {case:?}: the replaced value must be stored, got {:?}",
        after.get("key-000")
    );

    // Freeing a key makes room again: the same write that was refused now lands.
    let case = "room freed by an untag";
    let removed = untag_resource(&router, &arn, json!(["key-001"])).await;
    assert_eq!(
        removed.status, 200,
        "case {case:?}: expected 200 from UntagResource, got {} / {:?}",
        removed.status, removed.body
    );
    let accepted = tag_resource(&router, &arn, json!([tag("one-too-many", "v")])).await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: the freed slot must accept a new key, got {} / {:?}",
        accepted.status, accepted.body
    );
    let after = list_tags(&router, &arn).await.tags(case);
    assert_eq!(
        after.len(),
        MAX_TAGS,
        "case {case:?}: expected {MAX_TAGS} stored tags, got {}",
        after.len()
    );
    assert!(
        after.contains_key("one-too-many") && !after.contains_key("key-001"),
        "case {case:?}: expected the new key and not the removed one, got {after:?}"
    );

    // The same ceiling holds at create time, and a create it refuses makes no pipeline.
    let case = "a create carrying one tag past the ceiling";
    let too_many: Vec<Value> = (0..=MAX_TAGS)
        .map(|index| tag(&format!("key-{index:03}"), "v"))
        .collect();
    let refused = create_pipeline(
        &router,
        json!({
            "PipelineName": "tags-create-too-many",
            "MinUnits": 1,
            "MaxUnits": 1,
            "PipelineConfigurationBody": "version: \"2\"\ntag-pipeline:\n  source:\n    http:\n",
            "Tags": too_many.clone(),
        }),
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "case {case:?}: expected HTTP 409, got {} / {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type, "LimitExceededException",
        "case {case:?}: expected LimitExceededException, got {:?}",
        refused.error_type
    );
    let read = get_pipeline(&router, "tags-create-too-many").await;
    assert_eq!(
        read.status, 404,
        "case {case:?}: a refused create must store no pipeline, got {} / {:?}",
        read.status, read.body
    );

    // One call that names more tags than a pipeline may carry is refused whatever the keys
    // are, and leaves the pipeline untagged.
    let case = "one call naming more tags than the ceiling";
    let fresh = settled_pipeline(&router, case, "tags-one-huge-call", None).await;
    let refused = tag_resource(&router, &fresh, json!(too_many)).await;
    assert_eq!(
        refused.status, 409,
        "case {case:?}: expected HTTP 409, got {} / {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type, "LimitExceededException",
        "case {case:?}: expected LimitExceededException, got {:?}",
        refused.error_type
    );
    assert!(
        list_tags(&router, &fresh).await.tags(case).is_empty(),
        "case {case:?}: a refused tag write must store nothing, got {:?}",
        list_tags(&router, &fresh).await.tags(case)
    );
}
