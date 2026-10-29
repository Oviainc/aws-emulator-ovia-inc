// SPDX-License-Identifier: CC0-1.0

//! How `InvokeRestApi` ADDRESSES one entity of an Apache Airflow collection, end to end
//! over the router this service mounts in production.
//!
//! The rule under test: neither `/variables` nor `/pools` holds a sub-resource, so the
//! entity a path addresses is named by the whole remainder of that path after the
//! collection's own segment — a key that itself carries the path separator addresses the
//! entity stored under it — and a write refuses a key no such path could name, so every
//! entity the API stores can be read, changed and deleted through it again.
//!
//! Rows differ along how many separators a key carries, where they sit, the characters
//! around them, the entity kind and the method applied to it; a key with no separator at
//! all is the control row.

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

    /// One member of the answered document.
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

    /// One Apache Airflow REST API call against the environment these suites use.
    async fn invoke(&self, body: Value) -> Answer {
        self.send("POST", &format!("/restapi/{ENVIRONMENT}"), Some(body))
            .await
    }

    /// The environment every suite here invokes, every optional member absent.
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

/// The environment these suites invoke the Apache Airflow REST API of.
const ENVIRONMENT: &str = "KeyAddressing";

/// One Apache Airflow REST API call as a table row states it.
fn call(method: &str, path: &str) -> Value {
    json!({"Method": method, "Path": path})
}

/// The same, carrying a body.
fn call_with_body(method: &str, path: &str, body: Value) -> Value {
    json!({"Method": method, "Path": path, "Body": body})
}

/// The keys the addressing rule generalises over: a key with no separator, keys with the
/// separator in every position the grammar admits, and one key per segment count, so the
/// rule is asserted over the family rather than over one shape of key.
///
/// Every key here keeps `/variables/{key}` and `/pools/{name}` inside the modelled
/// `RestApiPath` bound (`@length(1, 64)`), which is what a path can carry at all.
fn addressable_keys() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = vec![
        ("no separator at all", "schedule"),
        ("one separator", "team/schedule"),
        ("two separators", "team/etl/schedule"),
        (
            "a separator beside the other characters a key carries",
            "data_lake.bucket-name/v1",
        ),
        ("a separator between characters outside ASCII", "ünï/codë"),
        ("a separator around a digit", "v1/2/3"),
    ]
    .into_iter()
    .map(|(case, key)| (case.to_string(), key.to_string()))
    .collect();
    for segments in 1..=6 {
        let key: Vec<String> = (0..segments).map(|index| format!("s{index}")).collect();
        rows.push((format!("a key of {segments} segments"), key.join("/")));
    }
    rows
}

/// The keys no path of this API could name again: the split a path is read through carries
/// no empty segment, so a key with one is refused at the write instead of being stored
/// where nothing could reach it.
fn unaddressable_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a key opening with the separator", "/leading"),
        ("a key closing with the separator", "trailing/"),
        ("a key carrying two separators in a row", "two//inside"),
        ("a key that is the separator", "/"),
        ("a key of nothing but separators", "//"),
        ("a doubled separator after a good segment", "a/b//c"),
    ]
}

/// A variable whose key carries the path separator is read, changed and deleted through
/// the very path that stored it, and deleting it frees the slot it held.
#[tokio::test]
async fn a_variable_key_carrying_the_separator_is_addressable() {
    let harness = Harness::with_environment().await;

    for (case, key) in addressable_keys() {
        let path = format!("/variables/{key}");

        let created = harness
            .invoke(call_with_body(
                "POST",
                "/variables",
                json!({"key": key, "value": "first"}),
            ))
            .await;
        assert_eq!(
            created.airflow_status(),
            200,
            "case {case:?}: the write of {key:?} must be served, got {}",
            created.text()
        );
        assert_eq!(
            created.member("key"),
            json!(key),
            "case {case:?}: the stored variable must carry the key written, got {}",
            created.document()
        );

        let read = harness.invoke(call("GET", &path)).await;
        assert_eq!(
            read.airflow_status(),
            200,
            "case {case:?}: GET {path} must read the stored variable, got {}",
            read.text()
        );
        assert_eq!(
            read.member("value"),
            json!("first"),
            "case {case:?}: expected the stored value \"first\", got {}",
            read.document()
        );

        let changed = harness
            .invoke(call_with_body(
                "PATCH",
                &path,
                json!({"key": key, "value": "second"}),
            ))
            .await;
        assert_eq!(
            changed.airflow_status(),
            200,
            "case {case:?}: PATCH {path} must change the stored variable, got {}",
            changed.text()
        );
        assert_eq!(
            changed.member("value"),
            json!("second"),
            "case {case:?}: expected the changed value \"second\", got {}",
            changed.document()
        );
        let reread = harness.invoke(call("GET", &path)).await;
        assert_eq!(
            reread.member("value"),
            json!("second"),
            "case {case:?}: the change must be visible to the next read, got {}",
            reread.document()
        );

        let deleted = harness.invoke(call("DELETE", &path)).await;
        assert_eq!(
            deleted.airflow_status(),
            204,
            "case {case:?}: DELETE {path} must remove the stored variable, got {}",
            deleted.text()
        );
        let gone = harness.invoke(call("GET", &path)).await;
        assert_eq!(
            gone.airflow_status(),
            404,
            "case {case:?}: the deleted variable must be gone, got {}",
            gone.text()
        );
    }

    // Nothing is left behind, so no slot of the store's own budget is held by an entity
    // the API could not reach.
    let listed = harness.invoke(call("GET", "/variables")).await;
    assert_eq!(
        listed.member("total_entries"),
        json!(0),
        "every variable written here was deleted again, got {}",
        listed.document()
    );
}

/// The same for a pool, whose name is its identity: a name carrying the separator is
/// addressable, and a delete through that path frees the name for a later create.
#[tokio::test]
async fn a_pool_name_carrying_the_separator_is_addressable() {
    let harness = Harness::with_environment().await;

    for (case, name) in addressable_keys() {
        let path = format!("/pools/{name}");

        let created = harness
            .invoke(call_with_body(
                "POST",
                "/pools",
                json!({"name": name, "slots": 3}),
            ))
            .await;
        assert_eq!(
            created.airflow_status(),
            200,
            "case {case:?}: the create of pool {name:?} must be served, got {}",
            created.text()
        );

        let read = harness.invoke(call("GET", &path)).await;
        assert_eq!(
            read.airflow_status(),
            200,
            "case {case:?}: GET {path} must read the stored pool, got {}",
            read.text()
        );
        assert_eq!(
            read.member("slots"),
            json!(3),
            "case {case:?}: expected the 3 slots stored, got {}",
            read.document()
        );

        let changed = harness
            .invoke(call_with_body(
                "PATCH",
                &path,
                json!({"name": name, "slots": 5}),
            ))
            .await;
        assert_eq!(
            changed.airflow_status(),
            200,
            "case {case:?}: PATCH {path} must change the stored pool, got {}",
            changed.text()
        );
        assert_eq!(
            changed.member("open_slots"),
            json!(5),
            "case {case:?}: the open slots must follow the slots stored, got {}",
            changed.document()
        );

        let deleted = harness.invoke(call("DELETE", &path)).await;
        assert_eq!(
            deleted.airflow_status(),
            204,
            "case {case:?}: DELETE {path} must remove the stored pool, got {}",
            deleted.text()
        );
        let gone = harness.invoke(call("GET", &path)).await;
        assert_eq!(
            gone.airflow_status(),
            404,
            "case {case:?}: the deleted pool must be gone, got {}",
            gone.text()
        );

        // A pool's name is its identity and a taken name is a conflict, so the name being
        // free again is what proves the delete reached the stored pool.
        let again = harness
            .invoke(call_with_body(
                "POST",
                "/pools",
                json!({"name": name, "slots": 1}),
            ))
            .await;
        assert_eq!(
            again.airflow_status(),
            200,
            "case {case:?}: the deleted name must be free to create again, got {}",
            again.text()
        );
        let removed = harness.invoke(call("DELETE", &path)).await;
        assert_eq!(
            removed.airflow_status(),
            204,
            "case {case:?}: the pool created again must delete again, got {}",
            removed.text()
        );
    }

    // What remains is the pool Apache Airflow seeds a metadata database with, and nothing
    // else.
    let listed = harness.invoke(call("GET", "/pools")).await;
    assert_eq!(
        listed.member("total_entries"),
        json!(1),
        "only {} may remain, got {}",
        state::DEFAULT_POOL_NAME,
        listed.document()
    );
}

/// A key no path of this API could name again is refused at the write, as a client error
/// of the Apache Airflow call, and nothing is stored.
#[tokio::test]
async fn a_key_no_path_can_address_is_refused_at_the_write() {
    let harness = Harness::with_environment().await;

    for (case, key) in unaddressable_keys() {
        for (entity, body) in [
            (
                "a variable",
                call_with_body("POST", "/variables", json!({"key": key, "value": "v"})),
            ),
            (
                "a pool",
                call_with_body("POST", "/pools", json!({"name": key, "slots": 1})),
            ),
        ] {
            let answer = harness.invoke(body).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} ({entity}): a refused call is the modelled exception's own \
                 HTTP 400, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.error_code(),
                "RestApiClientException",
                "case {case:?} ({entity}): expected RestApiClientException, got {}",
                answer.text()
            );
            assert_eq!(
                answer.airflow_status(),
                400,
                "case {case:?} ({entity}): expected RestApiStatusCode 400, got {}",
                answer.text()
            );
            assert!(
                answer
                    .member("detail")
                    .as_str()
                    .is_some_and(|detail| detail.contains(key)),
                "case {case:?} ({entity}): the detail must name the key refused, got {}",
                answer.document()
            );
        }
    }

    // None of those refusals stored anything: the variables are still empty and the pools
    // still hold only the pool Apache Airflow seeds.
    for (collection, path, expected) in [("variables", "/variables", 0), ("pools", "/pools", 1)] {
        let listed = harness.invoke(call("GET", path)).await;
        assert_eq!(
            listed.member("total_entries"),
            json!(expected),
            "case {collection:?}: a refused write must store nothing, got {}",
            listed.document()
        );
    }
}
