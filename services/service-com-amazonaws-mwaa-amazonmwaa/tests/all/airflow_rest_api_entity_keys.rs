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
//! A key no path could name is one of two things: a key the split a path is read through
//! carries no segment for (an empty one), and a key so long that every path naming it is
//! outside the modelled `Path` bound. Both are refused at the write.
//!
//! Rows differ along how many separators a key carries, where they sit, the characters
//! around them, the key's length against the bound a path can carry, the entity kind and
//! the method applied to it; a key with no separator at all is the control row.

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

/// The upper bound of this operation's modelled path member — `RestApiPath`
/// `@length(1, 64)`, from
/// `smithy_query(query="shape",shape="RestApiPath",model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")`
/// — counted in characters, as `@length` on a string is. It is the longest `Path` any call
/// can carry, and so the longest path an entity stored here can be addressed through again.
const LONGEST_PATH: usize = 64;

/// The longest key the collection hanging under `/{segment}` can hold and still be named by
/// a path: what is left of [`LONGEST_PATH`] once `/{segment}/` has taken its share — 53
/// characters under `/variables/`, 57 under `/pools/`.
fn longest_key(segment: &str) -> usize {
    LONGEST_PATH - format!("/{segment}/").chars().count()
}

/// One collection this API addresses an entity of by key: the segment it hangs under (so
/// how long a key a path can name under it), the member its entity's identity travels in,
/// and how a write that creates or changes one is spelled.
struct KeyedCollection {
    segment: &'static str,
    identity: &'static str,
    write: fn(&str) -> Value,
    change: fn(&str) -> Value,
}

/// Both collections whose entities are keyed, so the rule is asserted over the family
/// rather than over one of them: they differ in the length of their own path segment, in
/// the member carrying the key and in the body their writes take.
fn keyed_collections() -> Vec<KeyedCollection> {
    vec![
        KeyedCollection {
            segment: "variables",
            identity: "key",
            write: |key| json!({"key": key, "value": "first"}),
            change: |key| json!({"key": key, "value": "second"}),
        },
        KeyedCollection {
            segment: "pools",
            identity: "name",
            write: |name| json!({"name": name, "slots": 3}),
            change: |name| json!({"name": name, "slots": 5}),
        },
    ]
}

/// A key of exactly `length` characters carrying the path separator between its segments:
/// one-character segments joined by it. An even `length` would otherwise close with the
/// separator, which is the OTHER unaddressable shape, so the last character is always a
/// segment's.
fn separated_key(length: usize) -> String {
    let mut key: String = (0..length)
        .map(|index| if index % 2 == 1 { '/' } else { 'k' })
        .collect();
    if key.ends_with('/') {
        key.pop();
        key.push('k');
    }
    key
}

/// A key at the longest a path can name is stored AND reachable: read, changed and deleted
/// through the path that names it, whatever characters it is built from.
///
/// Rows differ along the collection (so along the ceiling itself), and along what the key
/// at that ceiling is made of: one segment, separators between many segments, and
/// characters outside ASCII (the bound counts characters, not bytes).
#[tokio::test]
async fn a_key_at_the_longest_a_path_can_name_is_stored_and_addressable() {
    let harness = Harness::with_environment().await;

    for collection in keyed_collections() {
        let longest = longest_key(collection.segment);
        for (shape, key) in [
            ("one segment", "k".repeat(longest)),
            ("separators between segments", separated_key(longest)),
            ("characters outside ASCII", "ü".repeat(longest)),
        ] {
            let case = format!("{} under /{} at {longest}", shape, collection.segment);
            let path = format!("/{}/{key}", collection.segment);
            assert!(
                path.chars().count() <= LONGEST_PATH,
                "case {case:?}: the path naming the key must be one a call can carry, got \
                 {} characters",
                path.chars().count()
            );

            let created = harness
                .invoke(call_with_body(
                    "POST",
                    &format!("/{}", collection.segment),
                    (collection.write)(&key),
                ))
                .await;
            assert_eq!(
                created.airflow_status(),
                200,
                "case {case:?}: the write must be served, got {}",
                created.text()
            );
            assert_eq!(
                created.member(collection.identity),
                json!(key),
                "case {case:?}: the stored entity must carry the key written, got {}",
                created.document()
            );

            let read = harness.invoke(call("GET", &path)).await;
            assert_eq!(
                read.airflow_status(),
                200,
                "case {case:?}: GET {path} must read the stored entity, got {}",
                read.text()
            );

            let changed = harness
                .invoke(call_with_body("PATCH", &path, (collection.change)(&key)))
                .await;
            assert_eq!(
                changed.airflow_status(),
                200,
                "case {case:?}: PATCH must change the stored entity, got {}",
                changed.text()
            );
            let reread = harness.invoke(call("GET", &path)).await;
            assert_eq!(
                reread.document(),
                changed.document(),
                "case {case:?}: the change must be visible to the next read, got {}",
                reread.document()
            );

            let deleted = harness.invoke(call("DELETE", &path)).await;
            assert_eq!(
                deleted.airflow_status(),
                204,
                "case {case:?}: DELETE must remove the stored entity, got {}",
                deleted.text()
            );
            let gone = harness.invoke(call("GET", &path)).await;
            assert_eq!(
                gone.airflow_status(),
                404,
                "case {case:?}: the deleted entity must be gone, got {}",
                gone.text()
            );
        }
    }
}

/// A key longer than any path could name is refused at the write — as a client error of
/// the Apache Airflow call, naming the length it was held to — and nothing is stored, so no
/// entity the API could never reach again occupies a slot of the collection's budget.
///
/// Rows differ along the collection, along how far past the ceiling the key reaches (one
/// character, the store's own key ceiling, past that ceiling too) and along what it is made
/// of (one segment, separators, characters outside ASCII).
#[tokio::test]
async fn a_key_longer_than_any_path_could_name_is_refused_at_the_write() {
    let harness = Harness::with_environment().await;

    for collection in keyed_collections() {
        let longest = longest_key(collection.segment);
        let listing = call("GET", &format!("/{}", collection.segment));
        let before = harness.invoke(listing.clone()).await.member("total_entries");

        for (shape, key) in [
            ("one character past the ceiling", "k".repeat(longest + 1)),
            (
                "separators between segments, one character past it",
                separated_key(longest + 1),
            ),
            (
                "characters outside ASCII, one character past it",
                "ü".repeat(longest + 1),
            ),
            (
                // The store's own key ceiling, which a path cannot reach any part of.
                "the longest key this store would hold",
                "k".repeat(state::MAX_AIRFLOW_KEY_LENGTH),
            ),
            (
                "past the store's own ceiling too",
                "k".repeat(state::MAX_AIRFLOW_KEY_LENGTH + 1),
            ),
        ] {
            let length = key.chars().count();
            let case = format!(
                "{shape} under /{} ({length} characters, ceiling {longest})",
                collection.segment
            );

            // The path that would name such an entity is one no call can carry: the
            // boundary refuses it on the modelled bound, which is why the write must not
            // store it.
            let addressed = harness
                .invoke(call("GET", &format!("/{}/{key}", collection.segment)))
                .await;
            assert_eq!(
                addressed.error_code(),
                "ValidationException",
                "case {case:?}: the path naming this key must be outside the modelled \
                 bound, got {} {}",
                addressed.status,
                addressed.text()
            );

            let refused = harness
                .invoke(call_with_body(
                    "POST",
                    &format!("/{}", collection.segment),
                    (collection.write)(&key),
                ))
                .await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: a refused call is the modelled exception's own HTTP 400, \
                 got {} {}",
                refused.status,
                refused.text()
            );
            assert_eq!(
                refused.error_code(),
                "RestApiClientException",
                "case {case:?}: expected RestApiClientException, got {}",
                refused.text()
            );
            assert_eq!(
                refused.airflow_status(),
                400,
                "case {case:?}: expected RestApiStatusCode 400, got {}",
                refused.text()
            );
            let document = refused.document();
            let detail = document
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                detail.contains(&format!("of {length} characters"))
                    && detail.contains(&format!("at most {longest} characters")),
                "case {case:?}: the detail must state the length refused and the length a \
                 path can name, got {detail:?}"
            );
        }

        let after = harness.invoke(listing).await.member("total_entries");
        assert_eq!(
            after, before,
            "case {:?}: a refused write must store nothing, expected {before} entries, got \
             {after}",
            collection.segment
        );
    }
}
