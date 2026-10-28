// SPDX-License-Identifier: CC0-1.0

//! What a WRITE gets once an environment's teardown has started, end to end over HTTP
//! through the router this service mounts in production.
//!
//! The rule every case here asserts: a change is applied only to an environment that can
//! take one. A record whose teardown has already started still reads back, and still holds
//! its name, until the clock removes it — but it accepts no change, so every writer that
//! reaches a stored record refuses with HTTP 400 `ValidationException`, naming the
//! environment and the status it is in, and leaves the record exactly as it was. A read is
//! untouched by the rule: each case re-reads the record THROUGH the teardown, and those
//! reads have to keep answering.
//!
//! The rule is about the write, not about one operation or one request, so the table spans
//! what it generalises over: which writer carries the change (`TagResource`,
//! `UntagResource`, `InvokeRestApi`), which method a data-plane write arrives with (`POST`,
//! `PATCH`, `DELETE`, and a method that API serves no arm for), which entity it addresses
//! (a tag map, a variable, a slot pool), how many values it carries, and whether the change
//! would have altered anything at all — a request that asks for the state the record is
//! already in is still a write, and is refused like the rest.
//!
//! Every row is paired with a control through the SAME table: against an environment that
//! can take the change, that very write is served and its effect is readable. So no
//! assertion here could pass against a service that simply refused everything.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region the `AUTHORIZATION` scope above signs for.
const REGION: &str = "us-east-1";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// How often a wait below re-reads the environment. Far shorter than
/// [`state::TRANSITION_SPAN`], so no phase of a lifecycle slips between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The tags every environment in this suite is created with — what a refused tag write has
/// to leave behind.
const SEEDED_TAGS: [(&str, &str); 2] = [("Team", "data-platform"), ("Env", "staging")];

/// The Apache Airflow variable seeded through the data plane, so a row that changes or
/// removes one has an entity to address.
const SEEDED_VARIABLE: &str = "schedule";

/// The slot pool seeded through the data plane, for the same reason.
const SEEDED_POOL: &str = "etl";

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

    fn message(&self) -> String {
        let body = self.json();
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// `RestApiStatusCode`: the status the Apache Airflow REST API call answered with,
    /// which a success and a `RestApiClientException` both carry.
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
}

/// Everything a write in this suite could change, as this service's own readers report it:
/// the environment's tag map, its Apache Airflow variables and its slot pools.
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    tags: Value,
    variables: Value,
    pools: Value,
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

    /// Create one environment carrying [`SEEDED_TAGS`].
    async fn create(&self, name: &str) -> Answer {
        self.send(
            "PUT",
            &format!("/environments/{name}"),
            Some(json!({
                "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
                "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket",
                "DagS3Path": "dags",
                "NetworkConfiguration": {
                    "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
                    "SecurityGroupIds": ["sg-00000000000000001"],
                },
                "Tags": tag_map(&SEEDED_TAGS),
            })),
        )
        .await
    }

    async fn delete(&self, name: &str) -> Answer {
        self.send("DELETE", &format!("/environments/{name}"), None)
            .await
    }

    /// `TagResource`: merge `tags` into the environment the ARN names.
    async fn tag(&self, arn: &str, tags: &Value) -> Answer {
        self.send(
            "POST",
            &format!("/tags/{}", encode_label(arn)),
            Some(json!({"Tags": tags})),
        )
        .await
    }

    /// `UntagResource`: remove `keys` from the environment the ARN names.
    async fn untag(&self, arn: &str, keys: &[String]) -> Answer {
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={}", encode_query(key)))
            .collect::<Vec<_>>()
            .join("&");
        self.send("DELETE", &format!("/tags/{}?{query}", encode_label(arn)), None)
            .await
    }

    /// `InvokeRestApi`: one Apache Airflow REST API call against `name`.
    async fn invoke(&self, name: &str, call: &Value) -> Answer {
        self.send("POST", &format!("/restapi/{name}"), Some(call.clone()))
            .await
    }

    /// The `Status` a successful `GetEnvironment` reports.
    async fn status_of(&self, case: &str, name: &str) -> String {
        let answer = self.send("GET", &format!("/environments/{name}"), None).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected GetEnvironment 200, got {} {}",
            answer.status,
            answer.text()
        );
        answer
            .json()
            .pointer("/Environment/Status")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: GetEnvironment reported no Status"))
            .to_string()
    }

    /// Wait until the create settles, so every row starts from an environment that can
    /// take a change.
    async fn wait_until_available(&self, case: &str, name: &str) {
        let deadline = Instant::now() + state::TRANSITION_SPAN * 20;
        loop {
            let status = self.status_of(case, name).await;
            if status == state::STATUS_AVAILABLE {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the create never settled into {}, last saw {status}",
                state::STATUS_AVAILABLE
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The `Tags` map `ListTagsForResource` reports, or `Null` when it reports none.
    async fn tags_of(&self, case: &str, arn: &str) -> Value {
        let answer = self
            .send("GET", &format!("/tags/{}", encode_label(arn)), None)
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: ListTagsForResource must still report the record, got {} {}",
            answer.status,
            answer.text()
        );
        answer.json().get("Tags").cloned().unwrap_or(Value::Null)
    }

    /// The document one SERVED Apache Airflow read answers with — a read has to keep
    /// answering for as long as the record is there.
    async fn read_through_airflow(&self, case: &str, name: &str, path: &str) -> Value {
        let answer = self.invoke(name, &call("GET", path)).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a read of {path} must be served, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.airflow_status(),
            200,
            "case {case:?}: a read of {path} must answer the Apache Airflow 200, got {}",
            answer.text()
        );
        answer.document()
    }

    /// What every reader of this environment reports right now.
    async fn snapshot(&self, case: &str, name: &str, arn: &str) -> Snapshot {
        Snapshot {
            tags: self.tags_of(case, arn).await,
            variables: self.read_through_airflow(case, name, "/variables").await,
            pools: self.read_through_airflow(case, name, "/pools").await,
        }
    }

    /// One environment of this suite, settled and carrying the seeded tags, variable and
    /// pool.
    async fn seeded(case: &str, name: &str) -> Self {
        let harness = Self::new();
        let created = harness.create(name).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: the fixture create must succeed, got {} {}",
            created.status,
            created.text()
        );
        harness.wait_until_available(case, name).await;
        for seed in [
            call_with_body(
                "POST",
                "/variables",
                json!({"key": SEEDED_VARIABLE, "value": "@daily"}),
            ),
            call_with_body("POST", "/pools", json!({"name": SEEDED_POOL, "slots": 7})),
        ] {
            let written = harness.invoke(name, &seed).await;
            assert_eq!(
                written.status,
                200,
                "case {case:?}: the fixture write {seed} must succeed, got {} {}",
                written.status,
                written.text()
            );
        }
        harness
    }
}

/// Percent-encode the characters an ARN carries that are not legal in a single
/// (non-greedy) `@httpLabel` segment — exactly what an SDK does for `ResourceArn`.
fn encode_label(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// Percent-encode one `tagKeys` query value, the way an SDK encodes an `@httpQuery`
/// member: every byte outside RFC 3986's unreserved set is escaped, which is always a
/// legal encoding of the value.
fn encode_query(value: &str) -> String {
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

/// A `TagMap` JSON object of the given pairs. Built rather than written as a `json!`
/// literal because the keys are the table's own values.
fn tag_map(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect(),
    )
}

/// One Apache Airflow REST API call, as an `InvokeRestApi` input states it.
fn call(method: &str, path: &str) -> Value {
    json!({"Method": method, "Path": path})
}

/// The same, carrying a body.
fn call_with_body(method: &str, path: &str, body: Value) -> Value {
    json!({"Method": method, "Path": path, "Body": body})
}

/// The ARN of an environment of this suite, as this service mints it.
fn environment_arn(name: &str) -> String {
    format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/{name}")
}

/// Which writer carries one row's change, and what it asks for.
enum Write {
    /// `TagResource`, with the `Tags` map to merge.
    Tag(Value),
    /// `UntagResource`, with the `tagKeys` to remove.
    Untag(Vec<String>),
    /// `InvokeRestApi`, with one Apache Airflow REST API call that is not a `GET`.
    Airflow(Value),
}

/// One write, and what the service answers it with when the environment CAN take it.
struct Row {
    case: &'static str,
    write: Write,
    /// The operation's own HTTP status for a served write.
    served_status: u16,
    /// The Apache Airflow status beside it, for an [`Write::Airflow`] row.
    served_airflow_status: Option<i64>,
    /// Whether the served write leaves what the readers report different from before. A
    /// row that changes nothing is still a write, and still refused by the rule under test.
    changes_state: bool,
}

impl Row {
    /// Issue this row's write against `name`.
    async fn send(&self, harness: &Harness, name: &str) -> Answer {
        match &self.write {
            Write::Tag(tags) => harness.tag(&environment_arn(name), tags).await,
            Write::Untag(keys) => harness.untag(&environment_arn(name), keys).await,
            Write::Airflow(call) => harness.invoke(name, call).await,
        }
    }
}

/// A `tagKeys` list from its keys.
fn keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| (*key).to_string()).collect()
}

/// Every write this suite asserts the rule over. Rows differ along the writer, the method,
/// the entity, how many values travel in the request, and whether the change would have
/// altered anything.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "TagResource adds a key the environment does not carry",
            write: Write::Tag(tag_map(&[("Added", "while-deleting")])),
            served_status: 200,
            served_airflow_status: None,
            changes_state: true,
        },
        Row {
            case: "TagResource replaces the value of a key it does carry",
            write: Write::Tag(tag_map(&[("Team", "another-team")])),
            served_status: 200,
            served_airflow_status: None,
            changes_state: true,
        },
        Row {
            case: "TagResource rewrites a pair with the value already stored",
            write: Write::Tag(tag_map(&[SEEDED_TAGS[0]])),
            served_status: 200,
            served_airflow_status: None,
            changes_state: false,
        },
        Row {
            case: "TagResource writes several pairs at once",
            write: Write::Tag(tag_map(&[("One", "1"), ("Two", "2"), ("Three", "3")])),
            served_status: 200,
            served_airflow_status: None,
            changes_state: true,
        },
        Row {
            case: "UntagResource removes a key the environment carries",
            write: Write::Untag(keys(&[SEEDED_TAGS[0].0])),
            served_status: 200,
            served_airflow_status: None,
            changes_state: true,
        },
        Row {
            case: "UntagResource removes every key it carries",
            write: Write::Untag(keys(&[SEEDED_TAGS[0].0, SEEDED_TAGS[1].0])),
            served_status: 200,
            served_airflow_status: None,
            changes_state: true,
        },
        Row {
            case: "UntagResource removes a key it does not carry",
            write: Write::Untag(keys(&["NeverStored"])),
            served_status: 200,
            served_airflow_status: None,
            changes_state: false,
        },
        Row {
            case: "InvokeRestApi POST /variables creates a variable",
            write: Write::Airflow(call_with_body(
                "POST",
                "/variables",
                json!({"key": "retries", "value": "3"}),
            )),
            served_status: 200,
            served_airflow_status: Some(200),
            changes_state: true,
        },
        Row {
            case: "InvokeRestApi POST /variables replaces the seeded one",
            write: Write::Airflow(call_with_body(
                "POST",
                "/variables",
                json!({"key": SEEDED_VARIABLE, "value": "@hourly"}),
            )),
            served_status: 200,
            served_airflow_status: Some(200),
            changes_state: true,
        },
        Row {
            case: "InvokeRestApi PATCH /variables/{key} changes the seeded one",
            write: Write::Airflow(call_with_body(
                "PATCH",
                &format!("/variables/{SEEDED_VARIABLE}"),
                json!({"key": SEEDED_VARIABLE, "value": "@weekly"}),
            )),
            served_status: 200,
            served_airflow_status: Some(200),
            changes_state: true,
        },
        Row {
            case: "InvokeRestApi DELETE /variables/{key} removes the seeded one",
            write: Write::Airflow(call("DELETE", &format!("/variables/{SEEDED_VARIABLE}"))),
            served_status: 200,
            served_airflow_status: Some(204),
            changes_state: true,
        },
        Row {
            case: "InvokeRestApi POST /pools creates a slot pool",
            write: Write::Airflow(call_with_body(
                "POST",
                "/pools",
                json!({"name": "reporting", "slots": 3}),
            )),
            served_status: 200,
            served_airflow_status: Some(200),
            changes_state: true,
        },
        Row {
            case: "InvokeRestApi PATCH /pools/{name} resizes the seeded pool",
            write: Write::Airflow(call_with_body(
                "PATCH",
                &format!("/pools/{SEEDED_POOL}"),
                json!({"name": SEEDED_POOL, "slots": 9}),
            )),
            served_status: 200,
            served_airflow_status: Some(200),
            changes_state: true,
        },
        Row {
            case: "InvokeRestApi DELETE /pools/{name} removes the seeded pool",
            write: Write::Airflow(call("DELETE", &format!("/pools/{SEEDED_POOL}"))),
            served_status: 200,
            served_airflow_status: Some(204),
            changes_state: true,
        },
        Row {
            // A method the Apache Airflow REST API serves no arm for on that path is still
            // a write: `RestApiMethod` admits it, and only a `GET` reports rather than
            // changes. Against an environment that can take a change the API itself answers
            // the refusal (405, carried by `RestApiClientException`); against one being torn
            // down the change never reaches it.
            case: "InvokeRestApi PUT /variables, a method that API serves no arm for",
            write: Write::Airflow(call_with_body(
                "PUT",
                "/variables",
                json!({"key": "retries", "value": "3"}),
            )),
            served_status: 400,
            served_airflow_status: Some(405),
            changes_state: false,
        },
    ]
}

#[tokio::test]
async fn every_writer_refuses_a_change_to_an_environment_being_torn_down() {
    for row in rows() {
        let case = row.case;
        let name = "TeardownEnv";
        let harness = Harness::seeded(case, name).await;
        let before = harness.snapshot(case, name, &environment_arn(name)).await;

        assert_eq!(
            harness.delete(name).await.status,
            200,
            "case {case:?}: the delete that starts the teardown must succeed"
        );
        let status = harness.status_of(case, name).await;
        assert_eq!(
            status,
            state::STATUS_DELETING,
            "case {case:?}: the write must arrive while the teardown is running, got {status}"
        );

        let refused = row.send(&harness, name).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected the write to be refused with 400, got {} {}",
            refused.status,
            refused.text()
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.message(),
            state::change_not_allowed_message(name, state::STATUS_DELETING),
            "case {case:?}: the refusal must name the environment and its status, got {:?}",
            refused.message()
        );

        // Nothing was applied, and every reader still reports the record the refusal
        // named — the teardown has started, it has not finished.
        let after = harness.snapshot(case, name, &environment_arn(name)).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused write must leave the record untouched, got {after:?}"
        );
    }
}

#[tokio::test]
async fn the_same_write_is_served_while_the_environment_can_take_the_change() {
    for row in rows() {
        let case = row.case;
        let name = "SettledEnv";
        let harness = Harness::seeded(case, name).await;
        let before = harness.snapshot(case, name, &environment_arn(name)).await;

        let answer = row.send(&harness, name).await;
        assert_eq!(
            answer.status,
            row.served_status,
            "case {case:?}: expected HTTP {} from an environment that can take the change, \
             got {} {}",
            row.served_status,
            answer.status,
            answer.text()
        );
        assert_ne!(
            answer.message(),
            state::change_not_allowed_message(name, state::STATUS_DELETING),
            "case {case:?}: a settled environment must not answer the teardown refusal"
        );
        if let Some(expected) = row.served_airflow_status {
            assert_eq!(
                answer.airflow_status(),
                expected,
                "case {case:?}: expected RestApiStatusCode {expected}, got {}",
                answer.text()
            );
        }

        let after = harness.snapshot(case, name, &environment_arn(name)).await;
        if row.changes_state {
            assert_ne!(
                after, before,
                "case {case:?}: a served write must be visible to the next read, got {after:?}"
            );
        } else {
            assert_eq!(
                after, before,
                "case {case:?}: this write asks for the state already stored, got {after:?}"
            );
        }
    }
}
