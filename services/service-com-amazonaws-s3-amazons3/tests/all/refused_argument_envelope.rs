// SPDX-License-Identifier: CC0-1.0

//! A refusal about one ARGUMENT of a request names that argument, and echoes the value it
//! carried.
//!
//! The rule under test: S3's `InvalidArgument` document is
//! `<Error><Code/><Message/><ArgumentName/><ArgumentValue/>…</Error>` (Amazon S3 API
//! Reference, *Error responses*, whose `<Error>` carries the condition's own contextual
//! elements beside the code and the message). The `<Message>` states the RULE the value
//! broke — the same sentence for every value that breaks it — so the pair is the only part of
//! the refusal that says WHICH field the caller must change and what it sent; an SDK error
//! object surfaces them as `ArgumentName` / `ArgumentValue`. The name is the field as the API
//! publishes it, and the value is the one the request carried — a string as the request
//! spelled it, and a TIMESTAMP as the service renders a date (see
//! [`super::served_envelope_values`]).
//!
//! Beside it, the same envelope rule for an object-scoped NOT-FOUND: the document names the
//! `<Key>` the request addressed, and the `<VersionId>` when the request named one, because
//! a client fanning one operation out over many keys cannot otherwise attribute the failure.
//!
//! The tables span what the two rules generalise over:
//!
//! * the `versionId` table over the SHAPE of the token refused (too short, standard Base64's
//!   slash which the shape excludes, an ASCII punctuation mark, a word) and over the one
//!   VALUE no issuer mints (the zero token), and over the CALL that refuses it (the read, the
//!   metadata read, the delete, the attributes read), so one argument is named the same way
//!   wherever it arrives;
//! * the retention table over WHERE the rejected date arrived — the `Retention` document of
//!   `PutObjectRetention` and the `x-amz-object-lock-retain-until-date` header of the three
//!   writes that carry one — and over the SPELLING the request used for it, which the
//!   envelope renders as the service renders a date rather than echoing back; the table of
//!   instants and renderings that rule spans is [`super::served_envelope_values`]'s;
//! * the not-found table over whether the request named a version at all, and over a key
//!   carrying XML metacharacters — which the envelope has to escape, so a caller-chosen key
//!   cannot break the document it is reported in — and over a COPY, whose refusal is about
//!   its SOURCE rather than about the object the request addressed.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate own), so what is pinned is the wire
//! document.

use service_com_amazonaws_s3_amazons3::state::{
    GOVERNANCE_RETENTION_MODE, VERSION_ID_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_legal_hold::lock_enabled_bucket;
use super::object_retention::{now_seconds, put_object_retention};
use super::object_versioning::{delete_object, head_object};

/// The bytes every object in this suite holds.
const BODY: &[u8] = b"argument-envelope-subject";

/// The key every row addresses.
const KEY: &str = "ledger.bin";

/// The code and status a value a request field cannot carry is refused with.
const INVALID_ARGUMENT: &str = "InvalidArgument";

/// The published sentence a `versionId` that is not shaped like one earns.
const INVALID_VERSION_ID_MESSAGE: &str = "Invalid version id specified";

/// The published sentence a retention period that does not end in the future earns.
const RETAIN_UNTIL_MESSAGE: &str = "The retain until date must be in the future!";

/// The tokens the `versionId` table refuses, each with the reason it is not one: a version id
/// is the literal `null` or an opaque token of the published shape and width, and no issuer
/// mints the zero token — so a string of another shape, or the zero one, names no version of
/// anything.
fn malformed_version_ids() -> Vec<(&'static str, String)> {
    let published_width = "a".repeat(VERSION_ID_CHARS);
    vec![
        ("a word that is no id", "not-a-version".to_string()),
        (
            "one character short of the published minimum",
            published_width[..VERSION_ID_CHARS - 1].to_string(),
        ),
        (
            "standard Base64's slash, which the shape excludes",
            format!("{}/", &published_width[..VERSION_ID_CHARS - 1]),
        ),
        (
            "an exclamation mark",
            format!("{}!", &published_width[..VERSION_ID_CHARS - 1]),
        ),
        (
            "the zero token, which no issuer mints",
            "A".repeat(VERSION_ID_CHARS),
        ),
    ]
}

/// `GET /<bucket>/<key>?attributes` — `GetObjectAttributes`, which takes a `versionId` too.
async fn get_object_attributes(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    query: &str,
) -> Answer {
    send_bytes(
        router,
        "GET",
        &format!("/{bucket_name}/{key}?attributes{query}"),
        &[("x-amz-object-attributes", "ETag")],
        b"",
    )
    .await
}

/// Assert `answer` is the `InvalidArgument` refusal naming `argument` with `value`.
fn assert_names_argument(answer: &Answer, case: &str, argument: &str, value: &str, message: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {:?}",
        answer.status,
        text(answer)
    );
    assert_eq!(
        answer.error_code(),
        INVALID_ARGUMENT,
        "case {case:?}: expected the {INVALID_ARGUMENT:?} code, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        answer.error_message(),
        message,
        "case {case:?}: expected the published sentence {message:?}, got {:?}",
        answer.error_message()
    );
    assert_eq!(
        answer.element("ArgumentName").as_deref(),
        Some(argument),
        "case {case:?}: the envelope must name the field it rejected, got {:?} in {:?}",
        answer.element("ArgumentName"),
        text(answer)
    );
    assert_eq!(
        answer.element("ArgumentValue").as_deref(),
        Some(value),
        "case {case:?}: the envelope must echo the value that field carried, got {:?} in \
         {:?}",
        answer.element("ArgumentValue"),
        text(answer)
    );
}

#[tokio::test]
async fn a_malformed_version_id_is_refused_by_name_wherever_it_arrives() {
    let router = router();
    let name = "argument-envelope-versions";
    bucket(&router, name).await;
    let written = put_object(&router, name, KEY, BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );

    for (case, version_id) in malformed_version_ids() {
        let query = format!("?versionId={version_id}");
        for (call, answer) in [
            (
                "GetObject",
                get_object(&router, name, KEY, &query, &[]).await,
            ),
            (
                "DeleteObject",
                delete_object(&router, name, KEY, &query).await,
            ),
            (
                "GetObjectAttributes",
                get_object_attributes(&router, name, KEY, &format!("&versionId={version_id}"))
                    .await,
            ),
        ] {
            assert_names_argument(
                &answer,
                &format!("{case}/{call}"),
                "versionId",
                &version_id,
                INVALID_VERSION_ID_MESSAGE,
            );
        }
        // A HEAD of the same address answers the same status and carries no document, so
        // the status is the whole answer there.
        let headed = head_object(&router, name, KEY, &query).await;
        assert_eq!(
            headed.status, 400,
            "case {case:?}/HeadObject: expected HTTP 400, got {}",
            headed.status
        );
    }

    // The object itself is untouched by every refusal above.
    let read = get_object(&router, name, KEY, "", &[]).await;
    assert_eq!(
        read.body.as_ref(),
        BODY,
        "a refused argument must leave the object as it was, got {:?}",
        text(&read)
    );
}

#[tokio::test]
async fn a_retention_date_that_has_passed_is_refused_by_name_and_rendered_as_a_date() {
    let router = router();
    let name = "argument-envelope-retention";
    lock_enabled_bucket(&router, name).await;
    let written = put_object(&router, name, KEY, BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );

    // (case, the text the request spells the date in). Every row is the SAME instant that
    // has gone by, spelled the way a client may send it: the canonical ISO 8601 date-time,
    // one with a fraction of a second, and one whose fraction is explicitly zero. The
    // envelope renders the INSTANT rather than echoing the text, so every row — and every
    // call it arrives on — must carry one and the same value; which rendering that is, over
    // a table of instants, is pinned by [`super::served_envelope_values`].
    let a_day_ago = now_seconds() - 24 * 3_600;
    let spellings: Vec<(&str, String)> = vec![
        ("the canonical date-time", iso8601(a_day_ago)),
        (
            "a fraction of a second",
            format!("{}.250Z", iso8601(a_day_ago).trim_end_matches('Z')),
        ),
        (
            "a fraction that is explicitly zero",
            format!("{}.000Z", iso8601(a_day_ago).trim_end_matches('Z')),
        ),
    ];

    let mut rendered: Option<String> = None;
    for (case, spelled) in &spellings {
        // In the `Retention` DOCUMENT of the retention write.
        let payload = format!(
            "<Retention xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <Mode>{GOVERNANCE_RETENTION_MODE}</Mode>\
             <RetainUntilDate>{spelled}</RetainUntilDate></Retention>"
        );
        let refused = put_object_retention(&router, name, KEY, "", &[], &payload).await;
        let echoed = refused.element("ArgumentValue").unwrap_or_default();
        assert_ne!(
            echoed, *spelled,
            "case {case:?}/PutObjectRetention: the envelope must render the rejected \
             instant, not echo the request's own text, got {echoed:?} in {:?}",
            text(&refused)
        );
        let expected = rendered.get_or_insert_with(|| echoed.clone()).clone();
        assert_names_argument(
            &refused,
            &format!("{case}/PutObjectRetention"),
            "RetainUntilDate",
            &expected,
            RETAIN_UNTIL_MESSAGE,
        );

        // And in the `x-amz-object-lock-retain-until-date` HEADER of the writes that carry
        // one: the field is that header, and the value is that same rendering.
        let headers = vec![
            ("x-amz-object-lock-mode", GOVERNANCE_RETENTION_MODE),
            ("x-amz-object-lock-retain-until-date", spelled.as_str()),
        ];
        for (call, answer) in [
            (
                "PutObject",
                put_object(&router, name, "locked-on-write.bin", BODY, &headers).await,
            ),
            (
                "CreateMultipartUpload",
                send(
                    &router,
                    "POST",
                    &format!("/{name}/locked-upload.bin?uploads"),
                    &headers,
                    "",
                )
                .await,
            ),
        ] {
            assert_names_argument(
                &answer,
                &format!("{case}/{call}"),
                "x-amz-object-lock-retain-until-date",
                &expected,
                RETAIN_UNTIL_MESSAGE,
            );
        }
    }

    // A date in the FUTURE is served, so every row above was refused for the period it
    // described and not for carrying a date at all.
    let future = format!(
        "<Retention xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <Mode>{GOVERNANCE_RETENTION_MODE}</Mode><RetainUntilDate>{}</RetainUntilDate>\
         </Retention>",
        iso8601(now_seconds() + 24 * 3_600)
    );
    let placed = put_object_retention(&router, name, KEY, "", &[], &future).await;
    assert_eq!(
        placed.status, 200,
        "a period that ends in the future must be served, got {} with body {:?}",
        placed.status,
        text(&placed)
    );
    // And the write the header refused stored nothing.
    let absent = get_object(&router, name, "locked-on-write.bin", "", &[]).await;
    assert_eq!(
        absent.status, 404,
        "a refused write must store nothing, got {} with body {:?}",
        absent.status,
        text(&absent)
    );
}

/// `epoch_seconds` as the ISO 8601 `date-time` the wire carries a timestamp in.
fn iso8601(epoch_seconds: i64) -> String {
    aws_smithy_types::DateTime::from_secs(epoch_seconds)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("the test's own instant renders as an ISO 8601 date-time")
}

#[tokio::test]
async fn an_object_scoped_not_found_names_the_key_and_the_version_it_addressed() {
    let router = router();
    let name = "argument-envelope-not-found";
    bucket(&router, name).await;
    super::object_versioning::set_versioning(&router, name, "Enabled").await;
    let written = put_object(&router, name, KEY, BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    let minted = written
        .header("x-amz-version-id")
        .expect("a versioning-enabled bucket reports the version id it minted");

    // (case, the key the read addresses, the versionId it names, the code it must answer).
    // The rows differ along whether the key is there and whether the request named a
    // version: a key the bucket holds nothing at is `NoSuchKey`, and a version it holds no
    // entry for is `NoSuchVersion`.
    let absent_version = "0".repeat(VERSION_ID_CHARS);
    let rows: Vec<(&str, &str, &str, Option<&str>, &str)> = vec![
        (
            "a key the bucket holds nothing at",
            "never/written.bin",
            "never/written.bin",
            None,
            "NoSuchKey",
        ),
        (
            "a key with a dotted name the bucket holds nothing at",
            "logs/2026.01.01.json",
            "logs/2026.01.01.json",
            None,
            "NoSuchKey",
        ),
        (
            // The envelope escapes what it carries, so a key a caller chose cannot break the
            // document it is reported in.
            "a key carrying XML metacharacters",
            "a%3Cb%3E%26c.bin",
            "a<b>&c.bin",
            None,
            "NoSuchKey",
        ),
        (
            "a version the bucket holds no entry for",
            KEY,
            KEY,
            Some(absent_version.as_str()),
            "NoSuchVersion",
        ),
        (
            "a version of a key that holds nothing at all",
            "never/written.bin",
            "never/written.bin",
            Some(absent_version.as_str()),
            "NoSuchVersion",
        ),
    ];

    for (case, addressed, key, version_id, code) in rows {
        let query = version_id
            .map(|version_id| format!("?versionId={version_id}"))
            .unwrap_or_default();
        let read = get_object(&router, name, addressed, &query, &[]).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.error_code(),
            code,
            "case {case:?}: expected the {code:?} code, got {:?}",
            read.error_code()
        );
        assert_eq!(
            read.element("Key").as_deref(),
            Some(key),
            "case {case:?}: the envelope must name the key the request addressed, got {:?} \
             in {:?}",
            read.element("Key"),
            text(&read)
        );
        assert_eq!(
            read.element("VersionId").as_deref(),
            version_id,
            "case {case:?}: the envelope must name the version the request addressed, and \
             none where it named none, got {:?} in {:?}",
            read.element("VersionId"),
            text(&read)
        );
    }

    // The version that IS there still reads back, so every refusal above was about the
    // address it named.
    let read = get_object(&router, name, KEY, &format!("?versionId={minted}"), &[]).await;
    assert_eq!(
        read.body.as_ref(),
        BODY,
        "the stored version must still read back, got {:?}",
        text(&read)
    );

    // A COPY is refused about its SOURCE, which is not the object the request addressed: the
    // envelope names the key `x-amz-copy-source` carried, not the destination the request
    // was sent to. (`<Key>`, and `<VersionId>` for a copy naming one.)
    for (case, source, expected_key, expected_version) in [
        (
            "a copy source the bucket holds nothing at",
            format!("{name}/never/copied.bin"),
            "never/copied.bin",
            None,
        ),
        (
            "a copy source version the bucket holds no entry for",
            format!("{name}/{KEY}?versionId={absent_version}"),
            KEY,
            Some(absent_version.as_str()),
        ),
    ] {
        let refused = send_bytes(
            &router,
            "PUT",
            &format!("/{name}/copies/destination.bin"),
            &[("x-amz-copy-source", source.as_str())],
            b"",
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.element("Key").as_deref(),
            Some(expected_key),
            "case {case:?}: the envelope must name the SOURCE key, got {:?} in {:?}",
            refused.element("Key"),
            text(&refused)
        );
        assert_eq!(
            refused.element("VersionId").as_deref(),
            expected_version,
            "case {case:?}: the envelope must name the source version the copy named, got \
             {:?} in {:?}",
            refused.element("VersionId"),
            text(&refused)
        );
    }
}
