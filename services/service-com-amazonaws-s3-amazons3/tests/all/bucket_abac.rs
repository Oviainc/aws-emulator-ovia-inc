// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for S3's bucket ABAC subresource: set the attribute-based access
//! control property of a bucket, read it back, and get the right refusal when the payload
//! or the bucket is not one the service can act on.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is
//! the wire behaviour (status, error code, payload) rather than any handler's internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. `PutBucketAbac` records the `AbacStatus` the request carries on the named bucket
//!    and answers 200 with an empty body; `GetBucketAbac` reports exactly the last
//!    recorded status. ABAC is a property every general purpose bucket HAS, so a bucket
//!    no put has touched reports the state it was created in rather than nothing at all.
//! 2. An `AbacStatus` naming no state the service can set — no `Status`, or a `Status`
//!    outside `BucketAbacStatus` — is refused with HTTP 400 `MalformedXML`, the code S3
//!    answers a document that does not validate against its published schema with, and
//!    records nothing.
//! 3. An ABAC read or write against a bucket this emulator does not hold is `NoSuchBucket`
//!    (404); one whose `x-amz-expected-bucket-owner` is not this emulator's account is
//!    `AccessDenied` (403).
//! 4. Each bucket carries its own ABAC status: setting one bucket's leaves every other
//!    bucket's alone.

use std::sync::Arc;

use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `s3`. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off, exactly as the emulator runs it unless
/// started with authentication on, so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The values `BucketAbacStatus` accepts on the wire, from the model
/// (`smithy_query(query="shape",shape="BucketAbacStatus")` reports
/// `values: ["Enabled", "Disabled"]`).
const ABAC_STATUSES: &[&str] = &["Enabled", "Disabled"];

/// The status a bucket no `PutBucketAbac` has touched reports — the state `CreateBucket`
/// leaves the property in, since that request carries no ABAC member at all.
const DEFAULT_ABAC_STATUS: &str = "Disabled";

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The wire error code: the `<Code>` element of S3's restXml error document (the
    /// model sets `noErrorWrapping`, so it is the document's own child).
    fn error_code(&self) -> String {
        self.element("Code").unwrap_or_else(|| self.body.clone())
    }

    /// The text of the first `<name>` element in the body, or `None` when the body
    /// carries none — which is how an absent output member reaches the wire.
    fn element(&self, name: &str) -> Option<String> {
        self.body
            .split_once(&format!("<{name}>"))
            .and_then(|(_, rest)| rest.split_once(&format!("</{name}>")))
            .map(|(value, _)| value.to_string())
    }
}

/// Dispatch one request, carrying `headers` beside the ones every request needs.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.us-east-1.amazonaws.com{uri}"))
        .header("host", "s3.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/xml");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body: String::from_utf8_lossy(&body).to_string(),
    }
}

/// An `AbacStatus` payload carrying `Status` when it is `Some`.
fn payload(status: Option<&str>) -> String {
    let status = status.map(|status| format!("<Status>{status}</Status>"));
    format!(
        "<AbacStatus xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">{}</AbacStatus>",
        status.unwrap_or_default()
    )
}

/// Create `bucket`, failing the test if the create did not succeed.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let created = send(router, "PUT", &format!("/{bucket}"), "", &[]).await;
    assert_eq!(
        created.status, 200,
        "the bucket {bucket:?} must be created, got {} with body {}",
        created.status, created.body
    );
}

#[tokio::test]
async fn a_bucket_abac_status_round_trips_and_a_put_replaces_it() {
    let router = router();
    let bucket = "harvest-archive";
    create_bucket(&router, bucket).await;

    // Before any put the bucket still HAS an ABAC property: `CreateBucket` carries no
    // member for it and the API publishes no delete for the subresource, so the bucket is
    // in the state the property defaults to.
    let unset = send(&router, "GET", &format!("/{bucket}?abac"), "", &[]).await;
    assert_eq!(
        unset.status, 200,
        "case \"a bucket whose ABAC was never set\": expected HTTP 200, got {} with body {}",
        unset.status, unset.body
    );
    assert_eq!(
        unset.element("Status").as_deref(),
        Some(DEFAULT_ABAC_STATUS),
        "case \"a bucket whose ABAC was never set\": expected Status {DEFAULT_ABAC_STATUS:?}, \
         got body {}",
        unset.body
    );

    // Every published status, each set after the other so a put is seen to REPLACE what
    // the bucket carried — including the one that puts the property back to its default,
    // and a repeat of a status the bucket already has (idempotent).
    let mut rows: Vec<(String, &str)> = Vec::new();
    for status in ABAC_STATUSES {
        rows.push((format!("{status} from the previous state"), status));
        rows.push((format!("{status} again, unchanged"), status));
    }

    for (label, status) in &rows {
        let answer = send(
            &router,
            "PUT",
            &format!("/{bucket}?abac"),
            &payload(Some(status)),
            &[],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: PutBucketAbac must accept it, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body, "",
            "case {label:?}: PutBucketAbac answers an empty body, got {}",
            answer.body
        );

        let read = send(&router, "GET", &format!("/{bucket}?abac"), "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetBucketAbac must report the status, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.element("Status").as_deref(),
            Some(*status),
            "case {label:?}: expected Status {status:?}, got body {}",
            read.body
        );

        // A read changes nothing: the second one answers exactly what the first did.
        let again = send(&router, "GET", &format!("/{bucket}?abac"), "", &[]).await;
        assert_eq!(
            again.body, read.body,
            "case {label:?}: a read must not change the bucket's ABAC status"
        );
    }
}

#[tokio::test]
async fn an_abac_payload_naming_no_settable_status_is_refused() {
    let router = router();
    let bucket = "harvest-archive";
    create_bucket(&router, bucket).await;

    // A status the service CAN set, so the refusals below have something to leave alone.
    let accepted = send(
        &router,
        "PUT",
        &format!("/{bucket}?abac"),
        &payload(Some("Enabled")),
        &[],
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the baseline put must be accepted, got {} with body {}",
        accepted.status, accepted.body
    );

    // Payloads that name no state the service can set, differing in HOW: an empty
    // container, a status just outside the published pair, and one differing from a
    // published value only in case, since the enum's values are exact. Every row is the
    // same logical failure — a payload that does not validate against the schema the API
    // publishes — so every row carries the one code S3 answers that with, whether the
    // boundary validator or the handler is what finds it.
    let refused: &[(&str, String)] = &[
        ("an empty container", payload(None)),
        (
            "a Status outside the published pair",
            payload(Some("Paused")),
        ),
        ("a Status differing only in case", payload(Some("enabled"))),
        ("a Status that is the empty string", payload(Some(""))),
    ];
    for (label, body) in refused {
        let answer = send(&router, "PUT", &format!("/{bucket}?abac"), body, &[]).await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "MalformedXML",
            "case {label:?}: expected MalformedXML, got {:?}",
            answer.error_code()
        );

        // And nothing was recorded: the baseline status is still what a read reports.
        let read = send(&router, "GET", &format!("/{bucket}?abac"), "", &[]).await;
        assert_eq!(
            read.element("Status").as_deref(),
            Some("Enabled"),
            "case {label:?}: a refused put must leave the recorded status alone, got body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn the_abac_subresource_refuses_an_absent_bucket_and_a_foreign_owner() {
    let router = router();
    let bucket = "harvest-archive";
    create_bucket(&router, bucket).await;
    let valid = payload(Some("Enabled"));

    // A bucket this emulator does not hold, for BOTH the write and the read.
    for (operation, method, body) in [
        ("PutBucketAbac", "PUT", valid.as_str()),
        ("GetBucketAbac", "GET", ""),
    ] {
        let answer = send(&router, method, "/no-such-bucket-at-all?abac", body, &[]).await;
        assert_eq!(
            answer.status, 404,
            "case {operation:?} on an absent bucket: expected HTTP 404, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchBucket",
            "case {operation:?} on an absent bucket: expected NoSuchBucket, got {:?}",
            answer.error_code()
        );
    }

    // An expected bucket owner that is not this emulator's account, for both halves.
    for (operation, method, body) in [
        ("PutBucketAbac", "PUT", valid.as_str()),
        ("GetBucketAbac", "GET", ""),
    ] {
        let answer = send(
            &router,
            method,
            &format!("/{bucket}?abac"),
            body,
            &[("x-amz-expected-bucket-owner", "999988887777")],
        )
        .await;
        assert_eq!(
            answer.status, 403,
            "case {operation:?} with a foreign expected owner: expected HTTP 403, got {} \
             with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "AccessDenied",
            "case {operation:?} with a foreign expected owner: expected AccessDenied, got {:?}",
            answer.error_code()
        );
    }

    // The emulator's own account id is accepted, so the 403 above is about the MISMATCH
    // and not about the header being present.
    for (operation, method, body) in [
        ("PutBucketAbac", "PUT", valid.as_str()),
        ("GetBucketAbac", "GET", ""),
    ] {
        let owned = send(
            &router,
            method,
            &format!("/{bucket}?abac"),
            body,
            &[("x-amz-expected-bucket-owner", engine::emulator_account_id())],
        )
        .await;
        assert_eq!(
            owned.status, 200,
            "case {operation:?} with the emulator's own account as expected owner: expected \
             HTTP 200, got {} with body {}",
            owned.status, owned.body
        );
    }
}

#[tokio::test]
async fn each_bucket_keeps_its_own_abac_status() {
    let router = router();
    // One bucket per published status, plus one left untouched: a put against any of them
    // must leave the others exactly as they were.
    let buckets: &[(&str, Option<&str>)] = &[
        ("archive-abac-enabled", Some("Enabled")),
        ("archive-abac-disabled", Some("Disabled")),
        ("archive-untouched", None),
    ];
    for (bucket, _) in buckets {
        create_bucket(&router, bucket).await;
    }
    for (bucket, status) in buckets {
        if let Some(status) = status {
            let answer = send(
                &router,
                "PUT",
                &format!("/{bucket}?abac"),
                &payload(Some(status)),
                &[],
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {bucket:?}: the put must be accepted, got {} with body {}",
                answer.status, answer.body
            );
        }
    }
    for (bucket, status) in buckets {
        let read = send(&router, "GET", &format!("/{bucket}?abac"), "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {bucket:?}: expected HTTP 200, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.element("Status").as_deref(),
            Some(status.unwrap_or(DEFAULT_ABAC_STATUS)),
            "case {bucket:?}: expected Status {:?}, got body {}",
            status.unwrap_or(DEFAULT_ABAC_STATUS),
            read.body
        );
    }
}
