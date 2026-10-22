// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's OWNERSHIP CONTROLS:
//! `PutBucketOwnershipControls` stores them and `GetBucketOwnershipControls` reports them.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! the bucket-lifecycle suite in this crate already owns), so what is pinned is the wire
//! behaviour — status, body, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutBucketOwnershipControls` "creates or modifies OwnershipControls for an Amazon S3
//!   bucket" (the operation's own documentation): the `OwnershipControls` payload becomes
//!   the bucket's controls IN FULL — a replacement, not a merge — and the call answers the
//!   modelled 200 with an empty body.
//! * `GetBucketOwnershipControls` reports exactly those rules, in the order they were
//!   applied, and a bucket with no ownership controls answers
//!   `OwnershipControlsNotFoundError` with HTTP 404 rather than an empty container
//!   (`OwnershipControls$Rules` is a required member, so there is no such thing as
//!   ownership controls with no rule).
//! * An ownership-controls call naming a bucket this emulator does not hold answers the
//!   service's `NoSuchBucket` (HTTP 404), the shape both handlers widen in.
//! * A payload AWS would refuse — an `ObjectOwnership` outside the published value set, or
//!   an `OwnershipControls` naming no `Rule` — is refused with HTTP 400 and leaves the
//!   stored ownership controls untouched.

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::state::MAX_OWNERSHIP_CONTROLS_RULES;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// Every value the `ObjectOwnership` enum publishes, in the model's own order
/// (`smithy_query(query="shape",shape="ObjectOwnership")` reports
/// `values: [BucketOwnerPreferred, ObjectWriter, BucketOwnerEnforced]`).
const OBJECT_OWNERSHIP_VALUES: [&str; 3] = [
    "BucketOwnerPreferred",
    "ObjectWriter",
    "BucketOwnerEnforced",
];

/// The `OwnershipControls` payload for `rules`, spelled the way a client's XML writer
/// spells it: `OwnershipControls$Rules` is `xmlFlattened` with `xmlName` `Rule`, so each
/// rule is a `<Rule>` child of the container
/// (`smithy_query(query="shape",shape="OwnershipControls")`).
fn ownership_controls_payload(rules: &[&str]) -> String {
    let mut body = format!("<OwnershipControls xmlns=\"{XMLNS}\">");
    for rule in rules {
        body.push_str(&format!(
            "<Rule><ObjectOwnership>{rule}</ObjectOwnership></Rule>"
        ));
    }
    body.push_str("</OwnershipControls>");
    body
}

/// `PUT /<bucket>?ownershipControls` — `PutBucketOwnershipControls`.
async fn put_ownership_controls(
    router: &MultiServiceRouter,
    bucket: &str,
    rules: &[&str],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?ownershipControls"),
        &[],
        &ownership_controls_payload(rules),
    )
    .await
}

/// `GET /<bucket>?ownershipControls` — `GetBucketOwnershipControls`.
async fn get_ownership_controls(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/{bucket}?ownershipControls"),
        &[],
        "",
    )
    .await
}

/// The `ObjectOwnership` values a `GetBucketOwnershipControls` body reports, in document
/// order.
///
/// Parsed with `quick-xml` (already a dependency of this workspace), the way the tagging
/// suite beside this one reads a tag set, rather than by slicing the body.
fn reported_rules(answer: &Answer) -> Vec<String> {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut values: Vec<String> = Vec::new();
    let mut element = String::new();
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
            }
            Event::Text(text) => {
                if element == "ObjectOwnership" {
                    let decoded = text
                        .xml10_content()
                        .unwrap_or_else(|error| panic!("the response body must decode: {error}"));
                    values.push(decoded.into_owned());
                }
            }
            Event::End(_) => element.clear(),
            Event::Eof => break,
            _ => {}
        }
    }
    values
}

/// The table of rule lists every row of which must round-trip unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: WHICH
/// `ObjectOwnership` value a rule names (every one the enum publishes, including the two
/// the issue's recorded call never sent), how MANY rules the container carries, and the
/// ORDER they arrive in (which is not the enum's order and not sorted).
fn round_trip_rows() -> Vec<(&'static str, Vec<&'static str>)> {
    let mut rows: Vec<(&'static str, Vec<&'static str>)> = vec![
        (
            "every published value at once, in the model's order",
            OBJECT_OWNERSHIP_VALUES.to_vec(),
        ),
        (
            "the published values in an order that is neither the model's nor sorted",
            vec!["ObjectWriter", "BucketOwnerEnforced", "BucketOwnerPreferred"],
        ),
        (
            "the same value twice keeps both rules",
            vec!["ObjectWriter", "ObjectWriter"],
        ),
        (
            "exactly as many rules as this emulator records for one bucket",
            std::iter::repeat_n(
                OBJECT_OWNERSHIP_VALUES[2],
                MAX_OWNERSHIP_CONTROLS_RULES,
            )
            .collect(),
        ),
    ];
    // One row per published value on its own — the single-rule shape every real client
    // sends, over the whole catalog rather than the one value the issue recorded.
    for value in OBJECT_OWNERSHIP_VALUES {
        rows.push(("a single rule naming one published value", vec![value]));
    }
    rows
}

#[tokio::test]
async fn putting_ownership_controls_stores_them_and_reads_back_through_get() {
    let router = router();

    for (index, (case, rules)) in round_trip_rows().into_iter().enumerate() {
        let bucket = format!("ownership-round-trip-{index}");
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let put = put_ownership_controls(&router, &bucket, &rules).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the modelled 200 from PutBucketOwnershipControls, got \
             {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: PutBucketOwnershipControls's output is Unit, so the body must \
             be empty, got {:?}",
            String::from_utf8_lossy(&put.body)
        );

        let read = get_ownership_controls(&router, &bucket).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetBucketOwnershipControls on the bucket \
             just configured, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            reported_rules(&read),
            rules,
            "case {case:?}: expected the stored rules back unchanged, got {:?}",
            reported_rules(&read)
        );

        // The second put REPLACES: what comes back is the new container alone, with
        // nothing merged in from the rules above.
        let replacement = vec!["BucketOwnerPreferred"];
        let second = put_ownership_controls(&router, &bucket, &replacement).await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: the replacing put must succeed, got {} with body {:?}",
            second.status,
            String::from_utf8_lossy(&second.body)
        );
        let reread = get_ownership_controls(&router, &bucket).await;
        assert_eq!(
            reported_rules(&reread),
            replacement,
            "case {case:?}: the ownership controls must be replaced, not merged, got {:?}",
            reported_rules(&reread)
        );
    }
}

#[tokio::test]
async fn reading_a_bucket_with_no_ownership_controls_is_not_found() {
    let router = router();

    // Two buckets that have never been configured, one of them in a non-default Region, so
    // the answer is about the missing subresource rather than about one bucket.
    for (case, bucket, constraint) in [
        ("a bucket created with no location constraint", "ownership-absent-default", None),
        (
            "a bucket created in another Region",
            "ownership-absent-regional",
            Some("eu-west-1"),
        ),
    ] {
        let created = create_bucket(&router, bucket, constraint, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {}",
            created.status
        );

        let read = get_ownership_controls(&router, bucket).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a bucket with no ownership controls answers the documented \
             not-found rather than an empty container, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.error_code(),
            "OwnershipControlsNotFoundError",
            "case {case:?}: expected the documented error code, got {:?}",
            read.error_code()
        );

        // Reading twice changes nothing: the read reports, it does not configure.
        let again = get_ownership_controls(&router, bucket).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: a second read must answer the same way, got {}",
            again.status
        );

        // And the bucket still takes a put afterwards, so the rows above failed on the
        // missing subresource rather than on a broken bucket.
        let put = put_ownership_controls(&router, bucket, &["BucketOwnerEnforced"]).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the bucket must accept ownership controls, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let read = get_ownership_controls(&router, bucket).await;
        assert_eq!(
            reported_rules(&read),
            vec!["BucketOwnerEnforced"],
            "case {case:?}: the configured bucket must report what it was given, got {:?}",
            reported_rules(&read)
        );
    }
}

#[tokio::test]
async fn a_refused_ownership_controls_payload_changes_nothing() {
    let router = router();
    let bucket = "ownership-refusals";
    create_bucket(&router, bucket, None, None).await;

    // The controls the bucket holds throughout: every refusal below must leave them
    // exactly as they are.
    let stored = vec!["BucketOwnerPreferred", "ObjectWriter"];
    let put = put_ownership_controls(&router, bucket, &stored).await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    // (case, the refused rule list). One row per way a payload can fall outside what the
    // API accepts: a value one step past the published `ObjectOwnership` set (including
    // one differing only in case, since the set is case-sensitive), a container naming no
    // rule at all, and one rule past the ceiling this emulator records.
    let past_the_ceiling: Vec<&str> =
        std::iter::repeat_n("ObjectWriter", MAX_OWNERSHIP_CONTROLS_RULES + 1).collect();
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("a value outside the published set", vec!["BucketOwner"]),
        (
            "a published value differing only in case",
            vec!["bucketownerenforced"],
        ),
        ("an empty value", vec![""]),
        (
            "one rule outside the set among rules that are fine",
            vec!["ObjectWriter", "OwnerPreferred"],
        ),
        ("a container naming no Rule", vec![]),
        ("one rule past the recorded ceiling", past_the_ceiling),
    ];

    for (case, rules) in rows {
        let refused = put_ownership_controls(&router, bucket, &rules).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the payload to be refused with HTTP 400, got {} with \
             body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );

        let read = get_ownership_controls(&router, bucket).await;
        assert_eq!(
            reported_rules(&read),
            stored,
            "case {case:?}: a refused put must leave the stored ownership controls \
             untouched, got {:?}",
            reported_rules(&read)
        );
    }

    // The container naming no Rule is refused by the boundary rather than by the handler:
    // `OwnershipControls$Rules` is a REQUIRED member and the flattened `<Rule>` children
    // are the only way to carry it, so a container with none fails validation before the
    // handler runs and goes out through the framework's refusal code.
    let empty = put_ownership_controls(&router, bucket, &[]).await;
    assert_eq!(
        empty.error_code(),
        "ValidationException",
        "an OwnershipControls naming no Rule must be refused for the missing required \
         member, got {:?}",
        empty.error_code()
    );
}

#[tokio::test]
async fn ownership_controls_on_a_bucket_this_emulator_does_not_hold_is_no_such_bucket() {
    let router = router();
    create_bucket(&router, "held-ownership-bucket", None, None).await;

    // Names that are NOT the one bucket created above: a different name, a prefix of it,
    // an extension of it, and one differing only in case (bucket names are
    // case-sensitive). `NoSuchBucket` carries `httpError 404` in the model.
    for bucket in [
        "absent-ownership-bucket",
        "held",
        "held-ownership-bucket-2",
        "Held-Ownership-Bucket",
    ] {
        for (operation, answer) in [
            (
                "PutBucketOwnershipControls",
                put_ownership_controls(&router, bucket, &["BucketOwnerEnforced"]).await,
            ),
            (
                "GetBucketOwnershipControls",
                get_ownership_controls(&router, bucket).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {bucket:?}/{operation}: expected HTTP 404 for a bucket this emulator \
                 does not hold, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchBucket",
                "case {bucket:?}/{operation}: expected the modelled error code, got {:?}",
                answer.error_code()
            );
        }
    }

    // Nothing above created a bucket as a side effect of configuring one that is absent.
    for bucket in ["absent-ownership-bucket", "held"] {
        let read = get_ownership_controls(&router, bucket).await;
        assert_eq!(
            read.error_code(),
            "NoSuchBucket",
            "case {bucket:?}: a refused put must not have created the bucket, got {:?}",
            read.error_code()
        );
    }

    // And the bucket that IS held configures normally, so the rows above failed on the
    // lookup rather than on the operations being unreachable.
    let put = put_ownership_controls(&router, "held-ownership-bucket", &["ObjectWriter"]).await;
    assert_eq!(
        put.status, 200,
        "the held bucket must accept ownership controls, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
    let read = get_ownership_controls(&router, "held-ownership-bucket").await;
    assert_eq!(
        reported_rules(&read),
        vec!["ObjectWriter"],
        "the held bucket must report the controls it was given, got {:?}",
        reported_rules(&read)
    );
}
