// SPDX-License-Identifier: CC0-1.0

//! Both Apache Airflow entities an `InvokeRestApi` write can carry judge an optional
//! `description` by the SAME rule, end to end over HTTP through the router this service
//! mounts in production.
//!
//! The rule under test: a write's body may carry a `description`, and when it carries one
//! that description is held to `state::MAX_AIRFLOW_VALUE_LENGTH` characters — a longer one
//! is refused, with the same sentence whichever entity carries it, and nothing is stored —
//! while a body carrying none (an absent member, or the `null` that spells one) states no
//! length to bound and is taken. A variable write and a pool write therefore cannot
//! disagree about what a `description` may be.
//!
//! Rows differ along what the rule generalises over: whether the member is there at all,
//! how many characters it carries (none, one, a sentence, exactly the bound, one past it,
//! far past it) and how wide those characters are (ASCII, and multi-byte letters, since
//! the bound counts characters and not bytes). Every row is driven through all FOUR write
//! paths that reach the two parsers — `POST /variables`, `PATCH /variables/{key}`,
//! `POST /pools` and `PATCH /pools/{name}` — and an accepted description is read back
//! through the entity's own `GET`, so the answer each path gives is compared against the
//! others and against stored state.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The environment every call below is addressed to.
const ENVIRONMENT: &str = "DescriptionBound";

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The wire error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// `RestApiStatusCode`: the status the Apache Airflow REST API call answered with.
    fn airflow_status(&self) -> i64 {
        self.json()
            .get("RestApiStatusCode")
            .and_then(Value::as_i64)
            .unwrap_or_default()
    }

    /// `RestApiResponse`: the document that call answered with, or `Null` where it carried
    /// none.
    fn document(&self) -> Value {
        self.json()
            .get("RestApiResponse")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// One member of the answered document, `Null` where it carries none.
    fn member(&self, name: &str) -> Value {
        self.document().get(name).cloned().unwrap_or(Value::Null)
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serialisable body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "api.airflow.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        Answer {
            status: response.status().as_u16(),
            body: response.into_body().to_vec(),
        }
    }

    /// One Apache Airflow REST API call against the fixture environment.
    async fn invoke(&self, call: Value) -> Answer {
        self.send("POST", &format!("/restapi/{ENVIRONMENT}"), Some(call))
            .await
    }

    /// The fixture environment, created with every optional member absent.
    async fn with_environment() -> Self {
        let harness = Self::new();
        let created = harness
            .send(
                "PUT",
                &format!("/environments/{ENVIRONMENT}"),
                Some(json!({
                    "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
                    "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket",
                    "DagS3Path": "dags",
                    "NetworkConfiguration": {
                        "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
                        "SecurityGroupIds": ["sg-00000000000000001"],
                    },
                })),
            )
            .await;
        assert_eq!(
            created.status,
            200,
            "the fixture create of {ENVIRONMENT} must succeed, got {} {}",
            created.status,
            created.text()
        );
        harness
    }
}

/// One Apache Airflow REST API call as a row states it.
fn call(method: &str, path: &str) -> Value {
    json!({"Method": method, "Path": path})
}

/// The same, carrying a body.
fn call_with_body(method: &str, path: &str, body: Value) -> Value {
    json!({"Method": method, "Path": path, "Body": body})
}

/// One `description` a write's body may state, as a row of the table below.
struct DescriptionCase {
    case: &'static str,
    /// The JSON the body carries under `"description"`, or `None` when the body carries no
    /// such member at all.
    description: Option<Value>,
    /// What the entity's own document must report under `"description"` once the write is
    /// served, for a description within the bound.
    stored: Value,
    /// Whether the rule takes this description.
    accepted: bool,
}

/// The descriptions the rule is asserted over. The ceiling is the store's own
/// `state::MAX_AIRFLOW_VALUE_LENGTH`, which bounds a variable value or a description in
/// CHARACTERS, so the table carries each bound-length row twice: once spelled in ASCII and
/// once in multi-byte letters, whose byte length is a multiple of their character count.
fn cases() -> Vec<DescriptionCase> {
    let longest = state::MAX_AIRFLOW_VALUE_LENGTH;
    vec![
        DescriptionCase {
            case: "no description member at all",
            description: None,
            stored: Value::Null,
            accepted: true,
        },
        DescriptionCase {
            case: "a null description",
            description: Some(Value::Null),
            stored: Value::Null,
            accepted: true,
        },
        DescriptionCase {
            case: "an empty description",
            description: Some(json!("")),
            stored: json!(""),
            accepted: true,
        },
        DescriptionCase {
            case: "a single character",
            description: Some(json!("d")),
            stored: json!("d"),
            accepted: true,
        },
        DescriptionCase {
            case: "a sentence",
            description: Some(json!("what this entity is for")),
            stored: json!("what this entity is for"),
            accepted: true,
        },
        DescriptionCase {
            case: "multi-byte letters",
            description: Some(json!("die Beschreibung für Zürich")),
            stored: json!("die Beschreibung für Zürich"),
            accepted: true,
        },
        DescriptionCase {
            case: "exactly the longest this store keeps, in ASCII",
            description: Some(json!("d".repeat(longest))),
            stored: json!("d".repeat(longest)),
            accepted: true,
        },
        DescriptionCase {
            case: "exactly the longest this store keeps, in multi-byte letters",
            description: Some(json!("ü".repeat(longest))),
            stored: json!("ü".repeat(longest)),
            accepted: true,
        },
        DescriptionCase {
            case: "one character past the longest this store keeps",
            description: Some(json!("d".repeat(longest + 1))),
            stored: Value::Null,
            accepted: false,
        },
        DescriptionCase {
            case: "one multi-byte character past the longest this store keeps",
            description: Some(json!("ü".repeat(longest + 1))),
            stored: Value::Null,
            accepted: false,
        },
        DescriptionCase {
            case: "far past the longest this store keeps",
            description: Some(json!("d".repeat(longest * 3))),
            stored: Value::Null,
            accepted: false,
        },
    ]
}

impl DescriptionCase {
    /// A variable write's body for this row.
    fn variable_body(&self, key: &str) -> Value {
        self.with_description(json!({"key": key, "value": "v"}))
    }

    /// A pool write's body for this row.
    fn pool_body(&self, name: &str) -> Value {
        self.with_description(json!({"name": name, "slots": 3}))
    }

    fn with_description(&self, mut body: Value) -> Value {
        if let Some(description) = self.description.clone() {
            body.as_object_mut()
                .expect("a write body is an object")
                .insert("description".to_string(), description);
        }
        body
    }
}

/// What a refused description must answer with, and the sentence it answered: a refusal of
/// one entity's description has to be the same answer as a refusal of the other's.
fn assert_refused(case: &str, path: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status,
        400,
        "case {case:?} on {path}: a refused Apache Airflow call is the modelled \
         exception's own HTTP 400, got {} {}",
        answer.status,
        answer.text()
    );
    assert_eq!(
        answer.error_code(),
        "RestApiClientException",
        "case {case:?} on {path}: expected RestApiClientException, got {}",
        answer.text()
    );
    assert_eq!(
        answer.airflow_status(),
        400,
        "case {case:?} on {path}: expected RestApiStatusCode 400, got {}",
        answer.text()
    );
    assert_eq!(
        answer.member("status"),
        json!(400),
        "case {case:?} on {path}: the problem document must name the same status, got {}",
        answer.document()
    );
    let detail = answer
        .member("detail")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        detail.contains("description"),
        "case {case:?} on {path}: the refusal must name the member it refused, got {detail:?}"
    );
    assert!(
        detail.contains(&state::MAX_AIRFLOW_VALUE_LENGTH.to_string()),
        "case {case:?} on {path}: the refusal must name the bound it held the description \
         to, got {detail:?}"
    );
    detail
}

/// A variable write and a pool write answer an optional `description` the same way, on
/// create and on change alike.
#[tokio::test]
async fn both_airflow_entities_bound_a_description_by_one_rule() {
    let harness = Harness::with_environment().await;

    for (index, case) in cases().into_iter().enumerate() {
        let variable_key = format!("variable{index}");
        let patched_key = format!("patchedVariable{index}");
        let pool_name = format!("pool{index}");
        let patched_pool = format!("patchedPool{index}");

        // The two entities a change is applied to, written with no description at all, so
        // a PATCH carrying this row's description is the only thing under test.
        for seed in [
            call_with_body(
                "POST",
                "/variables",
                json!({"key": patched_key, "value": "v"}),
            ),
            call_with_body("POST", "/pools", json!({"name": patched_pool, "slots": 3})),
        ] {
            let seeded = harness.invoke(seed).await;
            assert_eq!(
                seeded.status,
                200,
                "case {:?}: the fixture write must be served, got {} {}",
                case.case,
                seeded.status,
                seeded.text()
            );
        }

        // The four write paths that reach the two parsers, each with the entity's own
        // reader and the body that write carries.
        let writes = [
            (
                "POST /variables",
                call_with_body("POST", "/variables", case.variable_body(&variable_key)),
                format!("/variables/{variable_key}"),
            ),
            (
                "PATCH /variables/{key}",
                call_with_body(
                    "PATCH",
                    &format!("/variables/{patched_key}"),
                    case.variable_body(&patched_key),
                ),
                format!("/variables/{patched_key}"),
            ),
            (
                "POST /pools",
                call_with_body("POST", "/pools", case.pool_body(&pool_name)),
                format!("/pools/{pool_name}"),
            ),
            (
                "PATCH /pools/{name}",
                call_with_body(
                    "PATCH",
                    &format!("/pools/{patched_pool}"),
                    case.pool_body(&patched_pool),
                ),
                format!("/pools/{patched_pool}"),
            ),
        ];

        let mut refusals: Vec<(&str, String)> = Vec::new();
        for (path, body, reader) in writes {
            let answer = harness.invoke(body).await;
            if case.accepted {
                assert_eq!(
                    answer.status,
                    200,
                    "case {:?} on {path}: a description within the bound must be served, \
                     got {} {}",
                    case.case,
                    answer.status,
                    answer.text()
                );
                assert_eq!(
                    answer.member("description"),
                    case.stored,
                    "case {:?} on {path}: the write must report the description it stored, \
                     got {}",
                    case.case,
                    answer.document()
                );
                // And the next read of that entity reports the same description.
                let read = harness.invoke(call("GET", &reader)).await;
                assert_eq!(
                    read.status,
                    200,
                    "case {:?}: GET {reader} must be served, got {} {}",
                    case.case,
                    read.status,
                    read.text()
                );
                assert_eq!(
                    read.member("description"),
                    case.stored,
                    "case {:?}: GET {reader} must report the stored description, got {}",
                    case.case,
                    read.document()
                );
            } else {
                refusals.push((path, assert_refused(case.case, path, &answer)));
                // A refused write stored nothing: a create leaves no entity behind, and a
                // change leaves the one it addressed with the description it had, which is
                // none.
                let read = harness.invoke(call("GET", &reader)).await;
                let seeded = reader.ends_with(&patched_key) || reader.ends_with(&patched_pool);
                if seeded {
                    assert_eq!(
                        read.member("description"),
                        Value::Null,
                        "case {:?}: a refused change must leave {reader} as it was, got {}",
                        case.case,
                        read.document()
                    );
                } else {
                    assert_eq!(
                        read.airflow_status(),
                        404,
                        "case {:?}: a refused create must store no entity at {reader}, got {}",
                        case.case,
                        read.text()
                    );
                }
            }
        }

        // Whichever entity carried it, a refused description is refused with ONE sentence.
        if let Some((first_path, first_detail)) = refusals.first() {
            for (path, detail) in &refusals {
                assert_eq!(
                    detail, first_detail,
                    "case {:?}: {path} must refuse a description with the same sentence as \
                     {first_path}, got {detail:?}",
                    case.case
                );
            }
        }
    }
}
