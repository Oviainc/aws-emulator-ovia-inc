// SPDX-License-Identifier: CC0-1.0

//! The three tagging operations — `TagResource`, `ListTagsForResource` and
//! `UntagResource` — over the wire, on every kind of Batch resource this service holds.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts
//! (`POST`, `GET` and `DELETE` on `/v1/tags/{resourceArn}`, with the ARN percent-encoded
//! into the one path segment the `@httpLabel` binds), so what is asserted is the response
//! a `batch` client gets.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * `TagResource` MERGES its `tags` into the tag map the resource `resourceArn` names
//!   already holds — a key the request carries is replaced, every other key is left
//!   alone ("If existing tags on a resource aren't specified in the request parameters,
//!   they aren't changed") — and that merge holds for EVERY resource kind whose record
//!   carries a tag map (compute environment, job queue, job definition, scheduling
//!   policy, consumable resource, service environment, quota share, job, service job);
//! * `ListTagsForResource` reports exactly that stored map, and the same map the kind's
//!   own `Describe*` reader reports: there is one copy of a resource's tags, not two;
//! * `UntagResource` deletes only the keys `tagKeys` names, and a key the resource does
//!   not hold is removed vacuously, so repeating an untag changes nothing;
//! * an ARN this service holds no resource for — a name nothing was created under,
//!   another region or account, an unknown resource kind, or a bare name rather than an
//!   ARN — is refused with the modelled `ClientException` (HTTP 400) by all three
//!   operations, and stores nothing;
//! * the bounds `TagrisTagsMap` declares hold: a key of 1..128 characters and a value of
//!   0..256 are accepted at each bound and refused one past it, at most 50 tags travel in
//!   one request, and a merge that would take one resource past 50 tags is refused rather
//!   than stored;
//! * a list is a REPORTING call: two lists at the same instant answer identically and a
//!   list leaves the resource untagged if it was untagged.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state::{self, limits};

use crate::job_lifecycle::{
    arn, compute_environment, container_properties, error_code, error_message, job_definition,
    job_queue, Batch,
};

/// The longest `TagKey` the model admits. Source: `com.amazonaws.batch#TagKey`
/// (`smithy_query(query="shape", shape="TagrisTagsMap")`), `@length(min: 1, max: 128)`.
const MAX_TAG_KEY_LEN: usize = 128;

/// The longest `TagValue` the model admits, from the same shape:
/// `@length(min: 0, max: 256)` — so an EMPTY value is legal and a 257-character one is
/// not.
const MAX_TAG_VALUE_LEN: usize = 256;

/// `value` percent-encoded for ONE URI component: every byte outside the unreserved set
/// (RFC 3986 §2.3 `ALPHA / DIGIT / "-" / "." / "_" / "~"`) becomes `%XX` over its UTF-8
/// bytes, which is what an SDK client does to an `@httpLabel` or an `@httpQuery` value.
///
/// Written here rather than pulled from a crate because it exists only to BUILD a test
/// request — the emulator itself never encodes a URI — and a test-only dependency is not
/// worth an entry in the workspace lock file.
fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(*byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// `/v1/tags/{resourceArn}`: the ARN fills ONE path segment, so every character outside
/// the unreserved set is percent-encoded, the way an SDK client encodes an `@httpLabel`.
fn tags_uri(resource_arn: &str) -> String {
    format!("/v1/tags/{}", percent_encode(resource_arn))
}

/// `TagResource`: `POST /v1/tags/{resourceArn}` with the `tags` map in the body.
async fn tag(batch: &Batch, resource_arn: &str, tags: Value) -> (u16, Value) {
    batch
        .send("POST", &tags_uri(resource_arn), Some(json!({"tags": tags})))
        .await
}

/// `ListTagsForResource`: `GET /v1/tags/{resourceArn}`.
async fn list(batch: &Batch, resource_arn: &str) -> (u16, Value) {
    batch.send("GET", &tags_uri(resource_arn), None).await
}

/// `UntagResource`: `DELETE /v1/tags/{resourceArn}?tagKeys=…`, one `tagKeys` parameter
/// per key (the `@httpQuery` binding of a list member).
async fn untag(batch: &Batch, resource_arn: &str, keys: &[String]) -> (u16, Value) {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encode(key)))
        .collect::<Vec<String>>()
        .join("&");
    batch
        .send("DELETE", &format!("{}?{query}", tags_uri(resource_arn)), None)
        .await
}

/// The tags `ListTagsForResource` reports for `resource_arn`, as an ordered map so an
/// assertion reads them in one stable order.
async fn listed(batch: &Batch, case: &str, resource_arn: &str) -> BTreeMap<String, String> {
    let (status, body) = list(batch, resource_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {status} {body}"
    );
    let map = body["tags"].as_object().cloned().unwrap_or_else(|| {
        panic!("case {case:?}: ListTagsForResource must answer a tags map, got {body}")
    });
    map.into_iter()
        .map(|(key, value)| {
            (
                key,
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// A tag map as an ordered map, for comparing against what was listed.
fn expected_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One batch of tags a request carries, and what makes it a different KIND of input.
struct TagBatch {
    label: &'static str,
    pairs: Vec<(String, String)>,
}

/// The batches every resource kind is tagged with, in order. The rows differ along what
/// the rule generalises over: how many tags travel in one request, the character classes
/// of a key and of a value (ASCII, non-ASCII, emoji), the empty value the model's
/// `@length(min: 0)` admits, and the published maxima of the key and the value.
fn tag_batches() -> Vec<TagBatch> {
    vec![
        TagBatch {
            label: "one ASCII tag",
            pairs: vec![("team".to_string(), "batch".to_string())],
        },
        TagBatch {
            label: "a non-ASCII value, including an emoji",
            pairs: vec![("owner".to_string(), "ünïcøde-ом-値-🚀".to_string())],
        },
        TagBatch {
            label: "a non-ASCII key",
            pairs: vec![("キー".to_string(), "値".to_string())],
        },
        TagBatch {
            label: "an empty value, the lower bound TagValue admits",
            pairs: vec![("empty".to_string(), String::new())],
        },
        TagBatch {
            label: "a key and a value at their published maxima",
            pairs: vec![(
                "k".repeat(MAX_TAG_KEY_LEN),
                "v".repeat(MAX_TAG_VALUE_LEN),
            )],
        },
        TagBatch {
            label: "five tags in one request",
            pairs: (0..5)
                .map(|index| (format!("bulk-{index}"), format!("value-{index}")))
                .collect(),
        },
    ]
}

/// A tag batch as the `tags` member of a request.
fn tags_body(pairs: &[(String, String)]) -> Value {
    let mut body = serde_json::Map::new();
    for (key, value) in pairs {
        body.insert(key.clone(), json!(value));
    }
    Value::Object(body)
}

/// One resource of every kind this service holds a tag map for: the kind's label and the
/// ARN its create minted.
async fn taggable_resources(batch: &Batch) -> Vec<(&'static str, String)> {
    let environment = compute_environment(batch, "tag-env").await;
    let queue = job_queue(batch, "tag-queue", &environment).await;
    let (definition, _revision) = job_definition(batch, "tag-def", container_properties()).await;

    let policy = batch
        .ok("/v1/createschedulingpolicy", json!({"name": "tag-policy"}))
        .await["arn"]
        .as_str()
        .expect("CreateSchedulingPolicy answers an ARN")
        .to_string();
    let consumable = batch
        .ok(
            "/v1/createconsumableresource",
            json!({"consumableResourceName": "tag-consumable", "totalQuantity": 4}),
        )
        .await["consumableResourceArn"]
        .as_str()
        .expect("CreateConsumableResource answers an ARN")
        .to_string();
    let service_environment = batch
        .ok(
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": "tag-service-env",
                "serviceEnvironmentType": "SAGEMAKER_TRAINING",
                "capacityLimits": [{"maxCapacity": 2, "capacityUnit": "NUM_INSTANCES"}],
            }),
        )
        .await["serviceEnvironmentArn"]
        .as_str()
        .expect("CreateServiceEnvironment answers an ARN")
        .to_string();
    let quota_share = batch
        .ok(
            "/v1/createquotashare",
            json!({
                "quotaShareName": "tag-share",
                "jobQueue": "tag-queue",
                "capacityLimits": [{"maxCapacity": 4, "capacityUnit": "ml.m5.large"}],
                "resourceSharingConfiguration": {"strategy": "RESERVE"},
                "preemptionConfiguration": {"inSharePreemption": "DISABLED"},
            }),
        )
        .await["quotaShareArn"]
        .as_str()
        .expect("CreateQuotaShare answers an ARN")
        .to_string();
    let job = batch
        .ok(
            "/v1/submitjob",
            json!({
                "jobName": "tag-job",
                "jobQueue": "tag-queue",
                "jobDefinition": "tag-def",
            }),
        )
        .await["jobArn"]
        .as_str()
        .expect("SubmitJob answers a job ARN")
        .to_string();

    // A SERVICE job needs a `SAGEMAKER_TRAINING` queue over a service environment, which
    // is a different queue from the one above.
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "tag-service-queue",
                "priority": 1,
                "jobQueueType": "SAGEMAKER_TRAINING",
                "serviceEnvironmentOrder": [
                    {"order": 1, "serviceEnvironment": "tag-service-env"},
                ],
            }),
        )
        .await;
    let service_job = batch
        .ok(
            "/v1/submitservicejob",
            json!({
                "jobName": "tag-service-job",
                "jobQueue": "tag-service-queue",
                "serviceJobType": "SAGEMAKER_TRAINING",
                "serviceRequestPayload": "{}",
            }),
        )
        .await["jobArn"]
        .as_str()
        .expect("SubmitServiceJob answers a job ARN")
        .to_string();

    vec![
        ("compute environment", environment),
        ("job queue", queue),
        ("job definition", definition),
        ("scheduling policy", policy),
        ("consumable resource", consumable),
        ("service environment", service_environment),
        ("quota share", quota_share),
        ("job", job),
        ("service job", service_job),
    ]
}

/// The rule this issue was filed about, over every resource kind and every class of tag:
/// a tag request MERGES into the resource's stored map, a list reports that map, and an
/// untag removes only the keys it names.
#[tokio::test]
async fn tagging_merges_into_the_stored_map_of_every_taggable_resource_kind() {
    let batch = Batch::new();
    for (kind, resource_arn) in taggable_resources(&batch).await {
        // A resource nobody has tagged holds no tag, and the list says so rather than
        // refusing.
        let case = format!("{kind}: before any tag");
        assert!(
            listed(&batch, &case, &resource_arn).await.is_empty(),
            "case {case:?}: a resource nothing tagged must list no tag"
        );

        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for request in tag_batches() {
            let case = format!("{kind}: {}", request.label);
            let (status, answered) =
                tag(&batch, &resource_arn, tags_body(&request.pairs)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from TagResource, got {status} {answered}"
            );
            expected.extend(request.pairs.iter().cloned());
            let actual = listed(&batch, &case, &resource_arn).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the merge of every batch so far {expected:?}, got \
                 {actual:?}"
            );
        }

        // A key the resource already holds is REPLACED, not added: the value changes and
        // the tag count does not.
        let case = format!("{kind}: re-tagging a key the resource already holds");
        let before = expected.len();
        let (status, answered) = tag(&batch, &resource_arn, json!({"team": "platform"})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from TagResource, got {status} {answered}"
        );
        expected.insert("team".to_string(), "platform".to_string());
        let actual = listed(&batch, &case, &resource_arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the replaced value and nothing else changed, got {actual:?}"
        );
        assert_eq!(
            actual.len(),
            before,
            "case {case:?}: replacing a key must not add one, got {actual:?}"
        );

        // An untag removes ONLY the keys it names.
        let case = format!("{kind}: untagging one key");
        let (status, answered) = untag(&batch, &resource_arn, &["team".to_string()]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UntagResource, got {status} {answered}"
        );
        expected.remove("team");
        let actual = listed(&batch, &case, &resource_arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected only the named key gone, got {actual:?}"
        );

        // A key the resource does not hold is removed vacuously, so the SAME untag
        // repeated answers the same success and changes nothing.
        let case = format!("{kind}: untagging a key the resource does not hold");
        let (status, answered) = untag(&batch, &resource_arn, &["team".to_string()]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from a repeated UntagResource, got {status} {answered}"
        );
        let actual = listed(&batch, &case, &resource_arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: a vacuous untag must change nothing, got {actual:?}"
        );

        // Untagging every remaining key leaves the resource untagged — an empty map, not
        // a refusal.
        let case = format!("{kind}: untagging every remaining key");
        let remaining: Vec<String> = expected.keys().cloned().collect();
        let (status, answered) = untag(&batch, &resource_arn, &remaining).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UntagResource, got {status} {answered}"
        );
        let actual = listed(&batch, &case, &resource_arn).await;
        assert!(
            actual.is_empty(),
            "case {case:?}: expected no tag left, got {actual:?}"
        );
    }
}

/// A resource's tags are ONE map: what `TagResource` wrote is what the kind's own
/// `Describe*` reader reports, and what an `UntagResource` removes disappears from it
/// too.
#[tokio::test]
async fn the_tags_tagresource_writes_are_the_map_every_reader_reports() {
    let batch = Batch::new();
    let environment = compute_environment(&batch, "reader-env").await;
    let queue = job_queue(&batch, "reader-queue", &environment).await;
    let consumable = batch
        .ok(
            "/v1/createconsumableresource",
            json!({"consumableResourceName": "reader-consumable"}),
        )
        .await["consumableResourceArn"]
        .as_str()
        .expect("CreateConsumableResource answers an ARN")
        .to_string();

    // One row per kind that has its own reader: the ARN to tag, the describe route and
    // its body, and where the described record carries its tags.
    struct Reader {
        kind: &'static str,
        resource_arn: String,
        route: &'static str,
        request: Value,
        /// The path into the describe response at which that record's tags live.
        tags_at: &'static [&'static str],
    }

    /// The `tags` of the described record, read at the row's path.
    fn tags_of(body: &Value, path: &[&str]) -> Value {
        let mut at = body;
        for step in path {
            at = match step.parse::<usize>() {
                Ok(index) => &at[index],
                Err(_) => &at[*step],
            };
        }
        at.clone()
    }

    let readers: Vec<Reader> = vec![
        Reader {
            kind: "compute environment",
            resource_arn: environment,
            route: "/v1/describecomputeenvironments",
            request: json!({"computeEnvironments": ["reader-env"]}),
            tags_at: &["computeEnvironments", "0", "tags"],
        },
        Reader {
            kind: "job queue",
            resource_arn: queue,
            route: "/v1/describejobqueues",
            request: json!({"jobQueues": ["reader-queue"]}),
            tags_at: &["jobQueues", "0", "tags"],
        },
        Reader {
            kind: "consumable resource",
            resource_arn: consumable,
            route: "/v1/describeconsumableresource",
            request: json!({"consumableResource": "reader-consumable"}),
            tags_at: &["tags"],
        },
    ];

    for reader in readers {
        let case = reader.kind;
        let (status, answered) = tag(
            &batch,
            &reader.resource_arn,
            json!({"env": "test", "owner": "sre"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from TagResource, got {status} {answered}"
        );

        let described = batch.ok(reader.route, reader.request.clone()).await;
        assert_eq!(
            tags_of(&described, reader.tags_at),
            json!({"env": "test", "owner": "sre"}),
            "case {case:?}: the describe reader must report the tags TagResource wrote, got \
             {described}"
        );

        let (status, answered) =
            untag(&batch, &reader.resource_arn, &["owner".to_string()]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UntagResource, got {status} {answered}"
        );
        let described = batch.ok(reader.route, reader.request).await;
        assert_eq!(
            tags_of(&described, reader.tags_at),
            json!({"env": "test"}),
            "case {case:?}: the describe reader must lose the untagged key too, got {described}"
        );
    }
}

/// An ARN no resource answers to is refused by all three operations with the modelled
/// `ClientException` (HTTP 400) — over the SET of ways an ARN can name nothing — and the
/// refusal stores nothing.
#[tokio::test]
async fn an_arn_naming_no_resource_is_refused_by_every_tagging_operation() {
    let batch = Batch::new();
    let environment = compute_environment(&batch, "refusal-env").await;
    let queue = job_queue(&batch, "refusal-queue", &environment).await;

    let other_region = format!(
        "arn:aws:batch:eu-west-1:{}:job-queue/refusal-queue",
        engine::EMULATOR_ACCOUNT_ID
    );
    let references: Vec<(&str, String)> = vec![
        ("a name nothing was created under", arn("job-queue/absent")),
        (
            "a compute environment nothing was created under",
            arn("compute-environment/absent"),
        ),
        (
            "a job definition revision nothing registered",
            arn("job-definition/absent:1"),
        ),
        ("a job nothing submitted", arn("job/0123456789abcdef")),
        (
            "a quota share nothing created",
            arn("job-queue/refusal-queue/quota-share/absent"),
        ),
        ("another region", other_region),
        (
            "another account",
            "arn:aws:batch:us-east-1:999988887777:job-queue/refusal-queue".to_string(),
        ),
        (
            "a resource kind Batch does not tag",
            arn("fleet/refusal-queue"),
        ),
        ("a bare name rather than an ARN", "refusal-queue".to_string()),
        ("an ARN of another service", "arn:aws:sqs:us-east-1:000000000000:queue".to_string()),
    ];

    for (case, reference) in references {
        let (status, refused) = tag(&batch, &reference, json!({"team": "batch"})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from TagResource, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case {case:?}: expected the modelled ClientException, got {refused}"
        );
        assert!(
            error_message(&refused).contains(&reference),
            "case {case:?}: expected the refusal to name the reference it was given, got {:?}",
            error_message(&refused)
        );

        let (status, refused) = list(&batch, &reference).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from ListTagsForResource, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case {case:?}: expected the modelled ClientException, got {refused}"
        );

        let (status, refused) = untag(&batch, &reference, &["team".to_string()]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from UntagResource, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case {case:?}: expected the modelled ClientException, got {refused}"
        );
    }

    // None of those refusals wrote anything to the resource that DOES exist.
    let actual = listed(&batch, "the queue that exists", &queue).await;
    assert!(
        actual.is_empty(),
        "case refusals: a refused tag must leave the stored resource untagged, got {actual:?}"
    );
}

/// The bounds `TagrisTagsMap` declares, at each published bound and one past it: the key
/// and value lengths, how many tags one request carries, and how many tags one resource
/// ends up holding.
#[tokio::test]
async fn the_published_tag_bounds_are_honoured_at_the_bound_and_refused_past_it() {
    let batch = Batch::new();
    // The bounds the service enforces are the ones the model declares, read off
    // `TagrisTagsMap` above.
    assert_eq!(
        (state::MAX_TAG_KEY_LEN, state::MAX_TAG_VALUE_LEN),
        (MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN),
        "the key and value bounds the service enforces must be the ones the model declares"
    );
    let environment = compute_environment(&batch, "bounds-env").await;

    // At the bound: a 1-character key, a 128-character key, an empty value and a
    // 256-character value are all accepted.
    let at_bound: Vec<(&str, Value)> = vec![
        ("a one-character key", json!({"k": "one-char-key"})),
        (
            "a key at its published maximum",
            tags_body(&[("k".repeat(MAX_TAG_KEY_LEN), "at-max-key".to_string())]),
        ),
        ("an empty value", json!({"empty-value": ""})),
        (
            "a value at its published maximum",
            tags_body(&[("at-max-value".to_string(), "v".repeat(MAX_TAG_VALUE_LEN))]),
        ),
    ];
    for (case, tags) in at_bound {
        let (status, answered) = tag(&batch, &environment, tags).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a tag at the published bound is accepted, got {status} {answered}"
        );
    }

    // One past the bound: refused, and nothing of the refused request is stored.
    //
    // WHICH refusal depends on who sees the violation: the boundary walk reaches a tag
    // VALUE and the map's own length, and this service words that refusal as Tagris's
    // `BadRequestException` (`service.rs`); a tag KEY is inside the map where the walk
    // does not reach, so it is this service's own modelled `ClientException`.
    let past_bound: Vec<(&str, Value, &str)> = vec![
        (
            "a key one character past its maximum",
            tags_body(&[("k".repeat(MAX_TAG_KEY_LEN + 1), "too-long-key".to_string())]),
            "ClientException",
        ),
        (
            "a value one character past its maximum",
            tags_body(&[(
                "too-long-value".to_string(),
                "v".repeat(MAX_TAG_VALUE_LEN + 1),
            )]),
            "BadRequestException",
        ),
        (
            "an empty key",
            json!({"": "empty-key"}),
            "ClientException",
        ),
        ("no tag at all", json!({}), "BadRequestException"),
        (
            "one tag past the 50 a request carries",
            tags_body(
                &(0..=limits::MAX_TAGS_PER_RESOURCE)
                    .map(|index| (format!("per-request-{index}"), index.to_string()))
                    .collect::<Vec<(String, String)>>(),
            ),
            "BadRequestException",
        ),
    ];
    for (case, tags, expected_code) in past_bound {
        let refused_keys: Vec<String> = tags
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        let (status, refused) = tag(&batch, &environment, tags).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 past the published bound, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused}"
        );
        let stored = listed(&batch, case, &environment).await;
        for key in refused_keys {
            assert!(
                !stored.contains_key(&key),
                "case {case:?}: a refused tag request must store none of its keys, got \
                 {stored:?}"
            );
        }
    }

    // The per-RESOURCE ceiling: 50 tags fit, and the tag that would be the 51st is
    // refused with the modelled ClientException while the stored 50 stay exactly as they
    // were.
    let queue = job_queue(&batch, "ceiling-queue", &environment).await;
    let full: Vec<(String, String)> = (0..limits::MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let (status, answered) = tag(&batch, &queue, tags_body(&full)).await;
    assert_eq!(
        status, 200,
        "case the ceiling: {} tags fit on one resource, got {status} {answered}",
        limits::MAX_TAGS_PER_RESOURCE
    );
    let at_ceiling = listed(&batch, "the ceiling", &queue).await;
    assert_eq!(
        at_ceiling.len(),
        limits::MAX_TAGS_PER_RESOURCE,
        "case the ceiling: expected {} stored tags, got {at_ceiling:?}",
        limits::MAX_TAGS_PER_RESOURCE
    );

    let (status, refused) = tag(&batch, &queue, json!({"one-too-many": "x"})).await;
    assert_eq!(
        status, 400,
        "case one past the ceiling: expected HTTP 400, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ClientException",
        "case one past the ceiling: expected the modelled ClientException, got {refused}"
    );
    let after = listed(&batch, "one past the ceiling", &queue).await;
    assert_eq!(
        after, at_ceiling,
        "case one past the ceiling: a refused tag must leave the stored map untouched, got \
         {after:?}"
    );

    // At the ceiling a REPLACEMENT still works: it adds no key.
    let (status, answered) = tag(&batch, &queue, json!({"key-0": "replaced"})).await;
    assert_eq!(
        status, 200,
        "case replacing at the ceiling: expected 200, got {status} {answered}"
    );
    let replaced = listed(&batch, "replacing at the ceiling", &queue).await;
    assert_eq!(
        replaced.len(),
        limits::MAX_TAGS_PER_RESOURCE,
        "case replacing at the ceiling: the tag count must not grow, got {replaced:?}"
    );
    assert_eq!(
        replaced.get("key-0").map(String::as_str),
        Some("replaced"),
        "case replacing at the ceiling: expected the replaced value, got {replaced:?}"
    );
}

/// `ListTagsForResource` REPORTS: two lists at the same instant answer identically, and a
/// list of an untagged resource leaves it untagged.
#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let batch = Batch::new();
    let environment = compute_environment(&batch, "reporting-env").await;

    let first = listed(&batch, "an untagged resource", &environment).await;
    let second = listed(&batch, "an untagged resource, read again", &environment).await;
    assert_eq!(
        first, second,
        "case an untagged resource: two lists must answer identically, got {first:?} and \
         {second:?}"
    );
    assert!(
        first.is_empty(),
        "case an untagged resource: a list must not invent a tag, got {first:?}"
    );

    let (status, answered) = tag(&batch, &environment, json!({"team": "batch", "env": "dev"})).await;
    assert_eq!(
        status, 200,
        "case tag then read twice: expected 200 from TagResource, got {status} {answered}"
    );
    let first = listed(&batch, "tag then read twice", &environment).await;
    let second = listed(&batch, "tag then read twice, again", &environment).await;
    assert_eq!(
        first, second,
        "case tag then read twice: a list must not change what a later list sees, got \
         {first:?} and {second:?}"
    );
    assert_eq!(
        first,
        expected_map(&[("env", "dev"), ("team", "batch")]),
        "case tag then read twice: expected the tags the request carried, got {first:?}"
    );
}
