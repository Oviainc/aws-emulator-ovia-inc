// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the ONE classify-and-word rule every door onto a record's
//! tag set shares.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from the
//! request's SigV4 scope, the framework deserializes and validates, the handler runs, and
//! the response is serialized — so what is asserted is the bytes a client would see.
//!
//! The rule under test, which every row below is one case of: a tag set a record cannot
//! carry earns ONE answer whichever door was asked — a key carrying no characters names
//! nothing a later tagging call could address, so it is a problem in the request
//! (`BadRequestException`, HTTP 400), and a set larger than the per-resource tag cap would
//! leave the record carrying more tags than one holds, so it is a quota refusal
//! (`ServiceQuotaExceededException`, HTTP 402) — and each states the single sentence the
//! state module words that refusal with.
//!
//! The rows span the five doors that write a record's `tags` (the four `Create*` paths and
//! `TagResource`, the operations whose input the model gives a `Tags` member:
//! `smithy_query(query="digest")`), the two kinds of refusal, and, inside each kind, the
//! ways a set can earn it — a bad key alone, beside good pairs, or inside an over-cap map;
//! a set one past the cap and one far past it — so the status, the error shape and the
//! sentence cannot land on one door and miss another. What a STORED set reads back as is
//! the business of `tests/all/resource_tagging.rs`; the accepted row here is only the
//! other side of the same decision.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    empty_tag_key_message, tags_past_cap_message, MAX_TAGS_PER_RESOURCE,
};
use service_com_amazonaws_mediaconvert_mediaconvert::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every case signs for.
const REGION: &str = "us-west-2";

/// An IAM role ARN of the shape `CreateJob`'s `Role` member documents.
const ROLE: &str = "arn:aws:iam::123456789012:role/MediaConvertRole";

/// A SigV4 `Authorization` header in this service's scope. It is what the router reads to
/// pick the service, and what a handler reads the ARN region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDTESTTESTTESTTEST/20260101/{region}/mediaconvert/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
    )
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
    /// The `message` the error body states, or `None` when it carries none.
    fn message(&self) -> Option<&str> {
        self.body.pointer("/message").and_then(Value::as_str)
    }
}

/// One emulator: a single shared engine behind the multi-service router, exactly as the
/// production router assembles it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is the transport's business, not this suite's; the
        // Authorization header is still present because routing and the ARN region read
        // the credential scope out of it.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let request = ::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("mediaconvert.{REGION}.amazonaws.com"))
            .header("authorization", authorization(REGION))
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
}

/// A door onto a record's tag set: the five operations that WRITE a record's `tags`.
#[derive(Clone, Copy, Debug)]
enum Door {
    CreateQueue,
    CreateJob,
    CreatePreset,
    CreateJobTemplate,
    TagResource,
}

impl Door {
    /// Every door this service has onto a record's tags, so a case is stated once and run
    /// against each.
    const ALL: [Self; 5] = [
        Self::CreateQueue,
        Self::CreateJob,
        Self::CreatePreset,
        Self::CreateJobTemplate,
        Self::TagResource,
    ];

    /// The status this door answers a tag set it ACCEPTS with: 201 for a create, which
    /// mints a resource, and 200 for the merge `TagResource` performs.
    fn accepted_status(self) -> u16 {
        match self {
            Self::TagResource => 200,
            _ => 201,
        }
    }

    /// Ask this door to put `tags` on a record, on a store of its own: a create sends them
    /// as the new record's `Tags`, and `TagResource` merges them into a queue that is
    /// already there.
    async fn call(self, emulator: &Emulator, tags: &Value) -> Reply {
        let name = "shared-tag-refusal-probe";
        match self {
            Self::CreateQueue => {
                emulator
                    .send(
                        "POST",
                        "/2017-08-29/queues",
                        Some(json!({ "name": name, "tags": tags })),
                    )
                    .await
            }
            Self::CreateJob => {
                emulator
                    .send(
                        "POST",
                        "/2017-08-29/jobs",
                        Some(json!({
                            "role": ROLE,
                            "settings": crate::job_lifecycle::settings(
                                "s3://example-bucket/in/a.mp4",
                                "_a",
                            ),
                            "tags": tags
                        })),
                    )
                    .await
            }
            Self::CreatePreset => {
                emulator
                    .send(
                        "POST",
                        "/2017-08-29/presets",
                        Some(json!({
                            "name": name,
                            "settings": { "containerSettings": { "container": "MP4" } },
                            "tags": tags
                        })),
                    )
                    .await
            }
            Self::CreateJobTemplate => {
                emulator
                    .send(
                        "POST",
                        "/2017-08-29/jobTemplates",
                        Some(json!({
                            "name": name,
                            "settings": crate::job_template_crud::settings(1, 1),
                            "tags": tags
                        })),
                    )
                    .await
            }
            Self::TagResource => {
                let created = emulator
                    .send("POST", "/2017-08-29/queues", Some(json!({ "name": name })))
                    .await;
                assert_eq!(
                    created.status, 201,
                    "fixture queue for {self:?}: expected HTTP 201, got {} with body {}",
                    created.status, created.body
                );
                // Built by the emulator's own ARN helper, so the ARN this door is handed
                // is not a second hand-written format.
                let resource_arn =
                    engine::emulator_arn("mediaconvert", REGION, &format!("queues/{name}"));
                emulator
                    .send(
                        "POST",
                        "/2017-08-29/tags",
                        Some(json!({ "arn": resource_arn, "tags": tags })),
                    )
                    .await
            }
        }
    }
}

/// What a tag set earns from a door, as a client sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    /// The set is one the record can carry, so the door does its work.
    None,
    /// A key carrying no characters: the modelled `BadRequestException` (HTTP 400).
    BadRequest,
    /// More tags than one record holds: the modelled `ServiceQuotaExceededException`
    /// (HTTP 402).
    Quota,
}

impl Refusal {
    /// The HTTP status, the `x-amzn-errortype` and the sentence this refusal states —
    /// the wording taken from the state module that owns it, never a second copy of it
    /// here.
    fn expected(self) -> Option<(u16, &'static str, String)> {
        match self {
            Self::None => None,
            Self::BadRequest => Some((400, "BadRequestException", empty_tag_key_message())),
            Self::Quota => Some((
                402,
                "ServiceQuotaExceededException",
                tags_past_cap_message(),
            )),
        }
    }
}

/// `count` distinct pairs, as a `Tags` member.
fn tag_map(count: usize) -> Value {
    let mut map = serde_json::Map::new();
    for index in 0..count {
        map.insert(format!("key-{index}"), Value::String(index.to_string()));
    }
    Value::Object(map)
}

/// The same, with a key that carries no characters added to it.
fn tag_map_with_empty_key(count: usize) -> Value {
    let mut map = tag_map(count);
    if let Some(object) = map.as_object_mut() {
        object.insert(String::new(), Value::String("v".to_string()));
    }
    map
}

/// A tag set a record cannot carry earns the same status, the same error shape and the
/// same sentence from EVERY door onto that record's tags, and a set it can carry is
/// accepted by every one of them.
#[tokio::test]
async fn every_door_classifies_a_tag_refusal_alike() {
    /// One row of the table.
    struct Case {
        case: &'static str,
        tags: Value,
        want: Refusal,
    }

    // The cap the state module publishes, so this table cannot drift from it.
    let cap = MAX_TAGS_PER_RESOURCE;
    let cases = vec![
        Case {
            case: "a set the record can carry",
            tags: json!({ "env": "prod", "team": "video" }),
            want: Refusal::None,
        },
        Case {
            case: "a key with no characters, alone",
            tags: json!({ "": "v" }),
            want: Refusal::BadRequest,
        },
        Case {
            case: "a key with no characters beside good pairs",
            tags: json!({ "": "v", "env": "prod" }),
            want: Refusal::BadRequest,
        },
        Case {
            // A problem in the request is answered as such even when the same set is also
            // over the cap, on every door alike.
            case: "a key with no characters inside an over-cap map",
            tags: tag_map_with_empty_key(cap + 1),
            want: Refusal::BadRequest,
        },
        Case {
            case: "one key past the cap",
            tags: tag_map(cap + 1),
            want: Refusal::Quota,
        },
        Case {
            case: "ten times the cap",
            tags: tag_map(cap * 10),
            want: Refusal::Quota,
        },
    ];

    for case in &cases {
        for door in Door::ALL {
            // A store of its own per row and door, so no earlier case's record decides
            // this one's answer.
            let emulator = Emulator::new();
            let reply = door.call(&emulator, &case.tags).await;
            let label = format!("{:?}/{door:?}", case.case);

            let Some((status, error_type, message)) = case.want.expected() else {
                assert_eq!(
                    reply.status,
                    door.accepted_status(),
                    "case {label}: expected HTTP {}, got {} with body {}",
                    door.accepted_status(),
                    reply.status,
                    reply.body
                );
                continue;
            };
            assert_eq!(
                reply.status, status,
                "case {label}: expected HTTP {status}, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some(error_type),
                "case {label}: expected the {error_type} error type, got {:?} with body {}",
                reply.error_type,
                reply.body
            );
            assert_eq!(
                reply.message(),
                Some(message.as_str()),
                "case {label}: expected the one refusal sentence {message:?}, got {}",
                reply.body
            );
        }
    }
}
