// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's CORS configuration: `PutBucketCors`
//! stores it and `GetBucketCors` reports it back.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! the bucket-lifecycle suite in this crate already owns), so what is pinned is the wire
//! behaviour — status, body, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutBucketCors` "sets the cors configuration for your bucket. If the configuration
//!   exists, Amazon S3 replaces it" (the operation's own documentation): the
//!   `CORSConfiguration` payload becomes the bucket's cors configuration IN FULL — a
//!   replacement, not a merge — and the call answers the modelled 200 with an empty body.
//! * `GetBucketCors` reports exactly those rules back, every member and every rule
//!   unchanged and in the order they were given, and a bucket with no cors configuration
//!   answers `NoSuchCORSConfiguration` with HTTP 404 rather than an empty document
//!   (`CORSRules` is the whole output document, so an empty 200 would report a
//!   configuration allowing no cross-origin access instead of the absence of one).
//! * A configuration breaking a limit the API publishes — a method outside the `GET`, `PUT`,
//!   `HEAD`, `POST`, `DELETE` set `CORSRule$AllowedMethods` documents, an `ID` longer than
//!   the documented 255 characters, a negative `MaxAgeSeconds`, more than the documented 100
//!   rules, or a document past the documented 64 KB — is refused with HTTP 400 and leaves
//!   the stored configuration untouched.
//! * A cors call naming a bucket this emulator does not hold answers the service's
//!   `NoSuchBucket` (HTTP 404), the shape both handlers widen in.
//!
//! What a round-trip row asserts is the ROUND TRIP, not one hand-written document: the
//! request payload and the reported document are compared leaf by leaf (the website suite's
//! [`content`], which is sensitive to the order of repeated siblings), so a row says "this
//! configuration comes back exactly as it went in" without restating it.

use service_com_amazonaws_s3_amazons3::state::{
    CORS_ALLOWED_METHODS, MAX_CORS_CONFIGURATION_BYTES, MAX_CORS_RULES, MAX_CORS_RULE_ID_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::bucket_website::{content, reported};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The whole `CORSConfiguration` document for the rules `inner` spells.
fn payload(inner: &str) -> String {
    format!("<CORSConfiguration xmlns=\"{XMLNS}\">{inner}</CORSConfiguration>")
}

/// `PUT /<bucket>?cors` — `PutBucketCors`.
async fn put_cors(router: &MultiServiceRouter, bucket: &str, inner: &str) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?cors"),
        &[],
        &payload(inner),
    )
    .await
}

/// `GET /<bucket>?cors` — `GetBucketCors`.
async fn get_cors(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?cors"), &[], "").await
}

/// A `CORSRule` carrying exactly the members `inner` spells.
fn rule(inner: &str) -> String {
    format!("<CORSRule>{inner}</CORSRule>")
}

/// Every entry of a flattened list, each in an element named `name`.
///
/// The element names are the `xmlName`s the model puts on `CORSRule`'s members
/// (`smithy_query(query="shape",shape="CORSRule")`): `AllowedMethod`, `AllowedOrigin`,
/// `AllowedHeader` and `ExposeHeader`.
fn entries(name: &str, values: &[&str]) -> String {
    values
        .iter()
        .map(|value| format!("<{name}>{value}</{name}>"))
        .collect()
}

/// The simplest rule that satisfies the two required members: one method and one origin.
fn simple_rule(method: &str, origin: &str) -> String {
    rule(&format!(
        "{}{}",
        entries("AllowedMethod", &[method]),
        entries("AllowedOrigin", &[origin])
    ))
}

/// The configuration the refusal rows leave in place, and the REPLACEMENT in the round-trip
/// rows: it shares no optional member with the rows below, so a merge rather than a
/// replacement is visible.
fn replacement() -> String {
    simple_rule("HEAD", "https://replacement.example.com")
}

/// The table of cors configurations every row of which must round-trip unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: WHICH of
/// `CORSRule`'s optional members a rule names, which VALUES they carry (every method the
/// `AllowedMethods` documentation publishes, not the ones the recorded call sent, and each
/// published bound of the rule-identifier length), how MANY rules the configuration carries
/// (one, several and the documented ceiling), how many entries a list carries, the ORDER the
/// rules arrive in, and the order the members themselves are spelled in.
fn round_trip_rows() -> Vec<(String, String)> {
    let any_origin = entries("AllowedOrigin", &["*"]);
    let get = entries("AllowedMethod", &["GET"]);

    let mut rows: Vec<(String, String)> = vec![
        (
            "one method and one origin".to_string(),
            simple_rule("GET", "https://www.example.com"),
        ),
        (
            "an origin naming a wildcard".to_string(),
            simple_rule("GET", "*"),
        ),
        (
            "an origin naming a wildcard subdomain".to_string(),
            simple_rule("GET", "https://*.example.com"),
        ),
        (
            "an origin naming a port".to_string(),
            simple_rule("GET", "http://localhost:3000"),
        ),
        (
            "three origins keep the order they arrived in".to_string(),
            rule(&format!(
                "{get}{}",
                entries(
                    "AllowedOrigin",
                    &[
                        "https://first.example.com",
                        "https://second.example.com",
                        "https://third.example.com",
                    ],
                )
            )),
        ),
        (
            "every published method in one rule".to_string(),
            rule(&format!(
                "{}{any_origin}",
                entries("AllowedMethod", &CORS_ALLOWED_METHODS)
            )),
        ),
        (
            "an allowed header".to_string(),
            rule(&format!(
                "{get}{any_origin}{}",
                entries("AllowedHeader", &["x-amz-request-id"])
            )),
        ),
        (
            "a wildcard allowed header".to_string(),
            rule(&format!(
                "{get}{any_origin}{}",
                entries("AllowedHeader", &["*"])
            )),
        ),
        (
            "two exposed headers".to_string(),
            rule(&format!(
                "{get}{any_origin}{}",
                entries("ExposeHeader", &["ETag", "x-amz-version-id"])
            )),
        ),
        (
            "a preflight cache time of zero seconds".to_string(),
            rule(&format!("{get}{any_origin}<MaxAgeSeconds>0</MaxAgeSeconds>")),
        ),
        (
            "a preflight cache time of an hour".to_string(),
            rule(&format!(
                "{get}{any_origin}<MaxAgeSeconds>3600</MaxAgeSeconds>"
            )),
        ),
        (
            "every member of one rule at once".to_string(),
            rule(&format!(
                "<ID>everything</ID>{}{}{}{}<MaxAgeSeconds>1800</MaxAgeSeconds>",
                entries("AllowedHeader", &["*"]),
                entries("AllowedMethod", &["PUT", "POST"]),
                entries("AllowedOrigin", &["https://app.example.com"]),
                entries("ExposeHeader", &["ETag"]),
            )),
        ),
        (
            "the members spelled in the reverse of the model's order".to_string(),
            rule(&format!(
                "<MaxAgeSeconds>60</MaxAgeSeconds>{}{}{}{}<ID>reversed</ID>",
                entries("ExposeHeader", &["ETag"]),
                entries("AllowedOrigin", &["https://app.example.com"]),
                entries("AllowedMethod", &["DELETE"]),
                entries("AllowedHeader", &["content-type"]),
            )),
        ),
        (
            "three rules keep the order they arrived in".to_string(),
            [
                rule(&format!(
                    "<ID>first</ID>{}{}",
                    entries("AllowedMethod", &["GET"]),
                    entries("AllowedOrigin", &["https://first.example.com"])
                )),
                rule(&format!(
                    "<ID>second</ID>{}{}",
                    entries("AllowedMethod", &["PUT"]),
                    entries("AllowedOrigin", &["https://second.example.com"])
                )),
                rule(&format!(
                    "<ID>third</ID>{}{}",
                    entries("AllowedMethod", &["POST"]),
                    entries("AllowedOrigin", &["https://third.example.com"])
                )),
            ]
            .concat(),
        ),
        (
            format!(
                "a rule identifier of exactly the {MAX_CORS_RULE_ID_CHARS} characters AWS \
                 documents"
            ),
            rule(&format!(
                "<ID>{}</ID>{get}{any_origin}",
                "i".repeat(MAX_CORS_RULE_ID_CHARS)
            )),
        ),
        (
            format!("exactly the {MAX_CORS_RULES} rules per configuration AWS documents"),
            (0..MAX_CORS_RULES)
                .map(|index| {
                    rule(&format!(
                        "<ID>rule-{index}</ID>{get}{}",
                        entries(
                            "AllowedOrigin",
                            &[format!("https://host-{index}.example.com").as_str()],
                        )
                    ))
                })
                .collect::<Vec<String>>()
                .concat(),
        ),
    ];

    // One row per method the `AllowedMethods` documentation publishes — the whole catalog
    // rather than the one method a recorded call happened to send.
    for method in CORS_ALLOWED_METHODS {
        rows.push((
            format!("a rule allowing {method} alone"),
            simple_rule(method, "https://www.example.com"),
        ));
    }

    rows
}

#[tokio::test]
async fn putting_a_cors_configuration_stores_it_and_reads_back_through_get() {
    let router = router();

    for (index, (case, configuration)) in round_trip_rows().into_iter().enumerate() {
        let bucket = format!("cors-round-trip-{index}");
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let put = put_cors(&router, &bucket, &configuration).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the modelled 200 from PutBucketCors, got {} with body \
             {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: PutBucketCors's output is Unit, so the body must be empty, got \
             {:?}",
            String::from_utf8_lossy(&put.body)
        );

        let read = get_cors(&router, &bucket).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetBucketCors on the bucket just \
             configured, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            reported(&read),
            content(&payload(&configuration)),
            "case {case:?}: expected the stored configuration back unchanged, got body {:?}",
            String::from_utf8_lossy(&read.body)
        );

        // Reading twice changes nothing: the read reports, it does not configure.
        let again = get_cors(&router, &bucket).await;
        assert_eq!(
            reported(&again),
            reported(&read),
            "case {case:?}: a second read must report exactly what the first did, got body \
             {:?}",
            String::from_utf8_lossy(&again.body)
        );

        // The second put REPLACES: what comes back is the new configuration alone, with
        // nothing merged in from the one above — and the replacement shares no optional
        // member with it, so a merge would show.
        let replaced = put_cors(&router, &bucket, &replacement()).await;
        assert_eq!(
            replaced.status, 200,
            "case {case:?}: the replacing put must succeed, got {} with body {:?}",
            replaced.status,
            String::from_utf8_lossy(&replaced.body)
        );
        let reread = get_cors(&router, &bucket).await;
        assert_eq!(
            reported(&reread),
            content(&payload(&replacement())),
            "case {case:?}: the cors configuration must be replaced, not merged, got body \
             {:?}",
            String::from_utf8_lossy(&reread.body)
        );
    }
}

#[tokio::test]
async fn reading_a_bucket_with_no_cors_configuration_is_not_found() {
    let router = router();

    // Two buckets that have never been configured, one of them in a non-default Region, so
    // the answer is about the missing subresource rather than about one bucket.
    for (case, bucket, constraint) in [
        (
            "a bucket created with no location constraint",
            "cors-absent-default",
            None,
        ),
        (
            "a bucket created in another Region",
            "cors-absent-regional",
            Some("eu-west-1"),
        ),
    ] {
        let created = create_bucket(&router, bucket, constraint, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {}",
            created.status
        );

        let read = get_cors(&router, bucket).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a bucket with no cors configuration answers the documented \
             not-found rather than an empty document, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.error_code(),
            "NoSuchCORSConfiguration",
            "case {case:?}: expected the documented error code, got {:?}",
            read.error_code()
        );

        // Reading twice answers the same way: the read reports, it does not configure.
        let again = get_cors(&router, bucket).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: a second read must answer the same way, got {}",
            again.status
        );

        // And the bucket still takes a put afterwards, so the rows above failed on the
        // missing subresource rather than on a broken bucket.
        let put = put_cors(&router, bucket, &replacement()).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the bucket must accept a cors configuration, got {} with body \
             {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let read = get_cors(&router, bucket).await;
        assert_eq!(
            reported(&read),
            content(&payload(&replacement())),
            "case {case:?}: the configured bucket must report what it was given, got body \
             {:?}",
            String::from_utf8_lossy(&read.body)
        );
    }
}

/// One refusal row: the case, the configuration, and the wire error code it must carry when
/// the rule being broken is one the handler itself judges.
///
/// A row whose code is `None` is one the request never reaches the handler with: both of
/// `CORSRule`'s required lists are `@required` in the model, so a document naming neither is
/// refused by the boundary validator — which answers its own code — before any rule of the
/// store is consulted. The row still pins the REFUSAL (HTTP 400, nothing stored), which is
/// what a caller sees.
type RefusalRow = (String, String, Option<&'static str>);

/// The table of cors configurations that must be refused, each with the code it goes out
/// with.
///
/// The rows differ along WHICH published limit is broken, and where the value sits relative
/// to it: one past each documented bound (the rule count, the identifier length, the
/// document size) and a value outside the documented method catalog in three different
/// shapes (a supported method in the wrong case, a method the catalog omits, and the
/// preflight method itself).
fn refusal_rows() -> Vec<RefusalRow> {
    let any_origin = entries("AllowedOrigin", &["*"]);
    let get = entries("AllowedMethod", &["GET"]);

    let mut rows: Vec<RefusalRow> = vec![
        ("no rule at all".to_string(), String::new(), None),
        (
            "a rule naming no method".to_string(),
            rule(&any_origin),
            None,
        ),
        (
            "a rule naming no origin".to_string(),
            rule(&get),
            None,
        ),
        (
            format!(
                "a rule identifier one character past the {MAX_CORS_RULE_ID_CHARS} AWS \
                 documents"
            ),
            rule(&format!(
                "<ID>{}</ID>{get}{any_origin}",
                "i".repeat(MAX_CORS_RULE_ID_CHARS + 1)
            )),
            Some("InvalidRequest"),
        ),
        (
            "a negative preflight cache time".to_string(),
            rule(&format!(
                "{get}{any_origin}<MaxAgeSeconds>-1</MaxAgeSeconds>"
            )),
            Some("InvalidRequest"),
        ),
        (
            format!("one rule past the {MAX_CORS_RULES} per configuration AWS documents"),
            (0..=MAX_CORS_RULES)
                .map(|index| {
                    rule(&format!(
                        "{get}{}",
                        entries(
                            "AllowedOrigin",
                            &[format!("https://host-{index}.example.com").as_str()],
                        )
                    ))
                })
                .collect::<Vec<String>>()
                .concat(),
            Some("InvalidRequest"),
        ),
        (
            format!(
                "a document past the {MAX_CORS_CONFIGURATION_BYTES} bytes a cors document \
                 may carry"
            ),
            // Few enough rules to stay under the rule ceiling, each carrying an origin long
            // enough that their text alone passes the documented document size.
            (0..MAX_CORS_RULES)
                .map(|index| {
                    rule(&format!(
                        "{get}{}",
                        entries(
                            "AllowedOrigin",
                            &[format!(
                                "https://{}-{index}.example.com",
                                "o".repeat(MAX_CORS_CONFIGURATION_BYTES / MAX_CORS_RULES)
                            )
                            .as_str()],
                        )
                    ))
                })
                .collect::<Vec<String>>()
                .concat(),
            Some("InvalidRequest"),
        ),
    ];

    // A method outside the published catalog, in the three shapes one arrives in.
    for method in ["get", "PATCH", "OPTIONS"] {
        rows.push((
            format!("a rule allowing the unsupported method {method:?}"),
            simple_rule(method, "*"),
            Some("InvalidRequest"),
        ));
    }

    rows
}

#[tokio::test]
async fn a_refused_cors_configuration_changes_nothing() {
    let router = router();
    let bucket = "cors-refusals";
    create_bucket(&router, bucket, None, None).await;

    // The configuration the bucket holds throughout: every refusal below must leave it
    // exactly as it is.
    let held = put_cors(&router, bucket, &replacement()).await;
    assert_eq!(
        held.status, 200,
        "the configuration the refusals must not disturb has to be stored first, got {} \
         with body {:?}",
        held.status,
        String::from_utf8_lossy(&held.body)
    );
    let before = reported(&get_cors(&router, bucket).await);

    for (case, configuration, code) in refusal_rows() {
        let refused = put_cors(&router, bucket, &configuration).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the configuration to be refused with HTTP 400, got {} \
             with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        if let Some(code) = code {
            assert_eq!(
                refused.error_code(),
                code,
                "case {case:?}: expected the published error code, got {:?} with body {:?}",
                refused.error_code(),
                String::from_utf8_lossy(&refused.body)
            );
        }
        assert!(
            !refused.error_message().trim().is_empty(),
            "case {case:?}: a refusal must say which rule the configuration breaks, got an \
             empty message in body {:?}",
            String::from_utf8_lossy(&refused.body)
        );

        let after = reported(&get_cors(&router, bucket).await);
        assert_eq!(
            after, before,
            "case {case:?}: a refused put must leave the stored cors configuration exactly \
             as it was"
        );
    }
}

#[tokio::test]
async fn a_cors_call_on_a_bucket_that_does_not_exist_is_no_such_bucket() {
    let router = router();

    // One bucket name that was never created, and one that exists only as another bucket's
    // neighbour, so the answer is about the named bucket rather than about an empty store.
    create_bucket(&router, "cors-neighbour", None, None).await;

    for (case, bucket) in [
        ("a name nothing was ever created under", "cors-missing"),
        ("a name close to an existing bucket's", "cors-neighbours"),
    ] {
        let put = put_cors(&router, bucket, &replacement()).await;
        assert_eq!(
            put.status, 404,
            "case {case:?}: PutBucketCors on a bucket the emulator does not hold answers \
             404, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert_eq!(
            put.error_code(),
            "NoSuchBucket",
            "case {case:?}: expected the service's NoSuchBucket from PutBucketCors, got {:?}",
            put.error_code()
        );

        let read = get_cors(&router, bucket).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: GetBucketCors on a bucket the emulator does not hold answers \
             404, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.error_code(),
            "NoSuchBucket",
            "case {case:?}: expected the service's NoSuchBucket from GetBucketCors, got {:?}",
            read.error_code()
        );
    }
}
