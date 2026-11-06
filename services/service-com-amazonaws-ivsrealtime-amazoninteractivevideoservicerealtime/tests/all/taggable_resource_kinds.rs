// SPDX-License-Identifier: CC0-1.0

//! The `/tags/{resourceArn}` surface resolves "which stored resource does this ARN name"
//! from ONE table of resource kinds, so its read operation and its two write operations
//! answer about exactly the same set of resources.
//!
//! The rule: an ARN is resolved to a resource kind this store holds — a stage, a
//! composition, an ingest configuration, an encoder configuration, a storage configuration
//! or a public key — and `ListTagsForResource`, `TagResource` and `UntagResource` all reach
//! the record of that one resolution; an ARN whose resource type names a kind the store
//! never holds is a 404 on all three, under this surface's own `NotFoundException` code
//! and its one fixed sentence.
//!
//! Each test drives the service through the router's own `dispatch`, so every assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body. The
//! rows span the resource KIND, which is what the table generalises over: every kind this
//! service mints, and — on the refusing side — a type this model declares an ARN shape for
//! but no operation of this service can put in the store.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The region every call here is signed for, and so the region every fixture is created
/// in: a resource is addressed through the region holding it.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries (`engine::mock_account_id`), and
/// so the account of the ARNs built by hand here.
const ACCOUNT: &str = "000000000000";

/// A low-latency channel of the sibling `ivs` endpoint, carried only so a
/// `StartComposition` request has a well-formed destination.
const CHANNEL_ARN: &str = "arn:aws:ivs:us-east-1:000000000000:channel/live-channel";

/// What a tagging write answers: the recorded AWS responses to `TagResource` and
/// `UntagResource` are 204 with no body.
const NO_CONTENT_STATUS: u16 = 204;

/// A kind of resource this store can hold, with the create of this service that mints
/// one.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Stage,
    Composition,
    IngestConfiguration,
    EncoderConfiguration,
    StorageConfiguration,
}

/// One emulator holding only this service, driven through the router that serves
/// production traffic.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` as an SDK does: `POST /<operation>`, signed for [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.send(
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// One of the three operations bound to `/tags/{resourceArn}`, carrying the ARN in the
    /// URI the way a client does: percent-encoded into one path segment.
    async fn tagging(
        &self,
        method: &str,
        arn: &str,
        query: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let path = format!("/tags/{}{query}", encode_path_label(arn));
        self.send(
            method,
            &path,
            body.map(|body| bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    async fn send(&self, method: &str, path: &str, body: Option<bytes::Bytes>) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivsrealtime.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(body.unwrap_or_default())
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        // Every non-empty response of a restJson1 service is JSON, so a body that does
        // not parse is a failure to report rather than an absent member to tolerate.
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|error| {
                panic!(
                    "{method} {path} answered {status} with a body that is not JSON: {error}"
                )
            })
        };
        (status, parsed)
    }

    /// Create one resource in [`REGION`] and return the ARN the service minted, read out
    /// of `member` of the answer.
    async fn created(&self, operation: &str, body: &Value, member: &str) -> String {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} {body} must answer HTTP 200, got {status} {response}"
        );
        string_member(&response[member], "arn")
    }

    /// Create a stage, which three of the kinds below are built on.
    async fn create_stage(&self) -> String {
        self.created("CreateStage", &json!({ "name": "taggable" }), "stage")
            .await
    }

    /// Create one resource of `kind`, with whatever other resource the create needs, and
    /// return the ARN the service minted for it.
    async fn create(&self, kind: Kind) -> String {
        match kind {
            Kind::Stage => self.create_stage().await,
            Kind::Composition => {
                let stage = self.create_stage().await;
                self.created(
                    "StartComposition",
                    &json!({
                        "stageArn": stage,
                        "destinations": [{ "channel": { "channelArn": CHANNEL_ARN } }],
                    }),
                    "composition",
                )
                .await
            }
            Kind::IngestConfiguration => {
                let stage = self.create_stage().await;
                self.created(
                    "CreateIngestConfiguration",
                    &json!({ "ingestProtocol": "RTMPS", "stageArn": stage }),
                    "ingestConfiguration",
                )
                .await
            }
            Kind::EncoderConfiguration => {
                self.created(
                    "CreateEncoderConfiguration",
                    &json!({ "name": "taggable" }),
                    "encoderConfiguration",
                )
                .await
            }
            Kind::StorageConfiguration => {
                self.created(
                    "CreateStorageConfiguration",
                    &json!({ "s3": { "bucketName": "recordings" }, "name": "taggable" }),
                    "storageConfiguration",
                )
                .await
            }
        }
    }

    /// The tags `ListTagsForResource` — the READ side of the table — answers for `arn`.
    async fn tags_of(&self, case: &str, arn: &str) -> HashMap<String, String> {
        let (status, response) = self.tagging("GET", arn, "", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource on {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        serde_json::from_value(response["tags"].clone()).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: ListTagsForResource on {arn} must answer its `@required` \
                 `tags` map, got {response}: {error}"
            )
        })
    }
}

/// A string member, read as the empty string when absent — what a client sees for a member
/// the wire omits.
fn string_member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The restJson1 error discriminator of a refusal.
fn error_code(response: &Value) -> String {
    string_member(response, "__type")
}

/// Percent-encode one `@httpLabel` value the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment. Every character an
/// IVS ARN can hold is ASCII, so one byte per character.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// An IVS ARN of `resource_type` in [`REGION`], naming the resource `id`.
fn arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:{resource_type}/{id}")
}

/// The code and the sentence the tagging surface states for an ARN it holds no resource
/// for: its own, which name neither the ARN nor the kind of resource the ARN named (see
/// `tagging.rs`).
const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";
const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The three methods the tagging surface binds, with the query and body that make each a
/// well-formed call: `GET` reads, `POST` writes one tag, `DELETE` removes one key.
fn every_tagging_method() -> [(&'static str, &'static str, Option<Value>); 3] {
    [
        ("GET", "", None),
        ("POST", "", Some(json!({ "tags": { "team": "video" } }))),
        ("DELETE", "?tagKeys=team", None),
    ]
}

/// Every kind the store holds resolves on BOTH sides of the surface: what `TagResource`
/// writes for a resource of that kind is what `ListTagsForResource` reads back, and what
/// `UntagResource` removes is gone from that same answer.
///
/// The rows are the resource kinds — the one thing the resolution table generalises over —
/// and each row writes a tag set of a different size, so the kind is not entangled with the
/// shape of the write.
#[tokio::test]
async fn every_stored_resource_kind_resolves_on_both_the_read_and_the_write_tag_path() {
    // (case, kind, the tags TagResource writes)
    let rows: Vec<(&str, Kind, HashMap<String, String>)> = vec![
        (
            "stage",
            Kind::Stage,
            HashMap::from([("team".to_string(), "video".to_string())]),
        ),
        (
            "composition",
            Kind::Composition,
            HashMap::from([
                ("team".to_string(), "video".to_string()),
                ("env".to_string(), "sim2real".to_string()),
            ]),
        ),
        (
            "ingest configuration",
            Kind::IngestConfiguration,
            HashMap::from([("owner".to_string(), "behavioral".to_string())]),
        ),
        (
            "encoder configuration",
            Kind::EncoderConfiguration,
            HashMap::from([
                ("owner".to_string(), "behavioral".to_string()),
                ("cost-centre".to_string(), "42".to_string()),
                ("env".to_string(), "sim2real".to_string()),
            ]),
        ),
        (
            "storage configuration",
            Kind::StorageConfiguration,
            HashMap::from([("env".to_string(), "sim2real".to_string())]),
        ),
    ];

    for (case, kind, written) in rows {
        let realtime = RealTime::new();
        let resource = realtime.create(kind).await;

        // The read side answers for the resource the create persisted, which carried no
        // tags.
        assert_eq!(
            realtime.tags_of(case, &resource).await,
            HashMap::new(),
            "case {case:?}: a resource created without tags must read back as untagged"
        );

        // The write side reaches the same record: what it wrote is what the read answers.
        let (status, response) = realtime
            .tagging("POST", &resource, "", Some(json!({ "tags": written })))
            .await;
        assert_eq!(
            status, NO_CONTENT_STATUS,
            "case {case:?}: TagResource on {resource} must answer {NO_CONTENT_STATUS}, \
             got {status} {response}"
        );
        assert_eq!(
            realtime.tags_of(case, &resource).await,
            written,
            "case {case:?}: ListTagsForResource must answer the tags TagResource wrote"
        );

        // And so does the removing write.
        let mut keys: Vec<String> = written.keys().cloned().collect();
        keys.sort();
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={key}"))
            .collect::<Vec<String>>()
            .join("&");
        let (status, response) = realtime
            .tagging("DELETE", &resource, &format!("?{query}"), None)
            .await;
        assert_eq!(
            status, NO_CONTENT_STATUS,
            "case {case:?}: UntagResource on {resource} must answer {NO_CONTENT_STATUS}, \
             got {status} {response}"
        );
        assert_eq!(
            realtime.tags_of(case, &resource).await,
            HashMap::new(),
            "case {case:?}: ListTagsForResource must answer the removal UntagResource made"
        );
    }
}

/// The refusing side of the same table: an ARN whose resource type names no kind this store
/// holds is the tagging surface's own not-found on the read operation and on both writes
/// alike — the two
/// sides cannot disagree about which kinds are resolvable.
///
/// The rows differ in WHY nothing is held: a kind the store does hold whose record it
/// never had (a public key, and a storage configuration), and one whose record it held and
/// the delete removed.
#[tokio::test]
async fn an_arn_of_no_stored_kind_is_a_not_found_on_the_read_and_the_write_tag_path() {
    let realtime = RealTime::new();

    let deleted = realtime.create_stage().await;
    let (status, response) = realtime.call("DeleteStage", &json!({ "arn": deleted })).await;
    assert_eq!(
        status, 200,
        "the fixture stage must really be deleted, got {status} {response}"
    );

    let cases = [
        (
            "a stored kind whose record this store never held".to_string(),
            arn("public-key", "0123456789ab"),
        ),
        (
            "another stored kind whose record this store never held".to_string(),
            arn("storage-configuration", "ffffffffffff"),
        ),
        ("a stored kind that was deleted".to_string(), deleted),
    ];

    for (case, missing) in cases {
        for (method, query, body) in every_tagging_method() {
            let (status, response) = realtime.tagging(method, &missing, query, body).await;
            assert_eq!(
                (
                    status,
                    error_code(&response),
                    string_member(&response, "message")
                ),
                (
                    404,
                    TAGGING_NOT_FOUND_CODE.to_string(),
                    TAGGING_NOT_FOUND_MESSAGE.to_string()
                ),
                "case {case:?} ({method}): an ARN naming no stored resource must be the \
                 tagging surface's not-found, got {status} {response}"
            );
        }
    }
}
