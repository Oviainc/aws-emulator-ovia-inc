// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's BLOCK PUBLIC ACCESS configuration:
//! `PutPublicAccessBlock` stores it, `GetPublicAccessBlock` reports it and
//! `DeletePublicAccessBlock` removes it.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire behaviour —
//! status, body, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutPublicAccessBlock` "creates or modifies the PublicAccessBlock configuration for an
//!   Amazon S3 bucket" (the operation's own documentation): the
//!   `PublicAccessBlockConfiguration` payload becomes the bucket's configuration IN FULL —
//!   a replacement, not a merge — and the call answers the modelled 200 with an empty body.
//! * Each member of `PublicAccessBlockConfiguration` is a boolean switch ("Specifies
//!   whether Amazon S3 should block … Setting this element to TRUE causes …",
//!   `smithy_query(query="shape",shape="PublicAccessBlockConfiguration")`), so a member the
//!   payload omits names a switch the request does not turn on: it reads back `false`.
//! * `GetPublicAccessBlock` reports exactly the stored switches, and a bucket with no
//!   configuration answers `NoSuchPublicAccessBlockConfiguration` with HTTP 404 rather than
//!   an empty container (the configuration is the whole @httpPayload, so a 200 would report
//!   a configuration that does not exist).
//! * `DeletePublicAccessBlock` removes the configuration and answers the modelled 204,
//!   whether the bucket carried one or not, after which the read is not-found again.
//! * A public-access-block call naming a bucket this emulator does not hold answers the
//!   service's `NoSuchBucket` (HTTP 404), the shape all three handlers widen in; one naming
//!   an `x-amz-expected-bucket-owner` that is not the bucket's owner answers `AccessDenied`
//!   with HTTP 403, as each request's `ExpectedBucketOwner` member documents.

use quick_xml::events::Event;
use quick_xml::Reader;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// Every member `PublicAccessBlockConfiguration` publishes, in the model's own order
/// (`smithy_query(query="shape",shape="PublicAccessBlockConfiguration")` reports
/// `BlockPublicAcls`, `IgnorePublicAcls`, `BlockPublicPolicy`, `RestrictPublicBuckets`,
/// each targeting the boolean `Setting` and each carrying its own `xmlName`).
const SETTINGS: [&str; 4] = [
    "BlockPublicAcls",
    "IgnorePublicAcls",
    "BlockPublicPolicy",
    "RestrictPublicBuckets",
];

/// The four switches a configuration names, in [`SETTINGS`] order.
type Flags = [bool; 4];

/// One member of a `PublicAccessBlockConfiguration` payload: the setting's name and the
/// value the payload gives it.
type Member<'a> = (&'a str, bool);

/// One row of the round-trip table: the case name, the members the payload names, and the
/// four switches the bucket must then report.
type RoundTripRow = (String, Vec<Member<'static>>, Flags);

/// One row of a table applied to a single bucket: the case name, the members the payload
/// names, and the four switches the bucket must then report.
type ReplacementRow<'a> = (&'a str, Vec<Member<'a>>, Flags);

/// One row of the delete table: the case name, the bucket, and the configuration it starts
/// out carrying (`None` for a bucket that was never configured).
type DeleteRow<'a> = (&'a str, &'a str, Option<Vec<Member<'a>>>);

/// The `PublicAccessBlockConfiguration` payload naming exactly `members` — each a
/// `(setting, value)` pair — in the order given.
///
/// A pair the list omits is a member the payload does not carry at all, which is how the
/// omission rows below are sent.
fn public_access_block_payload(members: &[(&str, bool)]) -> String {
    let mut body = format!("<PublicAccessBlockConfiguration xmlns=\"{XMLNS}\">");
    for (setting, value) in members {
        body.push_str(&format!("<{setting}>{value}</{setting}>"));
    }
    body.push_str("</PublicAccessBlockConfiguration>");
    body
}

/// `PUT /<bucket>?publicAccessBlock` — `PutPublicAccessBlock`.
async fn put_public_access_block(
    router: &MultiServiceRouter,
    bucket: &str,
    members: &[(&str, bool)],
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?publicAccessBlock"),
        headers,
        &public_access_block_payload(members),
    )
    .await
}

/// `GET /<bucket>?publicAccessBlock` — `GetPublicAccessBlock`.
async fn get_public_access_block(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{bucket}?publicAccessBlock"),
        headers,
        "",
    )
    .await
}

/// `DELETE /<bucket>?publicAccessBlock` — `DeletePublicAccessBlock`.
async fn delete_public_access_block(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/{bucket}?publicAccessBlock"),
        headers,
        "",
    )
    .await
}

/// The four switches a `GetPublicAccessBlock` body reports, in [`SETTINGS`] order.
///
/// Parsed with `quick-xml` (already a dependency of this workspace), the way the
/// ownership-controls suite beside this one reads its payload, rather than by slicing the
/// body. A setting the body does not carry is reported as absent, so an under-reporting
/// response fails loudly instead of defaulting to `false` here.
fn reported_flags(answer: &Answer) -> [Option<bool>; 4] {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut flags: [Option<bool>; 4] = [None; 4];
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
                if let Some(index) = SETTINGS.iter().position(|setting| *setting == element) {
                    let decoded = text
                        .xml10_content()
                        .unwrap_or_else(|error| panic!("the response body must decode: {error}"));
                    flags[index] = match decoded.as_ref() {
                        "true" => Some(true),
                        "false" => Some(false),
                        other => panic!("a Setting must be a boolean, got {other:?}"),
                    };
                }
            }
            Event::End(_) => element.clear(),
            Event::Eof => break,
            _ => {}
        }
    }
    flags
}

/// The flags `answer` reports, with every setting required to be present.
fn flags_of(case: &str, answer: &Answer) -> Flags {
    let reported = reported_flags(answer);
    let mut flags: Flags = [false; 4];
    for (index, setting) in SETTINGS.iter().enumerate() {
        flags[index] = reported[index].unwrap_or_else(|| {
            panic!(
                "case {case:?}: the reported configuration must carry {setting}, got body {:?}",
                String::from_utf8_lossy(&answer.body)
            )
        });
    }
    flags
}

/// Every payload that must round-trip: the case name, the members it names, and the four
/// switches the bucket must then report.
///
/// The rows differ along what the store-and-report rule generalises over: WHICH switches
/// the configuration names (every one of the 16 combinations of the four, not just the
/// all-true one the issue recorded), HOW MANY members the payload carries (all four, a
/// subset, none at all), and the ORDER they arrive in (the model's order and its reverse).
fn round_trip_rows() -> Vec<RoundTripRow> {
    let mut rows: Vec<RoundTripRow> = Vec::new();

    // All 16 combinations, every member named explicitly: a configuration that names a
    // switch reads that switch back unchanged.
    for mask in 0u8..16 {
        let flags: Flags = [
            mask & 1 != 0,
            mask & 2 != 0,
            mask & 4 != 0,
            mask & 8 != 0,
        ];
        let members: Vec<Member<'static>> = SETTINGS
            .iter()
            .enumerate()
            .map(|(index, setting)| (*setting, flags[index]))
            .collect();
        rows.push((format!("every member named, combination {mask:04b}"), members, flags));
    }

    // One row per single-member payload: the omitted three are switches the request does
    // not turn on, so they read back false.
    for (index, setting) in SETTINGS.iter().enumerate() {
        let mut flags: Flags = [false; 4];
        flags[index] = true;
        rows.push((
            format!("only {setting} named, and turned on"),
            vec![(*setting, true)],
            flags,
        ));
    }

    // A member explicitly turned OFF is the same state as one left out.
    rows.push((
        "a subset naming one switch on and one off".to_string(),
        vec![("IgnorePublicAcls", true), ("RestrictPublicBuckets", false)],
        [false, true, false, false],
    ));

    // The members in the reverse of the model's order: document order is not the
    // configuration.
    rows.push((
        "every member in the reverse of the model's order".to_string(),
        vec![
            ("RestrictPublicBuckets", true),
            ("BlockPublicPolicy", false),
            ("IgnorePublicAcls", true),
            ("BlockPublicAcls", false),
        ],
        [false, true, false, true],
    ));

    // A container naming NO member: `PublicAccessBlockConfiguration` has no required
    // member, so this is a configuration that turns nothing on.
    rows.push((
        "a container naming no member".to_string(),
        vec![],
        [false; 4],
    ));

    rows
}

#[tokio::test]
async fn putting_a_public_access_block_stores_it_and_reads_back_through_get() {
    let router = router();

    for (index, (case, members, expected)) in round_trip_rows().into_iter().enumerate() {
        let bucket = format!("public-access-block-round-trip-{index}");
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let put = put_public_access_block(&router, &bucket, &members, &[]).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the modelled 200 from PutPublicAccessBlock, got {} with \
             body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: PutPublicAccessBlock's output is Unit, so the body must be empty, \
             got {:?}",
            String::from_utf8_lossy(&put.body)
        );

        let read = get_public_access_block(&router, &bucket, &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetPublicAccessBlock on the bucket just \
             configured, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let actual = flags_of(&case, &read);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored switches {expected:?} back, got {actual:?}"
        );

        // Reading twice changes nothing: the read reports, it does not configure.
        let again = get_public_access_block(&router, &bucket, &[]).await;
        let reread = flags_of(&case, &again);
        assert_eq!(
            reread, expected,
            "case {case:?}: a second read must report the same switches {expected:?}, got \
             {reread:?}"
        );
    }
}

#[tokio::test]
async fn a_second_put_replaces_the_whole_configuration() {
    let router = router();
    let bucket = "public-access-block-replacement";
    create_bucket(&router, bucket, None, None).await;

    // Each row is applied on top of the one before it, so what the bucket reports can only
    // be right if the put REPLACED the container rather than merging into it: the all-true
    // configuration is followed by one that names a single switch, then by an empty one.
    let rows: Vec<ReplacementRow> = vec![
        (
            "every switch on",
            SETTINGS.iter().map(|setting| (*setting, true)).collect(),
            [true; 4],
        ),
        (
            "one switch on, the rest left out",
            vec![("BlockPublicPolicy", true)],
            [false, false, true, false],
        ),
        (
            "one switch explicitly off, the rest left out",
            vec![("BlockPublicPolicy", false)],
            [false; 4],
        ),
        (
            "a container naming no member at all",
            vec![],
            [false; 4],
        ),
        (
            "two switches on",
            vec![("BlockPublicAcls", true), ("IgnorePublicAcls", true)],
            [true, true, false, false],
        ),
    ];

    for (case, members, expected) in rows {
        let put = put_public_access_block(&router, bucket, &members, &[]).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the put must succeed, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let read = get_public_access_block(&router, bucket, &[]).await;
        let actual = flags_of(case, &read);
        assert_eq!(
            actual, expected,
            "case {case:?}: the configuration must be replaced, not merged: expected \
             {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn reading_a_bucket_with_no_public_access_block_is_not_found() {
    let router = router();

    // Buckets that have never been configured, one of them in a non-default Region, so the
    // answer is about the missing subresource rather than about one bucket.
    for (case, bucket, constraint) in [
        (
            "a bucket created with no location constraint",
            "public-access-block-absent-default",
            None,
        ),
        (
            "a bucket created in another Region",
            "public-access-block-absent-regional",
            Some("eu-west-1"),
        ),
    ] {
        let created = create_bucket(&router, bucket, constraint, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {}",
            created.status
        );

        for attempt in ["the first read", "a second read"] {
            let read = get_public_access_block(&router, bucket, &[]).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}/{attempt}: a bucket with no PublicAccessBlock configuration \
                 answers the documented not-found rather than an empty container, got {} with \
                 body {:?}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                read.error_code(),
                "NoSuchPublicAccessBlockConfiguration",
                "case {case:?}/{attempt}: expected the documented error code, got {:?}",
                read.error_code()
            );
        }

        // And the bucket still takes a put afterwards, so the reads above failed on the
        // missing subresource rather than on a broken bucket.
        let put = put_public_access_block(&router, bucket, &[("BlockPublicAcls", true)], &[]).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the bucket must accept a configuration, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let read = get_public_access_block(&router, bucket, &[]).await;
        let actual = flags_of(case, &read);
        assert_eq!(
            actual,
            [true, false, false, false],
            "case {case:?}: the configured bucket must report what it was given, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_public_access_block_removes_it_and_is_idempotent() {
    let router = router();

    // Each row is a bucket in a different starting state: configured with every switch on,
    // configured with one, and never configured at all. A delete answers the modelled 204
    // for all three and leaves every one of them with no configuration.
    let rows: Vec<DeleteRow> = vec![
        (
            "a bucket with every switch on",
            "public-access-block-delete-all",
            Some(SETTINGS.iter().map(|setting| (*setting, true)).collect()),
        ),
        (
            "a bucket with one switch on",
            "public-access-block-delete-one",
            Some(vec![("RestrictPublicBuckets", true)]),
        ),
        (
            "a bucket that was never configured",
            "public-access-block-delete-never",
            None,
        ),
    ];

    for (case, bucket, members) in rows {
        create_bucket(&router, bucket, None, None).await;
        if let Some(members) = &members {
            let put = put_public_access_block(&router, bucket, members, &[]).await;
            assert_eq!(
                put.status, 200,
                "case {case:?}: the setup put must succeed, got {} with body {:?}",
                put.status,
                String::from_utf8_lossy(&put.body)
            );
        }

        // Twice: the second delete has nothing left to remove and answers exactly the same
        // way, so the operation is idempotent.
        for attempt in ["the first delete", "a second delete"] {
            let deleted = delete_public_access_block(&router, bucket, &[]).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}/{attempt}: expected the modelled 204 from \
                 DeletePublicAccessBlock, got {} with body {:?}",
                deleted.status,
                String::from_utf8_lossy(&deleted.body)
            );
            assert!(
                deleted.body.is_empty(),
                "case {case:?}/{attempt}: DeletePublicAccessBlock's output is Unit, so the \
                 body must be empty, got {:?}",
                String::from_utf8_lossy(&deleted.body)
            );

            let read = get_public_access_block(&router, bucket, &[]).await;
            assert_eq!(
                read.error_code(),
                "NoSuchPublicAccessBlockConfiguration",
                "case {case:?}/{attempt}: the bucket must carry no configuration after the \
                 delete, got {} with body {:?}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
        }

        // The bucket itself survives the delete and takes a fresh configuration.
        let put = put_public_access_block(&router, bucket, &[("IgnorePublicAcls", true)], &[]).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the bucket must still accept a configuration, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let read = get_public_access_block(&router, bucket, &[]).await;
        let actual = flags_of(case, &read);
        assert_eq!(
            actual,
            [false, true, false, false],
            "case {case:?}: the reconfigured bucket must report what it was given, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn public_access_block_on_a_bucket_this_emulator_does_not_hold_is_no_such_bucket() {
    let router = router();
    create_bucket(&router, "held-public-access-block-bucket", None, None).await;

    // Names that are NOT the one bucket created above: a different name, a prefix of it, an
    // extension of it, and one differing only in case (bucket names are case-sensitive).
    // `NoSuchBucket` carries `httpError 404` in the model.
    for bucket in [
        "absent-public-access-block-bucket",
        "held",
        "held-public-access-block-bucket-2",
        "Held-Public-Access-Block-Bucket",
    ] {
        for (operation, answer) in [
            (
                "PutPublicAccessBlock",
                put_public_access_block(&router, bucket, &[("BlockPublicAcls", true)], &[]).await,
            ),
            (
                "GetPublicAccessBlock",
                get_public_access_block(&router, bucket, &[]).await,
            ),
            (
                "DeletePublicAccessBlock",
                delete_public_access_block(&router, bucket, &[]).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {bucket:?}/{operation}: expected HTTP 404 for a bucket this emulator does \
                 not hold, got {} with body {:?}",
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
    for bucket in ["absent-public-access-block-bucket", "held"] {
        let read = get_public_access_block(&router, bucket, &[]).await;
        assert_eq!(
            read.error_code(),
            "NoSuchBucket",
            "case {bucket:?}: a refused put must not have created the bucket, got {:?}",
            read.error_code()
        );
    }

    // And the bucket that IS held configures normally, so the rows above failed on the
    // lookup rather than on the operations being unreachable.
    let held = "held-public-access-block-bucket";
    let put = put_public_access_block(&router, held, &[("BlockPublicPolicy", true)], &[]).await;
    assert_eq!(
        put.status, 200,
        "the held bucket must accept a configuration, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
    let read = get_public_access_block(&router, held, &[]).await;
    let actual = flags_of("the held bucket", &read);
    assert_eq!(
        actual,
        [false, false, true, false],
        "the held bucket must report the configuration it was given, got {actual:?}"
    );
}

#[tokio::test]
async fn an_expected_bucket_owner_that_is_not_the_owner_is_access_denied() {
    let router = router();
    let bucket = "public-access-block-expected-owner";
    create_bucket(&router, bucket, None, None).await;

    let owner = engine::emulator_account_id();
    let stored: Vec<Member> = vec![("BlockPublicAcls", true)];
    let put = put_public_access_block(&router, bucket, &stored, &[]).await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    // Account ids that are NOT this emulator's: another well-formed 12-digit id, the
    // owner's id with a digit changed, a truncated one, and an empty one.
    let mut mismatched: Vec<String> = vec![
        "123456789012".to_string(),
        owner[..owner.len() - 1].to_string(),
        String::new(),
    ];
    let mut rotated: String = owner.to_string();
    if let Some(last) = rotated.pop() {
        let next = if last == '9' { '0' } else { '9' };
        rotated.push(next);
        mismatched.push(rotated);
    }

    for expected in &mismatched {
        assert_ne!(
            expected.as_str(),
            owner,
            "every row of this table must name an id that is NOT the bucket's owner"
        );
        for (operation, answer) in [
            (
                "PutPublicAccessBlock",
                put_public_access_block(
                    &router,
                    bucket,
                    &[("RestrictPublicBuckets", true)],
                    &[("x-amz-expected-bucket-owner", expected)],
                )
                .await,
            ),
            (
                "GetPublicAccessBlock",
                get_public_access_block(
                    &router,
                    bucket,
                    &[("x-amz-expected-bucket-owner", expected)],
                )
                .await,
            ),
            (
                "DeletePublicAccessBlock",
                delete_public_access_block(
                    &router,
                    bucket,
                    &[("x-amz-expected-bucket-owner", expected)],
                )
                .await,
            ),
        ] {
            assert_eq!(
                answer.status, 403,
                "case {expected:?}/{operation}: an expected owner that is not the bucket's owner \
                 answers HTTP 403, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "AccessDenied",
                "case {expected:?}/{operation}: expected the documented error code, got {:?}",
                answer.error_code()
            );
        }

        // None of the three refusals touched the stored configuration.
        let read = get_public_access_block(&router, bucket, &[]).await;
        let actual = flags_of(expected, &read);
        assert_eq!(
            actual,
            [true, false, false, false],
            "case {expected:?}: a refused call must leave the stored configuration untouched, \
             got {actual:?}"
        );
    }

    // The owner's own id is accepted on all three, so the refusals above are about the
    // mismatch and not about the header being present.
    let read = get_public_access_block(&router, bucket, &[("x-amz-expected-bucket-owner", owner)]).await;
    assert_eq!(
        read.status, 200,
        "the owner's own account id must be accepted, got {} with body {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    let deleted =
        delete_public_access_block(&router, bucket, &[("x-amz-expected-bucket-owner", owner)]).await;
    assert_eq!(
        deleted.status, 204,
        "the owner's own account id must be accepted on the delete, got {} with body {:?}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );
}
