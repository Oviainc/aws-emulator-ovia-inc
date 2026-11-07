// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tag-key namespace this service keeps for its own tags, driven
//! the way a client drives it: one HTTP request per call through the same
//! `MultiServiceRouter` the emulator boots.
//!
//! RULE: a tag key whose start is the reserved namespace
//! (`state::RESERVED_TAG_KEY_PREFIX`, matched however it is cased) names a SYSTEM tag that
//! is not a caller's to assign or to remove, so every request naming one — a tag write, a
//! tag removal, or a create that carries tags — is refused `ValidationException` (HTTP
//! 400) with the service's wording around the offending key, and nothing of that request
//! is applied; a key outside the namespace is accepted and read back.
//!
//! The rows of each table differ along what the rule generalises over: what follows the
//! prefix, how the prefix is cased, how long the key is (both published `TagKey` bounds),
//! where in the key the prefix sits, and where in a request's list the reserved entry
//! sits.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_osis_amazonopensearchingestionservice as osis;

/// A region that is NOT the emulator default, so nothing under test can depend on a
/// default-region code path.
const REGION: &str = "eu-west-2";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// Comfortably longer than the service's own transition span, so a pipeline is settled
/// before the tag calls run.
const PAST_TRANSITION: Duration =
    osis::state::TRANSITION_SPAN.saturating_add(Duration::from_millis(400));

/// The longest key `TagKey`'s `@length(1, 128)` admits (smithy_query: shape TagKey).
const MAX_TAG_KEY_CHARS: usize = 128;

/// One row of the tag-key table: the case, the key, and whether the rule reserves it.
#[derive(Debug)]
struct KeyCase {
    case: &'static str,
    key: String,
    reserved: bool,
}

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
    fn message(&self, case: &str) -> String {
        for key in ["message", "Message"] {
            if let Some(text) = self.body.get(key).and_then(Value::as_str) {
                return text.to_string();
            }
        }
        panic!("case {case:?}: expected an error message in {:?}", self.body)
    }

    /// The tag set of this answer as a key -> value map: the ORDER a service reports a tag
    /// set in is not part of its contract.
    fn tags(&self, case: &str) -> BTreeMap<String, String> {
        let list = [&self.body["Tags"], &self.body["Pipeline"]["Tags"]]
            .into_iter()
            .find_map(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a `Tags` list in {:?}", self.body)
            })
            .clone();
        list.iter()
            .map(|entry| {
                (
                    entry["Key"].as_str().unwrap_or_default().to_string(),
                    entry["Value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }
}

fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(osis::EmulatedService::new(engine));
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

/// The ARN of a pipeline in the signed region, as `CreatePipeline` mints it. A pipeline ARN
/// joins its segments with `:` and `/`, both of which RFC 3986 allows in a query component
/// unescaped, so it goes on the wire as itself.
fn arn_of(pipeline_name: &str) -> String {
    format!("arn:aws:osis:{REGION}:{ACCOUNT}:pipeline/{pipeline_name}")
}

/// The body of a create that asks for a pipeline named `name`, carrying `tags` when given.
fn create_body(name: &str, tags: Option<Value>) -> Value {
    let mut request = json!({
        "PipelineName": name,
        "MinUnits": 1,
        "MaxUnits": 2,
        "PipelineConfigurationBody": "version: \"2\"\ntag-pipeline:\n  source:\n    http:\n",
    });
    if let Some(tags) = tags {
        request["Tags"] = tags;
    }
    request
}

async fn create_pipeline(router: &MultiServiceRouter, name: &str, tags: Option<Value>) -> Answer {
    call(
        router,
        "POST",
        "/2022-01-01/osis/createPipeline",
        Some(create_body(name, tags)),
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

/// `TagResource` is `POST /2022-01-01/osis/tagResource`: `Arn` is an `@httpQuery` member,
/// so the body carries only `Tags`.
async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/2022-01-01/osis/tagResource?arn={arn}"),
        Some(json!({ "Tags": tags })),
    )
    .await
}

/// `UntagResource` is `POST /2022-01-01/osis/untagResource`, with `Arn` on the query.
async fn untag_resource(router: &MultiServiceRouter, arn: &str, keys: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/2022-01-01/osis/untagResource?arn={arn}"),
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
        &format!("/2022-01-01/osis/listTagsForResource?arn={arn}"),
        None,
    )
    .await
}

/// One tag as the wire carries it.
fn tag(key: &str, value: &str) -> Value {
    json!({ "Key": key, "Value": value })
}

/// Create a pipeline carrying `tags` and let the `CREATING` -> `ACTIVE` transition the
/// clock owes come due.
async fn settled_pipeline(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    tags: Option<Value>,
) -> String {
    let created = create_pipeline(router, name, tags).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreatePipeline, got {} / {:?}",
        created.status, created.body
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    arn_of(name)
}

/// The message the service refuses a reserved key with: fixed text around the offending
/// key, which is the request's own.
fn reserved_message(key: &str) -> String {
    osis::state::reserved_tag_key_message(key)
}

/// Assert `answer` is the refusal the rule prescribes for the reserved key `key`.
fn assert_reserved_refusal(case: &str, key: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a reserved tag key must be refused 400, got {} / {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type, "ValidationException",
        "case {case:?}: expected ValidationException, got {:?} / {:?}",
        answer.error_type, answer.body
    );
    assert_eq!(
        answer.message(case),
        reserved_message(key),
        "case {case:?}: expected the message {:?}, got {:?}",
        reserved_message(key),
        answer.message(case)
    );
}

/// The rows of the namespace rule: keys inside the reserved namespace and keys outside it.
///
/// The reserved rows vary what follows the prefix (a single segment, a vendor-and-name
/// pair, nothing at all), how the prefix is cased, and the key's length up to `TagKey`'s
/// published 128-character ceiling. The allowed rows vary how nearly a key misses the
/// namespace: the bare word with no colon, a longer word that only starts with the same
/// letters, the prefix in the middle, and the prefix at the very end.
fn key_cases() -> Vec<KeyCase> {
    let prefix = osis::state::RESERVED_TAG_KEY_PREFIX;
    let mut cases = vec![
        KeyCase {
            case: "the reserved prefix with one segment after it",
            key: format!("{prefix}reserved"),
            reserved: true,
        },
        KeyCase {
            case: "a system tag another service writes",
            key: format!("{prefix}cloudformation:stack-name"),
            reserved: true,
        },
        KeyCase {
            case: "the reserved prefix with nothing after it",
            key: prefix.to_string(),
            reserved: true,
        },
        KeyCase {
            case: "the reserved prefix in upper case",
            key: format!("{}RESERVED", prefix.to_ascii_uppercase()),
            reserved: true,
        },
        KeyCase {
            case: "the reserved prefix in mixed case",
            key: "Aws:Reserved".to_string(),
            reserved: true,
        },
        KeyCase {
            case: "a reserved key as long as TagKey allows",
            key: format!("{prefix}{}", "z".repeat(MAX_TAG_KEY_CHARS - prefix.len())),
            reserved: true,
        },
        KeyCase {
            case: "an ordinary customer key",
            key: "sim2real-normal".to_string(),
            reserved: false,
        },
        KeyCase {
            case: "the namespace word without its colon",
            key: prefix.trim_end_matches(':').to_string(),
            reserved: false,
        },
        KeyCase {
            case: "a longer word that merely starts the same",
            key: "awsome:thing".to_string(),
            reserved: false,
        },
        KeyCase {
            case: "the prefix in the middle of the key",
            key: format!("cost:{prefix}centre"),
            reserved: false,
        },
        KeyCase {
            case: "the prefix at the very end of the key",
            key: format!("my-{prefix}"),
            reserved: false,
        },
        KeyCase {
            case: "the shortest key TagKey allows",
            key: "k".to_string(),
            reserved: false,
        },
    ];
    // The longest allowed key, so both published `TagKey` bounds are covered on the
    // accepted side as well as the refused one.
    cases.push(KeyCase {
        case: "an allowed key as long as TagKey allows",
        key: "q".repeat(MAX_TAG_KEY_CHARS),
        reserved: false,
    });
    cases
}

/// A tag WRITE naming a reserved key is refused and writes nothing; a key outside the
/// namespace is accepted and read back.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_key_and_writes_nothing() {
    let router = new_router();
    // One pipeline that already carries a tag, so a refusal can be shown to leave the
    // stored set exactly as it was.
    let arn = settled_pipeline(
        &router,
        "tag write",
        "reserved-tag-writes",
        Some(json!([tag("owner", "ingest")])),
    )
    .await;

    let mut want: BTreeMap<String, String> = BTreeMap::new();
    want.insert("owner".to_string(), "ingest".to_string());

    for row in key_cases() {
        let case = row.case;
        let answered = tag_resource(&router, &arn, json!([tag(&row.key, "nope")])).await;
        if row.reserved {
            assert_reserved_refusal(case, &row.key, &answered);
        } else {
            assert_eq!(
                answered.status, 200,
                "case {case:?}: a key outside the reserved namespace must be accepted, got \
                 {} / {:?}",
                answered.status, answered.body
            );
            want.insert(row.key.clone(), "nope".to_string());
        }

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.tags(case),
            want,
            "case {case:?}: ListTagsForResource must report {want:?}, got {:?}",
            listed.tags(case)
        );
    }

    // The refusals left the pipeline readable and its tag set whole, and a normal key
    // written after them is still visible.
    let read = get_pipeline(&router, "reserved-tag-writes").await;
    assert_eq!(
        read.tags("tag write"),
        want,
        "case {:?}: GetPipeline must report the same set, got {:?}",
        "tag write",
        read.tags("tag write")
    );
}

/// A tag REMOVAL naming a reserved key is refused and removes nothing; keys outside the
/// namespace are removed as asked.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_key_and_removes_nothing() {
    let router = new_router();
    let arn = settled_pipeline(
        &router,
        "tag removal",
        "reserved-tag-removals",
        Some(json!([tag("owner", "ingest"), tag("env", "prod")])),
    )
    .await;

    let mut want: BTreeMap<String, String> = BTreeMap::new();
    want.insert("owner".to_string(), "ingest".to_string());
    want.insert("env".to_string(), "prod".to_string());

    for row in key_cases() {
        let case = row.case;
        let answered = untag_resource(&router, &arn, json!([row.key.clone()])).await;
        if row.reserved {
            assert_reserved_refusal(case, &row.key, &answered);
        } else {
            assert_eq!(
                answered.status, 200,
                "case {case:?}: removing a key outside the reserved namespace must be \
                 accepted, got {} / {:?}",
                answered.status, answered.body
            );
            // Removing a key the pipeline does not carry changes nothing, which is what
            // every allowed row here does.
            want.remove(row.key.as_str());
        }

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.tags(case),
            want,
            "case {case:?}: ListTagsForResource must report {want:?}, got {:?}",
            listed.tags(case)
        );
    }
}

/// A CREATE whose tags name a reserved key is refused and mints no pipeline; one whose
/// tags are all the caller's own is created with them.
#[tokio::test]
async fn create_pipeline_refuses_reserved_tags_and_mints_nothing() {
    let router = new_router();

    for (index, row) in key_cases().into_iter().enumerate() {
        let case = row.case;
        // `PipelineName` is `@length(3, 28)` with pattern `^[a-z][a-z0-9\-]+$`
        // (smithy_query: operation CreatePipeline), so the name is derived from the row's
        // position rather than from its key.
        let name = format!("reserved-create-{index:02}");
        let created = create_pipeline(
            &router,
            &name,
            Some(json!([tag("owner", "ingest"), tag(&row.key, "nope")])),
        )
        .await;

        if row.reserved {
            assert_reserved_refusal(case, &row.key, &created);
            let read = get_pipeline(&router, &name).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: a refused create must mint no pipeline, got {} / {:?}",
                read.status, read.body
            );
        } else {
            assert_eq!(
                created.status, 200,
                "case {case:?}: a create whose tags are all allowed must succeed, got {} / {:?}",
                created.status, created.body
            );
            let want: BTreeMap<String, String> = [
                ("owner".to_string(), "ingest".to_string()),
                (row.key.clone(), "nope".to_string()),
            ]
            .into_iter()
            .collect();
            let listed = list_tags(&router, &arn_of(&name)).await;
            assert_eq!(
                listed.tags(case),
                want,
                "case {case:?}: ListTagsForResource must report {want:?}, got {:?}",
                listed.tags(case)
            );
        }
    }
}

/// A request that carries a reserved key BESIDE allowed ones is refused for the reserved
/// key wherever it sits in the list, and applies no part of itself: the allowed entries of
/// the same call are not written either.
#[tokio::test]
async fn one_reserved_key_refuses_the_whole_request() {
    let router = new_router();
    let arn = settled_pipeline(&router, "mixed list", "reserved-tag-mixed", None).await;
    let reserved = format!("{}reserved", osis::state::RESERVED_TAG_KEY_PREFIX);

    // (case, the Tags list, the key the refusal must name)
    let cases: Vec<(&str, Value, String)> = vec![
        (
            "the reserved key first",
            json!([tag(&reserved, "nope"), tag("good-one", "ok")]),
            reserved.clone(),
        ),
        (
            "the reserved key last",
            json!([tag("good-one", "ok"), tag(&reserved, "nope")]),
            reserved.clone(),
        ),
        (
            "the reserved key between allowed ones",
            json!([
                tag("good-one", "ok"),
                tag(&reserved, "nope"),
                tag("good-two", "ok"),
            ]),
            reserved.clone(),
        ),
        (
            // Several reserved keys in one call: the refusal names the FIRST, so it does
            // not depend on how the rest of the list is ordered.
            "two reserved keys",
            json!([
                tag("aws:first", "nope"),
                tag("aws:second", "nope"),
                tag("good-one", "ok"),
            ]),
            "aws:first".to_string(),
        ),
    ];

    for (case, tags, named) in cases {
        let answered = tag_resource(&router, &arn, tags).await;
        assert_reserved_refusal(case, &named, &answered);

        let listed = list_tags(&router, &arn).await;
        assert!(
            listed.tags(case).is_empty(),
            "case {case:?}: a refused call must apply no part of itself, got {:?}",
            listed.tags(case)
        );
    }

    // The same keys, without the reserved one, are accepted on the same pipeline — the
    // refusals above are about the key, not about the call shape.
    let accepted = tag_resource(
        &router,
        &arn,
        json!([tag("good-one", "ok"), tag("good-two", "ok")]),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: expected 200, got {} / {:?}",
        "allowed keys only", accepted.status, accepted.body
    );
    let listed = list_tags(&router, &arn).await;
    let want: BTreeMap<String, String> = [
        ("good-one".to_string(), "ok".to_string()),
        ("good-two".to_string(), "ok".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        listed.tags("allowed keys only"),
        want,
        "case {:?}: ListTagsForResource must report {want:?}, got {:?}",
        "allowed keys only",
        listed.tags("allowed keys only")
    );
}

/// The namespace test is about the KEY: a reserved-looking VALUE is the caller's own data
/// and is stored as given.
#[tokio::test]
async fn a_reserved_looking_value_is_not_refused() {
    let router = new_router();
    let arn = settled_pipeline(&router, "value", "reserved-tag-values", None).await;
    let value = format!("{}kms", osis::state::RESERVED_TAG_KEY_PREFIX);

    let answered = tag_resource(&router, &arn, json!([tag("origin", &value)])).await;
    assert_eq!(
        answered.status, 200,
        "case {:?}: a reserved-looking value must be accepted, got {} / {:?}",
        "value", answered.status, answered.body
    );
    let listed = list_tags(&router, &arn).await;
    let want: BTreeMap<String, String> = [("origin".to_string(), value)].into_iter().collect();
    assert_eq!(
        listed.tags("value"),
        want,
        "case {:?}: ListTagsForResource must report {want:?}, got {:?}",
        "value",
        listed.tags("value")
    );
}
