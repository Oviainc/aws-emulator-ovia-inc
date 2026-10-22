// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for S3's bucket VERSIONING subresource: set a versioning state, read
//! it back, and get the right refusal when the configuration or the bucket is not one the
//! service can act on.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is
//! the wire behaviour (status, error code, payload) rather than any handler's internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. `PutBucketVersioning` records the submitted `VersioningConfiguration` on the named
//!    bucket and answers 200 with an empty body; `GetBucketVersioning` reports exactly
//!    the last recorded configuration and nothing of the one before it. A bucket whose
//!    versioning state has never been set reports no `Status` at all, and `MfaDelete` is
//!    reported only when the recorded configuration carried one.
//! 2. A `VersioningConfiguration` that names no state the service can set — no `Status`,
//!    or a `Status` outside `BucketVersioningStatus` — is refused with HTTP 400
//!    `IllegalVersioningConfigurationException`, and records nothing.
//! 3. A versioning read or write against a bucket this emulator does not hold is
//!    `NoSuchBucket` (404); one whose `x-amz-expected-bucket-owner` is not this
//!    emulator's account is `AccessDenied` (403).
//! 4. Each bucket carries its own versioning state: setting one bucket's leaves every
//!    other bucket's alone.

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

/// The values `BucketVersioningStatus` accepts on the wire, from the model
/// (`smithy_query(query="shape",shape="BucketVersioningStatus")` reports
/// `values: ["Enabled", "Suspended"]`).
const VERSIONING_STATUSES: &[&str] = &["Enabled", "Suspended"];

/// The values `MFADelete` accepts on the wire, from the model
/// (`smithy_query(query="shape",shape="MFADelete")` reports
/// `values: ["Enabled", "Disabled"]`).
const MFA_DELETE_VALUES: &[&str] = &["Enabled", "Disabled"];

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

/// A `VersioningConfiguration` payload carrying the members that are `Some`.
fn configuration(status: Option<&str>, mfa_delete: Option<&str>) -> String {
    let status = status.map(|status| format!("<Status>{status}</Status>"));
    // `VersioningConfiguration$MFADelete` carries `xmlName: "MfaDelete"`, so that — not
    // the member name — is the element on the wire.
    let mfa_delete = mfa_delete.map(|value| format!("<MfaDelete>{value}</MfaDelete>"));
    format!(
        "<VersioningConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">{}{}\
         </VersioningConfiguration>",
        status.unwrap_or_default(),
        mfa_delete.unwrap_or_default()
    )
}

/// One row of the refusal table: what it is, the `VersioningConfiguration` payload, and
/// the extra headers the request carries.
type RefusedCase = (&'static str, String, Vec<(&'static str, &'static str)>);

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
async fn a_bucket_versioning_state_round_trips_and_a_put_replaces_it() {
    let router = router();
    let bucket = "harvest-archive";
    create_bucket(&router, bucket).await;

    // Before any put the bucket exists but has no versioning state: "a
    // GetBucketVersioning request does not return a versioning state value"
    // (`PutBucketVersioning`'s documentation).
    let unset = send(&router, "GET", &format!("/{bucket}?versioning"), "", &[]).await;
    assert_eq!(
        unset.status, 200,
        "case \"a bucket whose versioning was never set\": expected HTTP 200, got {} with \
         body {}",
        unset.status, unset.body
    );
    assert_eq!(
        unset.element("Status"),
        None,
        "case \"a bucket whose versioning was never set\": expected no Status, got body {}",
        unset.body
    );
    assert_eq!(
        unset.element("MfaDelete"),
        None,
        "case \"a bucket whose versioning was never set\": expected no MfaDelete, got body {}",
        unset.body
    );

    // Configurations differing along every dimension the rule generalises over: each
    // published `Status`, each published `MfaDelete`, and whether `MfaDelete` is carried
    // at all. A put REPLACES the subresource, so the read after each row is asserted
    // against that row alone — including the rows that drop `MfaDelete` again.
    let mut rows: Vec<(String, Option<&str>, Option<&str>)> = Vec::new();
    for status in VERSIONING_STATUSES {
        rows.push((format!("{status}, no MfaDelete"), Some(status), None));
        for mfa_delete in MFA_DELETE_VALUES {
            rows.push((
                format!("{status}, MfaDelete {mfa_delete}"),
                Some(status),
                Some(mfa_delete),
            ));
        }
        // And back to a configuration without `MfaDelete`, which must leave none.
        rows.push((
            format!("{status} again, MfaDelete dropped"),
            Some(status),
            None,
        ));
    }

    for (label, status, mfa_delete) in &rows {
        // "you must include the x-amz-mfa request header and the Status and the MfaDelete
        // request elements" — a row carrying `MfaDelete` carries the header too.
        let headers: Vec<(&str, &str)> = match mfa_delete {
            Some(_) => vec![("x-amz-mfa", "arn:aws:iam::123456789012:mfa/device 123456")],
            None => Vec::new(),
        };
        let answer = send(
            &router,
            "PUT",
            &format!("/{bucket}?versioning"),
            &configuration(*status, *mfa_delete),
            &headers,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: PutBucketVersioning must accept it, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body, "",
            "case {label:?}: PutBucketVersioning answers an empty body, got {}",
            answer.body
        );

        let read = send(&router, "GET", &format!("/{bucket}?versioning"), "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetBucketVersioning must report the state, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.element("Status").as_deref(),
            *status,
            "case {label:?}: expected Status {status:?}, got body {}",
            read.body
        );
        assert_eq!(
            read.element("MfaDelete").as_deref(),
            *mfa_delete,
            "case {label:?}: expected MfaDelete {mfa_delete:?}, got body {}",
            read.body
        );

        // A read changes nothing: the second one answers exactly what the first did.
        let again = send(&router, "GET", &format!("/{bucket}?versioning"), "", &[]).await;
        assert_eq!(
            again.body, read.body,
            "case {label:?}: a read must not change the bucket's versioning state"
        );
    }
}

#[tokio::test]
async fn a_versioning_configuration_naming_no_settable_state_is_refused() {
    let router = router();
    let bucket = "harvest-archive";
    create_bucket(&router, bucket).await;

    // A state the service CAN set, so the refusals below have something to leave alone.
    let accepted = send(
        &router,
        "PUT",
        &format!("/{bucket}?versioning"),
        &configuration(Some("Enabled"), None),
        &[],
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the baseline put must be accepted, got {} with body {}",
        accepted.status, accepted.body
    );

    // Configurations that name no state the service can set, differing in HOW: an empty
    // container, a container carrying only `MfaDelete`, and each `Status` value just
    // outside the published pair (including one differing only in case, since the enum's
    // values are exact).
    let refused: &[RefusedCase] = &[
        ("an empty container", configuration(None, None), Vec::new()),
        (
            "only MfaDelete, no Status",
            configuration(None, Some("Enabled")),
            vec![("x-amz-mfa", "arn:aws:iam::123456789012:mfa/device 123456")],
        ),
        (
            "a Status outside the published pair",
            configuration(Some("Paused"), None),
            Vec::new(),
        ),
        (
            "a Status differing only in case",
            configuration(Some("enabled"), None),
            Vec::new(),
        ),
        (
            "MfaDelete without the x-amz-mfa header",
            configuration(Some("Suspended"), Some("Enabled")),
            Vec::new(),
        ),
    ];
    for (label, payload, headers) in refused {
        let answer = send(
            &router,
            "PUT",
            &format!("/{bucket}?versioning"),
            payload,
            headers,
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );

        // And nothing was recorded: the baseline state is still what a read reports.
        let read = send(&router, "GET", &format!("/{bucket}?versioning"), "", &[]).await;
        assert_eq!(
            read.element("Status").as_deref(),
            Some("Enabled"),
            "case {label:?}: a refused put must leave the recorded state alone, got body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn the_versioning_subresource_refuses_an_absent_bucket_and_a_foreign_owner() {
    let router = router();
    let bucket = "harvest-archive";
    create_bucket(&router, bucket).await;
    let valid = configuration(Some("Enabled"), None);

    // A bucket this emulator does not hold, for BOTH the write and the read.
    for (operation, method, body) in [
        ("PutBucketVersioning", "PUT", valid.as_str()),
        ("GetBucketVersioning", "GET", ""),
    ] {
        let answer = send(
            &router,
            method,
            "/no-such-bucket-at-all?versioning",
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
        ("PutBucketVersioning", "PUT", valid.as_str()),
        ("GetBucketVersioning", "GET", ""),
    ] {
        let answer = send(
            &router,
            method,
            &format!("/{bucket}?versioning"),
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
    let owned = send(
        &router,
        "PUT",
        &format!("/{bucket}?versioning"),
        &valid,
        &[("x-amz-expected-bucket-owner", engine::emulator_account_id())],
    )
    .await;
    assert_eq!(
        owned.status, 200,
        "case \"the emulator's own account as expected owner\": expected HTTP 200, got {} \
         with body {}",
        owned.status, owned.body
    );
}

#[tokio::test]
async fn each_bucket_keeps_its_own_versioning_state() {
    let router = router();
    // One bucket per published `Status`, plus one left unset: a put against any of them
    // must leave the others exactly as they were.
    let buckets: &[(&str, Option<&str>)] = &[
        ("archive-enabled", Some("Enabled")),
        ("archive-suspended", Some("Suspended")),
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
                &format!("/{bucket}?versioning"),
                &configuration(Some(status), None),
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
        let read = send(&router, "GET", &format!("/{bucket}?versioning"), "", &[]).await;
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
