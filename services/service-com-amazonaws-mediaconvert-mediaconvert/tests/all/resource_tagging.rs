// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the tagging surface: `TagResource`,
//! `ListTagsForResource` and `UntagResource`.
//!
//! The rule under test: a tagging call resolves its `Arn` to the resource that ARN
//! names and works on THAT resource's stored tag set — `TagResource` merges the pairs it
//! was sent (a new key is added, a key already present takes the new value, every other
//! key is left alone), `UntagResource` removes the keys it names, and
//! `ListTagsForResource` reports the set that is stored. An ARN naming no stored
//! resource is the modelled `NotFoundException` (404) and a value that is not a
//! MediaConvert resource ARN is the modelled `BadRequestException` (400).
//!
//! Every case drives the real restJson1 wire path — the router picks the service from
//! the request's SigV4 scope, the framework deserializes and validates, the handler
//! runs, and the response is serialized — so what is asserted is the bytes a client
//! would see, never an internal call.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every case signs for unless it names another.
const REGION: &str = "us-west-2";

/// An IAM role ARN of the shape `CreateJob`'s `Role` member documents.
const ROLE: &str = "arn:aws:iam::123456789012:role/MediaConvertRole";

/// Longer than any case takes to run, so a job stays `SUBMITTED` for the whole of a
/// case. Pinned rather than raced: the span is state, so no case depends on how fast
/// the machine is.
const HELD_IN_SUBMITTED: Duration = Duration::from_secs(3_600);

/// A SigV4 `Authorization` header in this service's scope, signed for `region`. It is
/// what the router reads to pick the service, and what the handlers read the ARN region
/// from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDTESTTESTTESTTEST/20260101/{region}/mediaconvert/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
    )
}

/// `arn:aws:mediaconvert:<region>:<account>:<resource>`, built by the emulator's own ARN
/// helper so the expectation is not a second hand-written format.
fn arn(resource: &str) -> String {
    arn_in(REGION, resource)
}

/// As [`arn`], for a region other than the suite's default.
fn arn_in(region: &str, resource: &str) -> String {
    engine::emulator_arn("mediaconvert", region, resource)
}

/// Percent-encode a value for one path segment, as an AWS SDK encodes a non-greedy
/// `@httpLabel`: everything but the RFC 3986 unreserved set, so the `:` and `/` inside
/// an ARN arrive as one segment.
fn encode_label(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One emulator: a single shared engine behind the multi-service router, exactly as the
/// production router assembles it.
struct Emulator {
    router: MultiServiceRouter,
}

/// One HTTP reply, decoded far enough to assert on.
struct Reply {
    status: u16,
    /// `x-amzn-errortype`, the header restJson1 resolves an error shape from.
    error_type: Option<String>,
    /// The JSON body, or `Value::Null` for an empty one.
    body: Value,
}

impl Reply {
    /// A member of the body, or `Value::Null` when it is absent.
    fn at(&self, pointer: &str) -> &Value {
        self.body.pointer(pointer).unwrap_or(&Value::Null)
    }

    /// A string member of the body.
    fn string(&self, pointer: &str) -> String {
        self.at(pointer).as_str().unwrap_or_default().to_string()
    }

    /// The reported tag set, as a sorted map so a comparison does not depend on the
    /// order a JSON object happens to carry.
    fn reported_tags(&self) -> BTreeMap<String, String> {
        self.at("/resourceTags/tags")
            .as_object()
            .map(|map| {
                map.iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            value.as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // Jobs stay `SUBMITTED` for the whole suite: no case here is about the clock.
        engine
            .state::<service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState>()
            .lock()
            .job_settles_after = HELD_IN_SUBMITTED;
        let mut router = MultiServiceRouter::new();
        // The signature itself is the transport's business, not this suite's; the
        // Authorization header is still present because routing and the ARN region read
        // the credential scope out of it.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        self.send_in(REGION, method, uri, body).await
    }

    async fn send_in(&self, region: &str, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let request = ::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("mediaconvert.{region}.amazonaws.com"))
            .header("authorization", authorization(region))
            .header("content-type", "application/json")
            .body(::bytes::Bytes::from(payload))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.body().clone();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    async fn tag(&self, resource_arn: &str, tags: Value) -> Reply {
        self.send(
            "POST",
            "/2017-08-29/tags",
            Some(json!({ "arn": resource_arn, "tags": tags })),
        )
        .await
    }

    async fn list_tags(&self, resource_arn: &str) -> Reply {
        self.list_tags_in(REGION, resource_arn).await
    }

    async fn list_tags_in(&self, region: &str, resource_arn: &str) -> Reply {
        self.send_in(
            region,
            "GET",
            &format!("/2017-08-29/tags/{}", encode_label(resource_arn)),
            None,
        )
        .await
    }

    /// `UntagResource`. `keys` is `None` for a request that names no `TagKeys` at all.
    async fn untag(&self, resource_arn: &str, keys: Option<&[&str]>) -> Reply {
        let body = match keys {
            Some(keys) => json!({ "tagKeys": keys }),
            None => json!({}),
        };
        self.send(
            "PUT",
            &format!("/2017-08-29/tags/{}", encode_label(resource_arn)),
            Some(body),
        )
        .await
    }

    /// A queue created with `tags`, and its ARN.
    async fn queue(&self, name: &str, tags: Value) -> String {
        let reply = self
            .send(
                "POST",
                "/2017-08-29/queues",
                Some(json!({ "name": name, "tags": tags })),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "fixture queue {name:?}: expected HTTP 201, got {} with body {}",
            reply.status, reply.body
        );
        arn(&format!("queues/{name}"))
    }

    /// The ARN of a job template this store holds: the built-in SYSTEM templates, read
    /// back through `ListJobTemplates` (`listBy=SYSTEM`, the value that lists them)
    /// rather than hard-coded, so the case uses a name the service itself reports.
    /// `CreateJobTemplate` is still a scaffold, so these are the only templates that
    /// exist.
    async fn first_job_template(&self) -> String {
        let listed = self
            .send("GET", "/2017-08-29/jobTemplates?listBy=SYSTEM", None)
            .await;
        assert_eq!(
            listed.status, 200,
            "fixture job template: expected HTTP 200 from ListJobTemplates, got {} with body {}",
            listed.status, listed.body
        );
        let name = listed.string("/jobTemplates/0/name");
        assert!(
            !name.is_empty(),
            "fixture job template: expected at least one built-in template, got {}",
            listed.body
        );
        arn(&format!("jobTemplates/{name}"))
    }

    /// A preset created with `tags`, and its ARN. The settings are the minimal shape
    /// `CreatePreset` requires — a container — per `smithy_query(query="shape",
    /// shape="PresetSettings")`.
    async fn preset(&self, name: &str, tags: Value) -> String {
        let reply = self
            .send(
                "POST",
                "/2017-08-29/presets",
                Some(json!({
                    "name": name,
                    "settings": { "containerSettings": { "container": "MP4" } },
                    "tags": tags
                })),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "fixture preset {name:?}: expected HTTP 201, got {} with body {}",
            reply.status, reply.body
        );
        arn(&format!("presets/{name}"))
    }

    /// A job created with `tags`, and its ARN.
    async fn job(&self, tags: Value) -> String {
        let reply = self
            .send(
                "POST",
                "/2017-08-29/jobs",
                Some(json!({
                    "role": ROLE,
                    "settings": {
                        "inputs": [{ "fileInput": "s3://example-bucket/in/a.mp4" }],
                        "outputGroups": [{
                            "name": "File Group",
                            "outputs": [{
                                "nameModifier": "_a",
                                "containerSettings": { "container": "MP4" }
                            }],
                            "outputGroupSettings": {
                                "type": "FILE_GROUP_SETTINGS",
                                "fileGroupSettings": {
                                    "destination": "s3://example-bucket/out/"
                                }
                            }
                        }]
                    },
                    "tags": tags
                })),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "fixture job: expected HTTP 201, got {} with body {}",
            reply.status, reply.body
        );
        arn(&format!("jobs/{}", reply.string("/job/id")))
    }
}

/// A sorted map from the pairs a case names, so expectations read as tables.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `TagResource` merges into the stored set and `ListTagsForResource` reports exactly
/// what was persisted — over a table whose rows differ in the resource KIND the ARN
/// names and in the shape of the tags sent: one pair, several pairs at once, a key with
/// an empty value ("You can tag resources with a key-value pair or with only a key"), a
/// key already present (which takes the new value), and a non-ASCII value.
#[tokio::test]
async fn tagging_merges_into_the_stored_set_and_reads_back() {
    /// One step: the tags to send, then the whole set expected afterwards.
    struct Step {
        case: &'static str,
        send: Value,
        after: Vec<(&'static str, &'static str)>,
    }

    let steps = vec![
        Step {
            case: "first pair",
            send: json!({ "team": "video" }),
            after: vec![("team", "video")],
        },
        Step {
            case: "two further pairs at once",
            send: json!({ "env": "prod", "owner": "ops" }),
            after: vec![("env", "prod"), ("owner", "ops"), ("team", "video")],
        },
        Step {
            case: "a key with only a key (empty value)",
            send: json!({ "archived": "" }),
            after: vec![
                ("archived", ""),
                ("env", "prod"),
                ("owner", "ops"),
                ("team", "video"),
            ],
        },
        Step {
            case: "a key already present takes the new value",
            send: json!({ "env": "staging" }),
            after: vec![
                ("archived", ""),
                ("env", "staging"),
                ("owner", "ops"),
                ("team", "video"),
            ],
        },
        Step {
            case: "a non-ASCII value",
            send: json!({ "owner": "équipe-vidéo" }),
            after: vec![
                ("archived", ""),
                ("env", "staging"),
                ("owner", "équipe-vidéo"),
                ("team", "video"),
            ],
        },
        Step {
            case: "an empty set of tags changes nothing",
            send: json!({}),
            after: vec![
                ("archived", ""),
                ("env", "staging"),
                ("owner", "équipe-vidéo"),
                ("team", "video"),
            ],
        },
    ];

    for kind in ["queue", "job", "jobTemplate", "preset"] {
        let emulator = Emulator::new();
        let resource_arn = match kind {
            "queue" => emulator.queue("tagged", json!({})).await,
            "job" => emulator.job(json!({})).await,
            "jobTemplate" => emulator.first_job_template().await,
            _ => emulator.preset("tagged", json!({})).await,
        };

        // A resource nobody has tagged reports an empty set, not a refusal.
        let initial = emulator.list_tags(&resource_arn).await;
        assert_eq!(
            initial.status, 200,
            "case {kind} untagged: expected HTTP 200, got {} with body {}",
            initial.status, initial.body
        );
        assert_eq!(
            initial.reported_tags(),
            BTreeMap::new(),
            "case {kind} untagged: expected no tags, got {}",
            initial.body
        );

        for step in &steps {
            let tagged = emulator.tag(&resource_arn, step.send.clone()).await;
            assert_eq!(
                tagged.status, 200,
                "case {kind}/{}: expected HTTP 200 from TagResource, got {} with body {}",
                step.case, tagged.status, tagged.body
            );
            let listed = emulator.list_tags(&resource_arn).await;
            assert_eq!(
                listed.reported_tags(),
                expected(&step.after),
                "case {kind}/{}: expected the merged set {:?}, got {}",
                step.case,
                expected(&step.after),
                listed.body
            );
            assert_eq!(
                listed.string("/resourceTags/arn"),
                resource_arn,
                "case {kind}/{}: expected the resource's own ARN, got {}",
                step.case,
                listed.body
            );
        }
    }
}

/// Tags a resource was CREATED with are the same stored set the tagging operations work
/// on: `ListTagsForResource` reports them and a later `TagResource` merges into them.
#[tokio::test]
async fn tags_from_the_create_call_are_the_set_the_tagging_operations_see() {
    let emulator = Emulator::new();
    let cases: Vec<(&str, String)> = vec![
        (
            "queue",
            emulator
                .queue("born-tagged", json!({ "env": "prod", "team": "video" }))
                .await,
        ),
        (
            "job",
            emulator
                .job(json!({ "env": "prod", "team": "video" }))
                .await,
        ),
        (
            "preset",
            emulator
                .preset("born-tagged", json!({ "env": "prod", "team": "video" }))
                .await,
        ),
    ];

    for (case, resource_arn) in cases {
        let listed = emulator.list_tags(&resource_arn).await;
        assert_eq!(
            listed.reported_tags(),
            expected(&[("env", "prod"), ("team", "video")]),
            "case {case}: expected the tags the create call persisted, got {}",
            listed.body
        );

        let tagged = emulator.tag(&resource_arn, json!({ "env": "dev" })).await;
        assert_eq!(
            tagged.status, 200,
            "case {case}: expected HTTP 200 from TagResource, got {} with body {}",
            tagged.status, tagged.body
        );
        let listed = emulator.list_tags(&resource_arn).await;
        assert_eq!(
            listed.reported_tags(),
            expected(&[("env", "dev"), ("team", "video")]),
            "case {case}: expected the create-time set with env replaced, got {}",
            listed.body
        );
    }
}

/// `UntagResource` removes the keys it names and nothing else — over a table whose rows
/// differ in how many keys are removed, whether a named key is even present (removing an
/// absent key is a no-op, so the call is idempotent), and whether any key is named at
/// all.
#[tokio::test]
async fn untagging_removes_only_the_keys_it_names() {
    /// One step: the keys to remove, then the whole set expected afterwards.
    struct Step {
        case: &'static str,
        keys: Option<&'static [&'static str]>,
        after: Vec<(&'static str, &'static str)>,
    }

    let full = [
        ("archived", ""),
        ("env", "prod"),
        ("owner", "ops"),
        ("team", "video"),
    ];
    let steps = vec![
        Step {
            case: "no TagKeys at all removes nothing",
            keys: None,
            after: full.to_vec(),
        },
        Step {
            case: "an empty key list removes nothing",
            keys: Some(&[]),
            after: full.to_vec(),
        },
        Step {
            case: "a key the resource does not carry is a no-op",
            keys: Some(&["absent"]),
            after: full.to_vec(),
        },
        Step {
            case: "one key of several",
            keys: Some(&["env"]),
            after: vec![("archived", ""), ("owner", "ops"), ("team", "video")],
        },
        Step {
            case: "the same key again is idempotent",
            keys: Some(&["env"]),
            after: vec![("archived", ""), ("owner", "ops"), ("team", "video")],
        },
        Step {
            case: "two keys at once, one of them absent",
            keys: Some(&["owner", "env"]),
            after: vec![("archived", ""), ("team", "video")],
        },
        Step {
            case: "the remaining keys",
            keys: Some(&["archived", "team"]),
            after: Vec::new(),
        },
    ];

    for kind in ["queue", "job", "jobTemplate", "preset"] {
        let emulator = Emulator::new();
        let resource_arn = match kind {
            "queue" => emulator.queue("untagged", json!({})).await,
            "job" => emulator.job(json!({})).await,
            "jobTemplate" => emulator.first_job_template().await,
            _ => emulator.preset("untagged", json!({})).await,
        };
        let tagged = emulator
            .tag(
                &resource_arn,
                json!({ "archived": "", "env": "prod", "owner": "ops", "team": "video" }),
            )
            .await;
        assert_eq!(
            tagged.status, 200,
            "case {kind}: the fixture tags must be stored, got {} with body {}",
            tagged.status, tagged.body
        );

        for step in &steps {
            let untagged = emulator.untag(&resource_arn, step.keys).await;
            assert_eq!(
                untagged.status, 200,
                "case {kind}/{}: expected HTTP 200 from UntagResource, got {} with body {}",
                step.case, untagged.status, untagged.body
            );
            let listed = emulator.list_tags(&resource_arn).await;
            assert_eq!(
                listed.reported_tags(),
                expected(&step.after),
                "case {kind}/{}: expected {:?} to remain, got {}",
                step.case,
                expected(&step.after),
                listed.body
            );
        }
    }
}

/// An ARN that names no resource THIS store holds is the modelled `NotFoundException`
/// (404) from every tagging operation.
#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_not_found() {
    let emulator = Emulator::new();
    // One queue and one job DO exist, so a 404 is about the ARN, not an empty store.
    let _ = emulator.queue("present", json!({ "env": "prod" })).await;
    let _ = emulator.job(json!({})).await;

    // The resource ids come from the ARN forms MediaConvert documents as taggable:
    // `queues/`, `jobs/`, `presets/` and `jobTemplates/`.
    let cases = [
        ("a queue nobody created", arn("queues/absent")),
        ("a job id nobody was given", arn("jobs/1700000000000-zzzzzz")),
        ("a preset nobody created", arn("presets/Not-A-Preset")),
        (
            "a job template nobody created",
            arn("jobTemplates/Not-A-Template"),
        ),
    ];

    for (case, resource_arn) in cases {
        for (operation, reply) in [
            ("TagResource", emulator.tag(&resource_arn, json!({ "a": "b" })).await),
            ("ListTagsForResource", emulator.list_tags(&resource_arn).await),
            ("UntagResource", emulator.untag(&resource_arn, Some(&["a"])).await),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {case} / {operation}: expected HTTP 404, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("NotFoundException"),
                "case {case} / {operation}: expected NotFoundException, got {:?} with body {}",
                reply.error_type,
                reply.body
            );
        }
    }

    // The resources that DO exist are untouched by the refusals above.
    let listed = emulator.list_tags(&arn("queues/present")).await;
    assert_eq!(
        listed.reported_tags(),
        expected(&[("env", "prod")]),
        "the existing queue's tags must survive the refusals, got {}",
        listed.body
    );
}

/// A value that is not a MediaConvert resource ARN is a problem in the REQUEST: every
/// tagging operation answers the modelled `BadRequestException` (400), not a 404 and not
/// a success.
#[tokio::test]
async fn a_value_that_is_not_a_mediaconvert_arn_is_a_bad_request() {
    let emulator = Emulator::new();
    let _ = emulator.queue("present", json!({})).await;

    let cases = [
        ("a bare queue name", "present".to_string()),
        ("an ARN of another service", "arn:aws:s3:::my-bucket".to_string()),
        (
            "a MediaConvert ARN with an unknown resource type",
            arn("certificates/some-acm-arn"),
        ),
        ("a MediaConvert ARN with no resource type", arn("present")),
        ("a MediaConvert ARN with an empty resource name", arn("queues/")),
        ("a truncated ARN", "arn:aws:mediaconvert".to_string()),
        ("a value that is no ARN at all", "not-an-arn".to_string()),
    ];

    for (case, value) in cases {
        for (operation, reply) in [
            ("TagResource", emulator.tag(&value, json!({ "a": "b" })).await),
            ("ListTagsForResource", emulator.list_tags(&value).await),
            ("UntagResource", emulator.untag(&value, Some(&["a"])).await),
        ] {
            assert_eq!(
                reply.status, 400,
                "case {case} / {operation}: expected HTTP 400, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("BadRequestException"),
                "case {case} / {operation}: expected BadRequestException, got {:?} with body {}",
                reply.error_type,
                reply.body
            );
        }
    }

    // A tag with no key names nothing, so it is refused the same way.
    let no_key = emulator
        .tag(&arn("queues/present"), json!({ "": "value" }))
        .await;
    assert_eq!(
        no_key.status, 400,
        "case an empty tag key: expected HTTP 400, got {} with body {}",
        no_key.status, no_key.body
    );
    assert_eq!(
        no_key.error_type.as_deref(),
        Some("BadRequestException"),
        "case an empty tag key: expected BadRequestException, got {:?}",
        no_key.error_type
    );
    let listed = emulator.list_tags(&arn("queues/present")).await;
    assert_eq!(
        listed.reported_tags(),
        BTreeMap::new(),
        "a refused TagResource must persist nothing, got {}",
        listed.body
    );
}

/// Caller-driven growth is bounded: a resource holds tags up to the cap the state module
/// publishes, and a call that would carry it past the cap is the modelled quota refusal
/// — with the stored set left exactly as it was.
#[tokio::test]
async fn tagging_past_the_cap_is_a_quota_refusal_that_changes_nothing() {
    // The cap the state module publishes, so this test cannot drift from it.
    let cap = service_com_amazonaws_mediaconvert_mediaconvert::state::MAX_TAGS_PER_RESOURCE;
    let emulator = Emulator::new();
    let resource_arn = emulator.queue("capped", json!({})).await;

    let mut at_cap = serde_json::Map::new();
    for index in 0..cap {
        at_cap.insert(format!("key-{index}"), Value::String(index.to_string()));
    }
    let filled = emulator.tag(&resource_arn, Value::Object(at_cap)).await;
    assert_eq!(
        filled.status, 200,
        "case exactly {cap} tags: expected HTTP 200, got {} with body {}",
        filled.status, filled.body
    );

    for case in ["one key past the cap", "another key past the cap"] {
        let mut over = serde_json::Map::new();
        over.insert(format!("over-{case}"), Value::String("x".to_string()));
        let refused = emulator.tag(&resource_arn, Value::Object(over)).await;
        assert_eq!(
            refused.status, 402,
            "case {case}: expected HTTP 402, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {case}: expected ServiceQuotaExceededException, got {:?}",
            refused.error_type
        );
    }

    // Replacing the value of a key already present does not grow the set, so it is
    // still accepted at the cap.
    let replaced = emulator.tag(&resource_arn, json!({ "key-0": "replaced" })).await;
    assert_eq!(
        replaced.status, 200,
        "case replace at the cap: expected HTTP 200, got {} with body {}",
        replaced.status, replaced.body
    );

    let listed = emulator.list_tags(&resource_arn).await;
    let reported = listed.reported_tags();
    assert_eq!(
        reported.len(),
        cap,
        "the refusals must not grow the stored set past {cap}, got {}",
        listed.body
    );
    assert_eq!(
        reported.get("key-0").map(String::as_str),
        Some("replaced"),
        "the accepted replacement must be the stored value, got {}",
        listed.body
    );
}

/// The ARN `ListTagsForResource` reports names the region of the request that read it —
/// the same derivation every other ARN this service reports goes through — while the tag
/// set it reports is the one set that was stored.
#[tokio::test]
async fn the_reported_arn_follows_the_reading_region() {
    let emulator = Emulator::new();
    let resource_arn = emulator.queue("regional", json!({ "env": "prod" })).await;

    for region in ["us-west-2", "eu-central-1", "ap-southeast-2"] {
        let listed = emulator
            .list_tags_in(region, &arn_in(region, "queues/regional"))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {region:?}: expected HTTP 200, got {} with body {}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.string("/resourceTags/arn"),
            arn_in(region, "queues/regional"),
            "case {region:?}: expected the ARN in the reading region, got {}",
            listed.body
        );
        assert_eq!(
            listed.reported_tags(),
            expected(&[("env", "prod")]),
            "case {region:?}: expected the one stored tag set, got {}",
            listed.body
        );
    }

    // And an ARN whose own region is not the reading request's still names the same
    // resource: a record's identity here is its kind and name, not the region in the ARN
    // (which this store derives per read), so the foreign-region ARN resolves and the
    // reported ARN comes back in the READING region.
    let listed = emulator
        .list_tags_in(REGION, &arn_in("eu-central-1", "queues/regional"))
        .await;
    assert_eq!(
        listed.status, 200,
        "case a foreign-region ARN: expected HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.string("/resourceTags/arn"),
        arn("queues/regional"),
        "case a foreign-region ARN: expected the ARN in the reading region, got {}",
        listed.body
    );

    // And the suite's own default-region ARN still names the same resource.
    let listed = emulator.list_tags(&resource_arn).await;
    assert_eq!(
        listed.reported_tags(),
        expected(&[("env", "prod")]),
        "the default-region ARN must name the same resource, got {}",
        listed.body
    );
}
