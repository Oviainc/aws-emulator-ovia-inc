// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for S3's bucket REQUEST PAYMENT subresource: set who pays for
//! downloads from a bucket, read it back, and get the right refusal when the
//! configuration or the bucket is not one the service can act on.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is
//! the wire behaviour (status, error code, payload) rather than any handler's internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. `PutBucketRequestPayment` records the submitted `RequestPaymentConfiguration` on
//!    the named bucket and answers 200 with an empty body; `GetBucketRequestPayment`
//!    reports exactly the `Payer` last recorded and nothing of the one before it. A
//!    bucket whose configuration has never been set reports the default the bucket owner
//!    pays under.
//! 2. A `RequestPaymentConfiguration` whose `Payer` is outside the published `Payer`
//!    values is refused with HTTP 400 and records nothing.
//! 3. A request payment read or write against a bucket this emulator does not hold is
//!    `NoSuchBucket` (404); one whose `x-amz-expected-bucket-owner` is not this
//!    emulator's account is `AccessDenied` (403).
//! 4. Each bucket carries its own configuration: setting one bucket's leaves every other
//!    bucket's alone, including a bucket still on the default.
//! 5. The read answers the container the model names for its output — root element
//!    `RequestPaymentConfiguration` with `Payer` inside it — for a defaulted bucket as
//!    much as for a configured one.
//! 6. The configuration is a subresource OF the bucket and lives exactly as long as the
//!    bucket record: while the name holds no bucket the read is `NoSuchBucket`, and a
//!    bucket re-created under that name is back on the default.

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

/// The values `Payer` accepts on the wire, from the model
/// (`smithy_query(query="shape",shape="Payer")` reports
/// `values: ["Requester", "BucketOwner"]`).
const PAYER_VALUES: &[&str] = &["Requester", "BucketOwner"];

/// Who pays for downloads from a bucket whose request payment configuration has never
/// been set: "By default, the bucket owner pays for downloads from the bucket"
/// (`PutBucketRequestPayment`'s own documentation).
const DEFAULT_PAYER: &str = "BucketOwner";

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

/// A `RequestPaymentConfiguration` payload naming `payer`.
fn configuration(payer: &str) -> String {
    format!(
        "<RequestPaymentConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <Payer>{payer}</Payer></RequestPaymentConfiguration>"
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
async fn a_bucket_request_payment_configuration_round_trips_and_a_put_replaces_it() {
    let router = router();
    // Bucket names differing in length and form, so the round trip is not pinned to one
    // name's shape.
    for bucket in ["ab7", "harvest-archive", "logs.2026.quarterly-exports-bucket"] {
        create_bucket(&router, bucket).await;

        // Before any put the bucket exists and the bucket owner pays: the default is
        // REPORTED rather than recorded, so a read of a never-configured bucket answers
        // it.
        let unset = send(&router, "GET", &format!("/{bucket}?requestPayment"), "", &[]).await;
        assert_eq!(
            unset.status, 200,
            "case {bucket:?} \"never configured\": expected HTTP 200, got {} with body {}",
            unset.status, unset.body
        );
        assert_eq!(
            unset.element("Payer").as_deref(),
            Some(DEFAULT_PAYER),
            "case {bucket:?} \"never configured\": expected Payer {DEFAULT_PAYER:?}, got body {}",
            unset.body
        );

        // Every published `Payer`, then every one of them again from the other state, so
        // each row is read back against what IT recorded and not against the row before
        // it. A put REPLACES the subresource, including a put of the default value.
        let mut rows: Vec<String> = Vec::new();
        for payer in PAYER_VALUES {
            rows.push((*payer).to_string());
        }
        for payer in PAYER_VALUES.iter().rev() {
            rows.push((*payer).to_string());
        }
        rows.push(DEFAULT_PAYER.to_string());

        for payer in &rows {
            let answer = send(
                &router,
                "PUT",
                &format!("/{bucket}?requestPayment"),
                &configuration(payer),
                &[],
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {bucket:?}/{payer:?}: PutBucketRequestPayment must accept it, got {} \
                 with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body, "",
                "case {bucket:?}/{payer:?}: PutBucketRequestPayment answers an empty body, \
                 got {}",
                answer.body
            );

            let read = send(&router, "GET", &format!("/{bucket}?requestPayment"), "", &[]).await;
            assert_eq!(
                read.status, 200,
                "case {bucket:?}/{payer:?}: GetBucketRequestPayment must report it, got {} \
                 with body {}",
                read.status, read.body
            );
            assert_eq!(
                read.element("Payer").as_deref(),
                Some(payer.as_str()),
                "case {bucket:?}/{payer:?}: expected Payer {payer:?}, got body {}",
                read.body
            );

            // A read changes nothing: the second one answers exactly what the first did.
            let again = send(&router, "GET", &format!("/{bucket}?requestPayment"), "", &[]).await;
            assert_eq!(
                again.body, read.body,
                "case {bucket:?}/{payer:?}: a read must not change what the bucket carries, \
                 got {} then {}",
                read.body, again.body
            );
        }
    }
}

#[tokio::test]
async fn a_payer_outside_the_published_values_is_refused_and_records_nothing() {
    let router = router();
    let bucket = "harvest-archive";
    create_bucket(&router, bucket).await;

    // A configuration the service CAN apply, so the refusals below have something to
    // leave alone.
    let accepted = send(
        &router,
        "PUT",
        &format!("/{bucket}?requestPayment"),
        &configuration("Requester"),
        &[],
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the baseline put must be accepted, got {} with body {}",
        accepted.status, accepted.body
    );

    // One row per way a `Payer` can fall outside the published pair: a value that is not
    // one of them, one differing only in case (the enum's values are exact), the empty
    // value, and a container carrying no `Payer` at all for the REQUIRED member.
    let refused: &[(&str, String)] = &[
        ("a payer outside the published pair", configuration("Nobody")),
        ("a payer differing only in case", configuration("requester")),
        ("an empty payer", configuration("")),
        (
            "no Payer element at all",
            "<RequestPaymentConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             </RequestPaymentConfiguration>"
                .to_string(),
        ),
    ];
    for (label, payload) in refused {
        let answer = send(
            &router,
            "PUT",
            &format!("/{bucket}?requestPayment"),
            payload,
            &[],
        )
        .await;
        // Two channels refuse a payload outside the published schema — the boundary
        // validator, when the value breaks a MODELLED constraint, and the handler's own
        // `MalformedXML` — and both answer HTTP 400, which is what a client branches on.
        assert_eq!(
            answer.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );

        // And nothing was recorded: the baseline configuration is still what a read
        // reports.
        let read = send(&router, "GET", &format!("/{bucket}?requestPayment"), "", &[]).await;
        assert_eq!(
            read.element("Payer").as_deref(),
            Some("Requester"),
            "case {label:?}: a refused put must leave the recorded configuration alone, got \
             body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn the_request_payment_subresource_refuses_an_absent_bucket_and_a_foreign_owner() {
    let router = router();
    let bucket = "harvest-archive";
    create_bucket(&router, bucket).await;
    let valid = configuration("Requester");

    // A bucket this emulator does not hold, for BOTH the write and the read.
    for (operation, method, body) in [
        ("PutBucketRequestPayment", "PUT", valid.as_str()),
        ("GetBucketRequestPayment", "GET", ""),
    ] {
        let answer = send(
            &router,
            method,
            "/no-such-bucket-at-all?requestPayment",
            body,
            &[],
        )
        .await;
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

    // An expected bucket owner that is not this emulator's account: "the request fails
    // with the HTTP status code 403 Forbidden (access denied)" (`ExpectedBucketOwner`).
    for (operation, method, body) in [
        ("PutBucketRequestPayment", "PUT", valid.as_str()),
        ("GetBucketRequestPayment", "GET", ""),
    ] {
        let answer = send(
            &router,
            method,
            &format!("/{bucket}?requestPayment"),
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
        ("PutBucketRequestPayment", "PUT", valid.as_str()),
        ("GetBucketRequestPayment", "GET", ""),
    ] {
        let owned = send(
            &router,
            method,
            &format!("/{bucket}?requestPayment"),
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
async fn the_read_answers_the_modelled_request_payment_container() {
    let router = router();
    let bucket = "harvest-archive";
    create_bucket(&router, bucket).await;

    // The document S3 answers a request payment read with: the output structure carries
    // `xmlName: RequestPaymentConfiguration`
    // (`smithy_query(query="shape",shape="GetBucketRequestPaymentOutput")`), so the root
    // element is that container and `Payer` is its child — for a bucket still on the
    // default as much as for one every published `Payer` was put on.
    let mut cases: Vec<(&str, &str)> = vec![("never configured", DEFAULT_PAYER)];
    for payer in PAYER_VALUES {
        cases.push(("put", payer));
    }

    for (label, expected) in cases {
        if label == "put" {
            let put = send(
                &router,
                "PUT",
                &format!("/{bucket}?requestPayment"),
                &configuration(expected),
                &[],
            )
            .await;
            assert_eq!(
                put.status, 200,
                "case {label:?}/{expected:?}: the put must be accepted, got {} with body {}",
                put.status, put.body
            );
        }

        let read = send(&router, "GET", &format!("/{bucket}?requestPayment"), "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}/{expected:?}: expected HTTP 200, got {} with body {}",
            read.status, read.body
        );
        assert!(
            read.body.contains("<RequestPaymentConfiguration"),
            "case {label:?}/{expected:?}: expected the RequestPaymentConfiguration container, \
             got body {}",
            read.body
        );
        assert!(
            read.body.contains(&format!("<Payer>{expected}</Payer>")),
            "case {label:?}/{expected:?}: expected Payer {expected:?} inside the container, got \
             body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_bucket_recreated_after_a_delete_is_back_on_the_default_payer() {
    let router = router();
    // The configuration is a subresource OF the bucket, so it lives exactly as long as the
    // bucket record does: a name re-created after its bucket was deleted carries no
    // configuration at all and so reports the default again. One row per published
    // `Payer`, so the rule is not pinned to the non-default value alone.
    for payer in PAYER_VALUES {
        let bucket = "archive-recreated";
        create_bucket(&router, bucket).await;

        let put = send(
            &router,
            "PUT",
            &format!("/{bucket}?requestPayment"),
            &configuration(payer),
            &[],
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {payer:?}: the put must be accepted, got {} with body {}",
            put.status, put.body
        );
        let recorded = send(&router, "GET", &format!("/{bucket}?requestPayment"), "", &[]).await;
        assert_eq!(
            recorded.element("Payer").as_deref(),
            Some(*payer),
            "case {payer:?}: expected the put to be readable first, got body {}",
            recorded.body
        );

        let deleted = send(&router, "DELETE", &format!("/{bucket}"), "", &[]).await;
        assert_eq!(
            deleted.status, 204,
            "case {payer:?}: the empty bucket must be deletable, got {} with body {}",
            deleted.status, deleted.body
        );

        // While the name holds no bucket, the subresource is `NoSuchBucket` rather than a
        // configuration left behind.
        let gone = send(&router, "GET", &format!("/{bucket}?requestPayment"), "", &[]).await;
        assert_eq!(
            gone.error_code(),
            "NoSuchBucket",
            "case {payer:?}: expected NoSuchBucket after the delete, got {} with body {}",
            gone.status, gone.body
        );

        create_bucket(&router, bucket).await;
        let fresh = send(&router, "GET", &format!("/{bucket}?requestPayment"), "", &[]).await;
        assert_eq!(
            fresh.status, 200,
            "case {payer:?}: expected HTTP 200 on the re-created bucket, got {} with body {}",
            fresh.status, fresh.body
        );
        assert_eq!(
            fresh.element("Payer").as_deref(),
            Some(DEFAULT_PAYER),
            "case {payer:?}: expected the re-created bucket to report {DEFAULT_PAYER:?}, got \
             body {}",
            fresh.body
        );

        let cleanup = send(&router, "DELETE", &format!("/{bucket}"), "", &[]).await;
        assert_eq!(
            cleanup.status, 204,
            "case {payer:?}: the bucket must be deletable again, got {} with body {}",
            cleanup.status, cleanup.body
        );
    }
}

#[tokio::test]
async fn each_bucket_keeps_its_own_request_payment_configuration() {
    let router = router();
    // One bucket per published `Payer`, plus one left on the default: a put against any
    // of them must leave the others exactly as they were.
    let buckets: &[(&str, Option<&str>)] = &[
        ("archive-requester", Some("Requester")),
        ("archive-bucket-owner", Some("BucketOwner")),
        ("archive-untouched", None),
    ];
    for (bucket, _) in buckets {
        create_bucket(&router, bucket).await;
    }
    for (bucket, payer) in buckets {
        if let Some(payer) = payer {
            let answer = send(
                &router,
                "PUT",
                &format!("/{bucket}?requestPayment"),
                &configuration(payer),
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
    for (bucket, payer) in buckets {
        let read = send(&router, "GET", &format!("/{bucket}?requestPayment"), "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {bucket:?}: expected HTTP 200, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.element("Payer").as_deref(),
            Some(payer.unwrap_or(DEFAULT_PAYER)),
            "case {bucket:?}: expected Payer {:?}, got body {}",
            payer.unwrap_or(DEFAULT_PAYER),
            read.body
        );
    }
}
