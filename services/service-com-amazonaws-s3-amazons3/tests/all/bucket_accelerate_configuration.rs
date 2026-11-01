// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for S3's bucket accelerate subresource: set the Transfer Acceleration
//! state of a bucket, read it back, and get the right refusal when the payload, the bucket
//! or its name is not one the service can act on.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour (status, error code, payload) rather than any handler's internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. `PutBucketAccelerateConfiguration` records the `Status` its `AccelerateConfiguration`
//!    names on the named bucket and answers 200 with an empty body;
//!    `GetBucketAccelerateConfiguration` reports exactly the last recorded state. A put
//!    REPLACES what the bucket carried, and repeating one changes nothing.
//! 2. A bucket no put has touched has NO Transfer Acceleration state, so a read of it
//!    reports no state value at all rather than either published value.
//! 3. An `AccelerateConfiguration` naming no state the service can set — no `Status`, or a
//!    `Status` outside `BucketAccelerateStatus` — is refused with HTTP 400 and records
//!    nothing.
//! 4. A bucket whose name carries a period is one Transfer Acceleration cannot be used
//!    with, so a put against it is refused with HTTP 400 `InvalidRequest` and records
//!    nothing — whichever state it names.
//! 5. An accelerate read or write against a bucket this emulator does not hold is
//!    `NoSuchBucket` (404); one whose `x-amz-expected-bucket-owner` is not this emulator's
//!    account is `AccessDenied` (403).
//! 6. Each bucket carries its own state: setting one bucket's leaves every other bucket's
//!    alone.

use std::sync::Arc;

use service_com_amazonaws_s3_amazons3::state::MALFORMED_XML_MESSAGE;
use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `s3`. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off, exactly as the emulator runs it unless
/// started with authentication on, so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The values `BucketAccelerateStatus` accepts on the wire, from the model
/// (`smithy_query(query="shape",shape="BucketAccelerateStatus")` reports
/// `values: ["Enabled", "Suspended"]`).
const ACCELERATE_STATUSES: &[&str] = &["Enabled", "Suspended"];

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

/// An `AccelerateConfiguration` payload carrying `Status` when it is `Some`.
fn payload(status: Option<&str>) -> String {
    let status = status.map(|status| format!("<Status>{status}</Status>"));
    format!(
        "<AccelerateConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">{}\
         </AccelerateConfiguration>",
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
async fn a_bucket_accelerate_state_round_trips_and_a_put_replaces_it() {
    let router = router();
    let bucket = "harvest-archive";
    create_bucket(&router, bucket).await;

    // Before any put the bucket has NO Transfer Acceleration state, so the read reports no
    // state value — not a default, and not an error.
    let unset = send(&router, "GET", &format!("/{bucket}?accelerate"), "", &[]).await;
    assert_eq!(
        unset.status, 200,
        "case \"a bucket whose accelerate state was never set\": expected HTTP 200, got {} \
         with body {}",
        unset.status, unset.body
    );
    assert_eq!(
        unset.element("Status"),
        None,
        "case \"a bucket whose accelerate state was never set\": expected no Status at all, \
         got body {}",
        unset.body
    );

    // Every published state, each set after the other so a put is seen to REPLACE what the
    // bucket carried, and each repeated so a second put of the same state is seen to change
    // nothing.
    let mut rows: Vec<(String, &str)> = Vec::new();
    for status in ACCELERATE_STATUSES {
        rows.push((format!("{status} from the previous state"), status));
        rows.push((format!("{status} again, unchanged"), status));
    }
    // And back to the first state, so the table covers both transitions between the pair
    // rather than only the one the recorded order happens to take.
    rows.push((
        format!("{} after {}", ACCELERATE_STATUSES[0], ACCELERATE_STATUSES[1]),
        ACCELERATE_STATUSES[0],
    ));

    for (label, status) in &rows {
        let answer = send(
            &router,
            "PUT",
            &format!("/{bucket}?accelerate"),
            &payload(Some(status)),
            &[],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: the put must be accepted, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body, "",
            "case {label:?}: the put answers an empty body, got {}",
            answer.body
        );

        let read = send(&router, "GET", &format!("/{bucket}?accelerate"), "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the read must report the state, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.element("Status").as_deref(),
            Some(*status),
            "case {label:?}: expected Status {status:?}, got body {}",
            read.body
        );

        // A read changes nothing: the second one answers exactly what the first did.
        let again = send(&router, "GET", &format!("/{bucket}?accelerate"), "", &[]).await;
        assert_eq!(
            again.body, read.body,
            "case {label:?}: a read must not change the bucket's accelerate state"
        );
    }
}

#[tokio::test]
async fn an_accelerate_payload_naming_no_settable_state_is_refused() {
    let router = router();
    let bucket = "harvest-archive";
    create_bucket(&router, bucket).await;

    // A state the service CAN set, so the refusals below have something to leave alone.
    let accepted = send(
        &router,
        "PUT",
        &format!("/{bucket}?accelerate"),
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
    // container, a status just outside the published pair, one differing from a published
    // value only in case (the enum's values are exact), and the empty string.
    //
    // The third column is the error code this OPERATION owns: the container naming no
    // state at all is the row the handler judges, and it is answered the code S3 answers
    // a document that does not validate against its published schema with. A value
    // outside the enum never reaches the handler — the boundary validator refuses it, and
    // this operation does not reword that refusal — so those rows pin only the 400 and
    // that nothing was recorded.
    let refused: &[(&str, String, Option<&str>)] = &[
        ("an empty container", payload(None), Some("MalformedXML")),
        (
            "a Status outside the published pair",
            payload(Some("Paused")),
            None,
        ),
        (
            "a Status differing only in case",
            payload(Some("enabled")),
            None,
        ),
        (
            "a Status that is the empty string",
            payload(Some("")),
            None,
        ),
    ];
    for (label, body, code) in refused {
        let answer = send(&router, "PUT", &format!("/{bucket}?accelerate"), body, &[]).await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );

        if let Some(code) = code {
            assert_eq!(
                answer.error_code(),
                *code,
                "case {label:?}: expected {code}, got {:?}",
                answer.error_code()
            );
            // That code's own FIXED published sentence, whole and alone: it says the
            // document did not validate against the published schema, and that schema is
            // the same for every document, so the message carries nothing about this
            // request.
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(MALFORMED_XML_MESSAGE),
                "case {label:?}: expected the published MalformedXML sentence, got {:?}",
                answer.element("Message")
            );
        }

        // And nothing was recorded: the baseline state is still what a read reports.
        let read = send(&router, "GET", &format!("/{bucket}?accelerate"), "", &[]).await;
        assert_eq!(
            read.element("Status").as_deref(),
            Some("Enabled"),
            "case {label:?}: a refused put must leave the recorded state alone, got body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_bucket_name_carrying_a_period_cannot_be_accelerated() {
    let router = router();
    // Names differing in WHERE the period sits and how many there are, beside one name
    // with none at all as the control. Each is a name S3 holds a bucket under — the
    // published naming rules allow a period between two letters and refuse one at either
    // end — so what the put below is refused for is the feature, not the name.
    let dotted: &[&str] = &[
        "harvest.archive",
        "harvest.archive.logs",
        "h.arvest-archive",
    ];
    for bucket in dotted {
        create_bucket(&router, bucket).await;
    }
    create_bucket(&router, "harvest-archive").await;

    for bucket in dotted {
        // Whichever state the configuration names: the bucket's NAME is what the feature
        // cannot be used with.
        for status in ACCELERATE_STATUSES {
            let answer = send(
                &router,
                "PUT",
                &format!("/{bucket}?accelerate"),
                &payload(Some(status)),
                &[],
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {bucket:?} set to {status:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidRequest",
                "case {bucket:?} set to {status:?}: expected InvalidRequest, got {:?}",
                answer.error_code()
            );
        }

        // Nothing was recorded, so the bucket still has no Transfer Acceleration state —
        // and the READ of it is served, since only the put is refused.
        let read = send(&router, "GET", &format!("/{bucket}?accelerate"), "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {bucket:?}: the read must still be served, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.element("Status"),
            None,
            "case {bucket:?}: a refused put must record nothing, got body {}",
            read.body
        );
    }

    // The control: the same put against a name with no period is accepted, so the 400s
    // above are about the period and not about the request.
    let accepted = send(
        &router,
        "PUT",
        "/harvest-archive?accelerate",
        &payload(Some("Enabled")),
        &[],
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case \"a name with no period\": expected HTTP 200, got {} with body {}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn the_accelerate_subresource_refuses_an_absent_bucket_and_a_foreign_owner() {
    let router = router();
    let bucket = "harvest-archive";
    create_bucket(&router, bucket).await;
    let valid = payload(Some("Enabled"));

    // A bucket this emulator does not hold, for BOTH the write and the read.
    for (operation, method, body) in [
        ("PutBucketAccelerateConfiguration", "PUT", valid.as_str()),
        ("GetBucketAccelerateConfiguration", "GET", ""),
    ] {
        let answer = send(&router, method, "/no-such-bucket-at-all?accelerate", body, &[]).await;
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
        ("PutBucketAccelerateConfiguration", "PUT", valid.as_str()),
        ("GetBucketAccelerateConfiguration", "GET", ""),
    ] {
        let answer = send(
            &router,
            method,
            &format!("/{bucket}?accelerate"),
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
        ("PutBucketAccelerateConfiguration", "PUT", valid.as_str()),
        ("GetBucketAccelerateConfiguration", "GET", ""),
    ] {
        let owned = send(
            &router,
            method,
            &format!("/{bucket}?accelerate"),
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
async fn each_bucket_keeps_its_own_accelerate_state() {
    let router = router();
    // One bucket per published state, plus one left untouched: a put against any of them
    // must leave the others exactly as they were.
    let buckets: &[(&str, Option<&str>)] = &[
        ("archive-accelerate-enabled", Some("Enabled")),
        ("archive-accelerate-suspended", Some("Suspended")),
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
                &format!("/{bucket}?accelerate"),
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
        let read = send(&router, "GET", &format!("/{bucket}?accelerate"), "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {bucket:?}: expected HTTP 200, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.element("Status").as_deref(),
            *status,
            "case {bucket:?}: expected Status {status:?}, got body {}",
            read.body
        );
    }
}
