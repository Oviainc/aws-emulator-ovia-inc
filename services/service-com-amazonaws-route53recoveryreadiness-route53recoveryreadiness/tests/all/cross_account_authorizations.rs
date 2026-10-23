// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the cross-account readiness authorizations: the set
//! `CreateCrossAccountAuthorization` adds to, `ListCrossAccountAuthorizations` reports
//! and `DeleteCrossAccountAuthorization` withdraws from.
//!
//! Every assertion goes over the wire: a request is built, dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts, and only the HTTP status, the
//! `x-amzn-errortype` header and the JSON body are inspected. Nothing reaches into the
//! service's state struct, so a response that is not computed from persisted state
//! cannot pass.
//!
//! The tests are tables. The rule under test is that an authorization is SET MEMBERSHIP
//! keyed by its own value: a create adds the value and reports it back, a list reports
//! exactly the values held (in value order, paged), and a delete removes the named value
//! and answers the same empty HTTP 200 whether or not it was held — the model declares no
//! `ResourceNotFoundException` for the delete. The rows differ along what that rule
//! generalises over: the value's form and length, how many values are stored, which one
//! is addressed, and the page size asked for. The "delete a value that was never
//! created" case the issue recorded is one row.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::state::{
    DEFAULT_MAX_RESULTS, MAX_RECORDS_PER_STORE,
};
use service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `MaxResults` is `range 1..1000` on this list operation (model:
/// `smithy_query(query="operation", operation="ListCrossAccountAuthorizations")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// Authorization values this table ranges over.
///
/// `CrossAccountAuthorization` is a plain string with neither `@length` nor `@pattern`
/// (model: `smithy_query(query="shape", shape="CrossAccountAuthorization")`), so the
/// rows vary form freely: a bare account id, the account-root and role ARNs AWS's
/// cross-account documentation uses (the role ARN carries a `/`, which the delete's
/// `@httpLabel` has to survive percent-encoded), a single character, a value far longer
/// than any name this service mints, and a non-ASCII value. The issue's recorded case —
/// deleting a value that was never created — is exercised against every one of these in
/// [`deleting_an_authorization_the_store_does_not_hold_succeeds_idempotently`].
fn authorization_cases() -> Vec<String> {
    vec![
        "111122223333".to_string(),
        "arn:aws:iam::111122223333:root".to_string(),
        "arn:aws:iam::444455556666:role/ReadinessReader".to_string(),
        "0".to_string(),
        format!("arn:aws:iam::777788889999:role/{}", "deep_path/".repeat(20)),
        "авторизация".to_string(),
    ]
}

/// One wire answer, reduced to what a client branches on. `pub(crate)` so the
/// concurrency suite in the sibling module drives the same harness rather than carrying
/// a second copy of it.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    error_type: String,
    body: Value,
}

impl Reply {
    fn field(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|v| !v.is_null())
    }

    pub(crate) fn string(&self, name: &str) -> Option<String> {
        self.field(name).and_then(Value::as_str).map(str::to_string)
    }

    /// The error shape this answer names, from the `x-amzn-errortype` header, falling
    /// back to the body's `__type`.
    fn error_code(&self) -> String {
        let raw = if self.error_type.is_empty() {
            self.string("__type").unwrap_or_default()
        } else {
            self.error_type.clone()
        };
        raw.rsplit(['#', '/']).next().unwrap_or_default().to_string()
    }
}

/// Percent-encode a value for the delete's non-greedy `@httpLabel`
/// (`DELETE /crossaccountauthorizations/{CrossAccountAuthorization}`): an AWS SDK
/// encodes every byte outside RFC 3986's unreserved set, `/` included.
fn encoded_label(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router. `pub(crate)` for the same reason as [`Reply`].
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/route53-recovery-readiness/aws4_request, \
             SignedHeaders=host, Signature=deadbeef";
        let builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "route53-recovery-readiness.us-east-1.amazonaws.com")
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize body"),
                ))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    pub(crate) async fn create(&self, authorization: &str) -> Reply {
        self.send(
            "POST",
            "/crossaccountauthorizations",
            Some(json!({ "crossAccountAuthorization": authorization })),
        )
        .await
    }

    async fn delete(&self, authorization: &str) -> Reply {
        self.send(
            "DELETE",
            &format!(
                "/crossaccountauthorizations/{}",
                encoded_label(authorization)
            ),
            None,
        )
        .await
    }

    async fn list(&self, query: &str) -> Reply {
        let uri = if query.is_empty() {
            "/crossaccountauthorizations".to_string()
        } else {
            format!("/crossaccountauthorizations?{query}")
        };
        self.send("GET", &uri, None).await
    }

    /// Walk every page the list offers and return the authorizations it reported, in the
    /// order it reported them, asserting each page honours `maxResults` and that only
    /// the last page omits `nextToken`.
    async fn drain_pages(&self, max_results: Option<i64>, label: &str) -> Vec<String> {
        let mut items: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // More rounds than there can be pages, so a non-terminating cursor fails loudly
        // instead of looping forever.
        for _ in 0..64 {
            let mut query = Vec::new();
            if let Some(max) = max_results {
                query.push(format!("maxResults={max}"));
            }
            if let Some(cursor) = &token {
                query.push(format!("nextToken={cursor}"));
            }
            let page = self.list(&query.join("&")).await;
            assert_eq!(
                page.status, 200,
                "case {label:?}: expected a page to answer HTTP 200, got {} / {}",
                page.status, page.body
            );
            let reported: Vec<String> = page
                .field("crossAccountAuthorizations")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .map(|value| {
                            value
                                .as_str()
                                .unwrap_or_else(|| {
                                    panic!("case {label:?}: expected a string, got {value}")
                                })
                                .to_string()
                        })
                        .collect()
                })
                .unwrap_or_else(|| {
                    panic!(
                        "case {label:?}: expected a crossAccountAuthorizations array, got {}",
                        page.body
                    )
                });
            if let Some(max) = max_results {
                assert!(
                    reported.len() as i64 <= max,
                    "case {label:?}: expected at most {max} items on a page, got {}",
                    reported.len()
                );
            }
            items.extend(reported.iter().cloned());
            match page.string("nextToken") {
                Some(next) => {
                    assert_eq!(
                        reported.len() as i64,
                        max_results.unwrap_or(DEFAULT_MAX_RESULTS as i64),
                        "case {label:?}: a page that carries nextToken must be full, got {} items",
                        reported.len()
                    );
                    token = Some(next);
                }
                None => return items,
            }
        }
        panic!("case {label:?}: the cursor never terminated after 64 pages");
    }

    /// Every stored authorization, over as many pages as the service needs.
    pub(crate) async fn authorizations(&self, label: &str) -> Vec<String> {
        self.drain_pages(None, label).await
    }
}

/// The values in the order a set reports them.
fn in_set_order<I: IntoIterator<Item = String>>(values: I) -> Vec<String> {
    values.into_iter().collect::<BTreeSet<String>>().into_iter().collect()
}

#[tokio::test]
async fn a_created_authorization_is_reported_back_and_listed_exactly_once() {
    let emulator = Emulator::new();
    let mut created_so_far: Vec<String> = Vec::new();

    for authorization in authorization_cases() {
        let label = authorization.clone();
        let created = emulator.create(&authorization).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from the create, got {} / {}",
            created.status, created.body
        );
        assert_eq!(
            created.string("crossAccountAuthorization").as_deref(),
            Some(authorization.as_str()),
            "case {label:?}: expected the create to report the stored authorization, got {:?}",
            created.string("crossAccountAuthorization")
        );

        created_so_far.push(authorization.clone());
        assert_eq!(
            emulator.authorizations(&label).await,
            in_set_order(created_so_far.clone()),
            "case {label:?}: expected the list to report every authorization created so far"
        );
    }
}

#[tokio::test]
async fn creating_an_authorization_already_held_reports_it_without_storing_it_twice() {
    let emulator = Emulator::new();

    for authorization in authorization_cases() {
        let label = authorization.clone();
        for attempt in 1..=3 {
            let created = emulator.create(&authorization).await;
            assert_eq!(
                created.status, 200,
                "case {label:?} attempt {attempt}: expected HTTP 200, got {} / {}",
                created.status, created.body
            );
            assert_eq!(
                created.string("crossAccountAuthorization").as_deref(),
                Some(authorization.as_str()),
                "case {label:?} attempt {attempt}: expected the same value back, got {:?}",
                created.string("crossAccountAuthorization")
            );
        }

        let listed = emulator.authorizations(&label).await;
        let occurrences = listed.iter().filter(|value| **value == authorization).count();
        assert_eq!(
            occurrences, 1,
            "case {label:?}: expected the value stored exactly once, got {occurrences} in {listed:?}"
        );
    }
}

#[tokio::test]
async fn delete_withdraws_the_named_authorization_and_only_that_one() {
    let cases = authorization_cases();

    // One row per authorization deleted out of the full stored set, so the rule is
    // checked on every form rather than on one of them.
    for deleted in &cases {
        let emulator = Emulator::new();
        for authorization in &cases {
            let created = emulator.create(authorization).await;
            assert_eq!(
                created.status, 200,
                "case {deleted:?}: expected the setup create of {authorization:?} to succeed, got {} / {}",
                created.status, created.body
            );
        }

        let reply = emulator.delete(deleted).await;
        assert_eq!(
            reply.status, 200,
            "case {deleted:?}: expected HTTP 200 from the delete, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "",
            "case {deleted:?}: expected no error from the delete, got {:?} / {}",
            reply.error_code(),
            reply.body
        );

        let expected = in_set_order(
            cases
                .iter()
                .filter(|value| *value != deleted)
                .cloned()
                .collect::<Vec<String>>(),
        );
        assert_eq!(
            emulator.authorizations(deleted).await,
            expected,
            "case {deleted:?}: expected every other authorization to survive the delete"
        );
    }
}

#[tokio::test]
async fn deleting_an_authorization_the_store_does_not_hold_succeeds_idempotently() {
    // The model declares no `ResourceNotFoundException` for this operation, so a value
    // the store does not hold is answered exactly like one it did hold. The rows cover
    // both ways a value can be absent — never created, and created then deleted — over
    // an empty store and a store holding neighbours, which is the issue's recorded case
    // (`delete-cross-account-authorization-nonexistent-succeeds-idempotently`) plus the
    // forms it did not record.
    for neighbours in [Vec::new(), authorization_cases()] {
        let emulator = Emulator::new();
        for neighbour in &neighbours {
            emulator.create(neighbour).await;
        }
        let before = emulator.authorizations("setup").await;

        for authorization in authorization_cases() {
            let label = format!("{authorization} with {} neighbour(s)", neighbours.len());

            // Absent because it was created and then deleted, or (when it is not a
            // neighbour) because it was never created at all.
            for attempt in 1..=3 {
                let reply = emulator.delete(&authorization).await;
                assert_eq!(
                    reply.status, 200,
                    "case {label:?} attempt {attempt}: expected HTTP 200, got {} / {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.error_code(),
                    "",
                    "case {label:?} attempt {attempt}: expected no error, got {:?} / {}",
                    reply.error_code(),
                    reply.body
                );
            }
        }

        // Every value the table names is gone, and nothing else moved.
        let expected = in_set_order(
            before
                .iter()
                .filter(|value| !authorization_cases().contains(value))
                .cloned()
                .collect::<Vec<String>>(),
        );
        assert_eq!(
            emulator.authorizations("after").await,
            expected,
            "case {neighbours:?}: an idempotent delete must leave the rest of the store alone"
        );
    }
}

#[tokio::test]
async fn pagination_reports_every_stored_authorization_exactly_once() {
    // An exhaustive sweep of the bounded domain this rule generalises over: how many
    // authorizations are stored crossed with how large a page is asked for, including
    // both published `MaxResults` bounds and no `MaxResults` at all.
    for stored in 0usize..=8 {
        let emulator = Emulator::new();
        let values: Vec<String> = (0..stored)
            .map(|index| format!("arn:aws:iam::{:012}:root", index * 7 + 1))
            .collect();
        for value in &values {
            emulator.create(value).await;
        }

        for max_results in [
            Some(MAX_RESULTS_MIN),
            Some(2),
            Some(3),
            Some(MAX_RESULTS_MAX),
            None,
        ] {
            let label = format!("{stored} stored, maxResults {max_results:?}");
            assert_eq!(
                emulator.drain_pages(max_results, &label).await,
                in_set_order(values.clone()),
                "case {label:?}: expected every stored authorization exactly once, in value order"
            );
        }
    }
}

#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    emulator.create("111122223333").await;

    // One past each published bound of `MaxResults` (range 1..1000).
    for max in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let refused = emulator.list(&format!("maxResults={max}")).await;
        assert_eq!(
            refused.status, 400,
            "case {max}: expected HTTP 400 for a maxResults outside the modelled range, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {max}: expected ValidationException, got {:?} / {}",
            refused.error_code(),
            refused.body
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let emulator = Emulator::new();
    for value in ["111122223333", "444455556666"] {
        emulator.create(value).await;
    }

    // Rows differ in how the token fails to be one this service minted: not hex, hex
    // that is not text, hex text without the cursor tag, and empty.
    for token in ["not-a-token", "ff", "3131313132323232", ""] {
        let refused = emulator.list(&format!("nextToken={token}")).await;
        assert_eq!(
            refused.status, 400,
            "case {token:?}: expected HTTP 400 for a token this service did not mint, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {:?} / {}",
            refused.error_code(),
            refused.body
        );
    }
}

#[tokio::test]
async fn an_empty_authorization_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let refused = emulator.create("").await;
    assert_eq!(
        refused.status, 400,
        "case empty: expected HTTP 400 for an empty authorization, got {} / {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case empty: expected ValidationException, got {:?} / {}",
        refused.error_code(),
        refused.body
    );
    assert_eq!(
        emulator.authorizations("empty").await,
        Vec::<String>::new(),
        "case empty: a refused create must store nothing"
    );
}

#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    for value in authorization_cases() {
        emulator.create(&value).await;
    }

    let first = emulator.authorizations("first read").await;
    for round in 1..=3 {
        assert_eq!(
            emulator.authorizations("repeat read").await,
            first,
            "round {round}: repeated reads must not change what the store reports"
        );
    }
}

#[tokio::test]
async fn a_store_at_its_ceiling_refuses_another_authorization() {
    // The emulator keeps at most MAX_RECORDS_PER_STORE records per store, so a caller
    // varying the authorization value cannot grow the process without bound.
    let emulator = Emulator::new();
    for index in 0..MAX_RECORDS_PER_STORE {
        let created = emulator
            .create(&format!("arn:aws:iam::{index:012}:root"))
            .await;
        assert_eq!(
            created.status, 200,
            "record {index}: expected HTTP 200 below the ceiling, got {} / {}",
            created.status, created.body
        );
    }

    let refused = emulator.create("one:too:many").await;
    assert_eq!(
        refused.status, 400,
        "a store at {MAX_RECORDS_PER_STORE} records must refuse another, got {} / {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "expected ValidationException, got {:?} / {}",
        refused.error_code(),
        refused.body
    );
    let message = refused.string("message").unwrap_or_default();
    assert!(
        message.contains(&MAX_RECORDS_PER_STORE.to_string()),
        "expected the refusal to name the ceiling, got {message:?}"
    );

    // Re-authorizing a value the full store already holds is still a no-op, not a
    // refusal: it stores nothing new.
    let repeat = emulator.create("arn:aws:iam::000000000000:root").await;
    assert_eq!(
        repeat.status, 200,
        "a full store must still accept a value it already holds, got {} / {}",
        repeat.status, repeat.body
    );

    // And a delete frees room for one more.
    assert_eq!(
        emulator.delete("arn:aws:iam::000000000000:root").await.status,
        200,
        "expected the delete to answer HTTP 200"
    );
    assert_eq!(
        emulator.create("one:too:many").await.status,
        200,
        "expected room for one more authorization after a delete"
    );
}

#[tokio::test]
async fn a_list_with_no_page_size_answers_the_default_page_and_a_cursor() {
    // A caller who names no `maxResults` gets a bounded page, not every record stored.
    let emulator = Emulator::new();
    for index in 0..DEFAULT_MAX_RESULTS + 5 {
        emulator
            .create(&format!("arn:aws:iam::{index:012}:root"))
            .await;
    }

    let page = emulator.list("").await;
    let reported = page
        .field("crossAccountAuthorizations")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        reported, DEFAULT_MAX_RESULTS,
        "expected the default page size, got {reported}"
    );
    assert!(
        page.string("nextToken").is_some(),
        "expected a cursor for the records the page left out"
    );
}
