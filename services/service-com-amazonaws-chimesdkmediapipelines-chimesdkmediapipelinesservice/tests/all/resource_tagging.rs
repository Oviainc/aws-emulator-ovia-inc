// SPDX-License-Identifier: CC0-1.0

//! The generic tagging surface of this service, end to end over HTTP.
//!
//! The RULE pinned here: a resource's tags are ARN-keyed state the three generic tagging
//! routes share. `TagResource` MERGES the pairs it carries into what the resource already
//! has (a repeated key is re-valued, an untouched one kept), `UntagResource` removes
//! exactly the keys it names and nothing else, and `ListTagsForResource` reports back the
//! tags those two and the CREATE left — an empty list for a resource carrying none. A
//! tagging call keyed by a resource of this service that does not exist is refused with
//! the modelled 404 and writes nothing, as is one that would take a resource past the
//! number of tags this model's `TagList` bounds a resource by.
//!
//! The tables span what the rule generalises over: how many pairs one call carries (one,
//! several, the modelled maximum of 200, one past it), the form of a key and a value
//! (short, an EMPTY value, non-ASCII, at the modelled length bounds), which resource is
//! tagged (a media capture PIPELINE and a media insights pipeline CONFIGURATION, each
//! tagged at create time and afterwards), how many keys an untag names (one, several, one
//! the resource never carried), and the order the calls arrive in.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::state::MAX_TAGS_PER_RESOURCE;
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the suite signs for, as the sibling suites in this binary do.
const REGION: &str = "us-west-2";

/// A `ResourceAccessRoleArn` satisfying the modelled `Arn` shape, for the configurations
/// a row tags.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/media-insights-access";

/// One emulator: a shared engine with this service mounted on it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path_and_query: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map_or_else(Vec::new, |b| {
            serde_json::to_vec(&b).expect("a JSON value serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://media-pipelines-chime.{REGION}.amazonaws.com{path_and_query}"
            ))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTSUITE/20261021/{REGION}/chime/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, body)
    }

    /// Create a media capture pipeline and return its `(MediaPipelineId, ARN)`.
    async fn capture_pipeline(&self, suffix: &str, tags: &[(String, String)]) -> (String, String) {
        let mut request = json!({
            "SourceType": "ChimeSdkMeeting",
            "SourceArn": format!("arn:aws:chime:{REGION}:000000000000:meeting/m-{suffix}"),
            "SinkType": "S3Bucket",
            "SinkArn": format!("arn:aws:s3:::bucket-{suffix}/captures"),
        });
        if !tags.is_empty() {
            request["Tags"] = wire_tags(tags);
        }
        let (status, body) = self
            .call("POST", "/sdk-media-capture-pipelines", Some(request))
            .await;
        assert_eq!(
            status, 201,
            "CreateMediaCapturePipeline({suffix}) must answer 201, got {status} {body:?}"
        );
        let pipeline = &body["MediaCapturePipeline"];
        (
            string_at(pipeline, "MediaPipelineId"),
            string_at(pipeline, "MediaPipelineArn"),
        )
    }

    /// Create a media insights pipeline configuration and return its `(name, ARN)`.
    async fn insights_configuration(
        &self,
        name: &str,
        tags: &[(String, String)],
    ) -> (String, String) {
        let mut request = json!({
            "MediaInsightsPipelineConfigurationName": name,
            "ResourceAccessRoleArn": ROLE_ARN,
            "Elements": [{
                "Type": "AmazonTranscribeProcessor",
                "AmazonTranscribeProcessorConfiguration": { "LanguageCode": "en-US" },
            }],
        });
        if !tags.is_empty() {
            request["Tags"] = wire_tags(tags);
        }
        let (status, body) = self
            .call("POST", "/media-insights-pipeline-configurations", Some(request))
            .await;
        assert_eq!(
            status, 201,
            "CreateMediaInsightsPipelineConfiguration({name}) must answer 201, got \
             {status} {body:?}"
        );
        let configuration = &body["MediaInsightsPipelineConfiguration"];
        (
            string_at(configuration, "MediaInsightsPipelineConfigurationName"),
            string_at(configuration, "MediaInsightsPipelineConfigurationArn"),
        )
    }

    async fn tag(&self, arn: &str, pairs: &[(String, String)]) -> (u16, Value) {
        self.call(
            "POST",
            "/tags?operation=tag-resource",
            Some(json!({ "ResourceARN": arn, "Tags": wire_tags(pairs) })),
        )
        .await
    }

    async fn untag(&self, arn: &str, tag_keys: &[String]) -> (u16, Value) {
        self.call(
            "POST",
            "/tags?operation=untag-resource",
            Some(json!({ "ResourceARN": arn, "TagKeys": tag_keys })),
        )
        .await
    }

    async fn list_raw(&self, arn: &str) -> (u16, Value) {
        self.call("GET", &format!("/tags?arn={}", encode(arn)), None)
            .await
    }

    async fn list(&self, arn: &str) -> (u16, BTreeMap<String, String>) {
        let (status, body) = self.list_raw(arn).await;
        (status, tag_map(&body))
    }
}

fn string_at(value: &Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("no {member} in {value:?}"))
        .to_string()
}

/// Percent-encode a value for a query value, the way an AWS SDK encodes one.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn wire_tags(pairs: &[(String, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

fn tag_map(body: &Value) -> BTreeMap<String, String> {
    body["Tags"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|tag| {
                    Some((
                        tag["Key"].as_str()?.to_string(),
                        tag["Value"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn as_map(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs.iter().cloned().collect()
}

/// One row of the round-trip table: the pairs each successive tagging call applies, the
/// keys untagged afterwards, and the tags the resource is left with.
struct TagCase {
    case: &'static str,
    calls: Vec<Vec<(String, String)>>,
    removed: Vec<String>,
    left: Vec<(String, String)>,
}

/// RULE: tagging merges, untagging removes only the keys it names, and the listing
/// reports exactly the tags the writes left — for both resource kinds this service mints.
#[tokio::test]
async fn tags_round_trip_through_the_shared_tagging_routes() {
    let emulator = Emulator::new();

    // The pairs below stay inside the model's own bounds: `TagKey` is len 1..128 and
    // `TagValue` len 0..256 (`smithy_query(query="shape",shape="Tag")`), so an EMPTY
    // value is a value a row may carry.
    let long_key = "k".repeat(128);
    let long_value = "v".repeat(256);
    let at_bounds = owned(&[
        (long_key.as_str(), long_value.as_str()),
        ("équipe", "ノート"),
        ("empty", ""),
    ]);

    let rows: Vec<TagCase> = vec![
        TagCase {
            case: "one pair, kept",
            calls: vec![owned(&[("env", "dev")])],
            removed: Vec::new(),
            left: owned(&[("env", "dev")]),
        },
        TagCase {
            case: "several pairs in one call, one of them removed again",
            calls: vec![owned(&[("env", "dev"), ("team", "core"), ("tier", "gold")])],
            removed: keys(&["team"]),
            left: owned(&[("env", "dev"), ("tier", "gold")]),
        },
        TagCase {
            case: "two calls merging, the repeated key re-valued",
            calls: vec![
                owned(&[("env", "dev"), ("team", "core")]),
                owned(&[("env", "prod"), ("tier", "gold")]),
            ],
            removed: Vec::new(),
            left: owned(&[("env", "prod"), ("team", "core"), ("tier", "gold")]),
        },
        TagCase {
            case: "a key the resource never carried is removed as a no-op",
            calls: vec![owned(&[("env", "dev")])],
            removed: keys(&["absent", "neither"]),
            left: owned(&[("env", "dev")]),
        },
        TagCase {
            case: "every key removed, leaving no tags at all",
            calls: vec![owned(&[("env", "dev"), ("team", "core")])],
            removed: keys(&["env", "team"]),
            left: Vec::new(),
        },
        TagCase {
            case: "keys and values at the modelled length bounds, an empty value, non-ASCII text",
            calls: vec![at_bounds.clone()],
            removed: Vec::new(),
            left: at_bounds,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let TagCase {
            case,
            calls,
            removed,
            left,
        } = row;
        let (_, pipeline) = emulator.capture_pipeline(&format!("rt{index}"), &[]).await;
        let (_, configuration) = emulator
            .insights_configuration(&format!("conf-rt-{index}"), &[])
            .await;

        for arn in [pipeline.as_str(), configuration.as_str()] {
            let (status, tags) = emulator.list(arn).await;
            assert_eq!(
                (status, tags.len()),
                (200, 0),
                "case {case:?} ({arn}): an untagged resource reports no tags, got {tags:?}"
            );

            for pairs in &calls {
                let (status, body) = emulator.tag(arn, pairs).await;
                assert_eq!(
                    status, 204,
                    "case {case:?} ({arn}): expected TagResource to answer 204, got \
                     {status} {body:?}"
                );
            }
            if !removed.is_empty() {
                let (status, body) = emulator.untag(arn, &removed).await;
                assert_eq!(
                    status, 204,
                    "case {case:?} ({arn}): expected UntagResource to answer 204, got \
                     {status} {body:?}"
                );
            }

            let (status, tags) = emulator.list(arn).await;
            assert_eq!(
                (status, &tags),
                (200, &as_map(&left)),
                "case {case:?} ({arn}): expected exactly the tags the writes left"
            );

            // A read does not change what the next reader sees.
            let (status, again) = emulator.list(arn).await;
            assert_eq!(
                (status, &again),
                (200, &tags),
                "case {case:?} ({arn}): a second listing differed from the first"
            );
        }
    }
}

/// One row of the create-time table: the tags the create carries, what the later calls do
/// to them, and the tags the resource is left with.
struct CreateTagCase {
    case: &'static str,
    created_with: Vec<(String, String)>,
    then_tagged: Vec<(String, String)>,
    untagged: Vec<String>,
    left: Vec<(String, String)>,
}

/// RULE: tags given at CREATE time are the same state the tagging routes read and write,
/// so a created resource lists them, the tagging routes change them, and deleting the
/// resource takes them with it.
#[tokio::test]
async fn tags_supplied_at_create_time_are_listed_and_can_be_changed() {
    let emulator = Emulator::new();

    let rows: Vec<CreateTagCase> = vec![
        CreateTagCase {
            case: "one create-time tag, left alone",
            created_with: owned(&[("env", "dev")]),
            then_tagged: Vec::new(),
            untagged: Vec::new(),
            left: owned(&[("env", "dev")]),
        },
        CreateTagCase {
            case: "three create-time tags, one re-valued and one dropped",
            created_with: owned(&[("env", "dev"), ("team", "core"), ("tier", "gold")]),
            then_tagged: owned(&[("env", "prod")]),
            untagged: keys(&["tier"]),
            left: owned(&[("env", "prod"), ("team", "core")]),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let CreateTagCase {
            case,
            created_with,
            then_tagged,
            untagged,
            left,
        } = row;
        let (pipeline_id, pipeline) = emulator
            .capture_pipeline(&format!("ct{index}"), &created_with)
            .await;
        let (configuration_name, configuration) = emulator
            .insights_configuration(&format!("conf-ct-{index}"), &created_with)
            .await;

        for arn in [pipeline.as_str(), configuration.as_str()] {
            let (status, listed) = emulator.list(arn).await;
            assert_eq!(
                (status, &listed),
                (200, &as_map(&created_with)),
                "case {case:?} ({arn}): expected the create-time tags to be listed"
            );

            if !then_tagged.is_empty() {
                emulator.tag(arn, &then_tagged).await;
            }
            if !untagged.is_empty() {
                emulator.untag(arn, &untagged).await;
            }

            let (status, listed) = emulator.list(arn).await;
            assert_eq!(
                (status, &listed),
                (200, &as_map(&left)),
                "case {case:?} ({arn}): expected the create-time tags as the later calls left them"
            );
        }

        // A resource's tags go with the resource: once it is deleted, its ARN is not a
        // resource of this service any more and the listing refuses it.
        let deletes = [
            (
                "the capture pipeline",
                format!("/sdk-media-capture-pipelines/{pipeline_id}"),
                pipeline.clone(),
            ),
            (
                "the insights configuration",
                format!("/media-insights-pipeline-configurations/{configuration_name}"),
                configuration.clone(),
            ),
        ];
        for (kind, path, arn) in deletes {
            let (status, body) = emulator.call("DELETE", &path, None).await;
            assert_eq!(
                status, 204,
                "case {case:?} ({kind}): expected the delete to answer 204, got {status} {body:?}"
            );
            let (status, body) = emulator.list_raw(&arn).await;
            assert_eq!(
                (status, body["Code"].as_str()),
                (404, Some("NotFound")),
                "case {case:?} ({kind}): expected a deleted resource's tags to be refused, got \
                 {body:?}"
            );
        }
    }
}

/// RULE: a tagging call that names a resource of this service that does not exist is
/// refused with the modelled 404 and writes nothing, and a call past the number of tags
/// this model bounds a resource by is refused, leaving the resource's tags as they were.
#[tokio::test]
async fn a_tagging_call_this_service_cannot_serve_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let (_, pipeline) = emulator.capture_pipeline("refuse", &[]).await;

    // Rows: each resource kind of THIS service whose ARN names nothing, plus an ARN
    // outside this family's ARN namespace altogether.
    let absent: Vec<(&str, String)> = vec![
        (
            "a media pipeline that never existed",
            format!("arn:aws:chime:{REGION}:000000000000:media-pipeline/8f14e45f-ceea-467a-9ba1-3d1e2b3c4d5e"),
        ),
        (
            "an insights configuration that never existed",
            format!(
                "arn:aws:chime:{REGION}:000000000000:media-insights-pipeline-configuration/absent"
            ),
        ),
        (
            "another vendor's resource ARN",
            "arn:aws:sqs:us-east-1:000000000000:queue/q1".to_string(),
        ),
    ];

    for (case, absent_arn) in absent {
        let refusals = [
            (
                "TagResource",
                emulator.tag(&absent_arn, &owned(&[("k", "v")])).await,
            ),
            (
                "UntagResource",
                emulator.untag(&absent_arn, &keys(&["k"])).await,
            ),
            ("ListTagsForResource", emulator.list_raw(&absent_arn).await),
        ];
        for (operation, (status, payload)) in refusals {
            assert_eq!(
                (status, payload["Code"].as_str()),
                (404, Some("NotFound")),
                "case {case:?} ({operation}): expected 404 NotFound for {absent_arn}, got \
                 {status} {payload:?}"
            );
        }
    }

    // The tag count is bounded by this model's own `TagList` maximum, which is 200 and
    // not the 50 its sibling models declare: a call that would take the resource past it
    // is refused and writes nothing.
    let at_maximum: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("k{index:03}"), "v".to_string()))
        .collect();
    let (status, body) = emulator.tag(&pipeline, &at_maximum).await;
    assert_eq!(
        status, 204,
        "case the modelled maximum of {MAX_TAGS_PER_RESOURCE} tags: expected it accepted, got \
         {status} {body:?}"
    );

    let (status, body) = emulator
        .tag(&pipeline, &owned(&[("one-too-many", "v")]))
        .await;
    assert_eq!(
        (status, body["Code"].as_str()),
        (400, Some("BadRequest")),
        "case one tag past the maximum: expected the modelled 400 refusal, got {status} {body:?}"
    );
    let (status, listed) = emulator.list(&pipeline).await;
    assert_eq!(
        (status, listed.len()),
        (200, at_maximum.len()),
        "case one tag past the maximum: expected the refused call to write nothing"
    );

    // Re-valuing a key the resource already carries adds none, so it is accepted even at
    // the maximum.
    let (status, body) = emulator
        .tag(&pipeline, &owned(&[("k000", "re-valued")]))
        .await;
    assert_eq!(
        status, 204,
        "case re-valuing a key at the maximum: expected it accepted, got {status} {body:?}"
    );
    let (status, listed) = emulator.list(&pipeline).await;
    assert_eq!(
        (status, listed.get("k000").map(String::as_str)),
        (200, Some("re-valued")),
        "case re-valuing a key at the maximum: expected the new value, got {listed:?}"
    );
}
