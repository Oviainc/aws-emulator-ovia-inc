// SPDX-License-Identifier: CC0-1.0

//! What a response ENVELOPE serves for the values the service itself renders: the ids a
//! response names its request by, the rendering of a timestamp a refusal rejected, and the
//! spelling of a stored enum a read reports.
//!
//! The rules under test:
//!
//! * Every response of this service — a refusal included — names its request by the pair
//!   of ids S3 publishes for it: "`x-amz-request-id`: A value created by Amazon S3 that
//!   uniquely identifies the request" and "`x-amz-id-2`: A special token that helps Amazon
//!   Web Services troubleshoot problems" (Amazon S3 API Reference, *Common Response
//!   Headers*). Each is an OPAQUE TOKEN of its own shape — the request id uppercase
//!   alphanumeric, the extended one lowercase hexadecimal — the REST-XML error document
//!   mirrors them in `<RequestId>` and `<HostId>`, the two have to agree, and no two
//!   responses are named by the same pair. The shape is the contract: a client, a log
//!   correlator or a support request reads the token, so a lowercase hyphenated UUID is
//!   not one of these ids however unique it is.
//! * A refusal about one ARGUMENT echoes that argument's value, and a TIMESTAMP is
//!   rendered there the way the service renders a date — `EEE MMM dd HH:mm:ss zzz yyyy`,
//!   in the service's own zone — rather than in the ISO 8601 form the wire carried it in
//!   or in whatever form the caller happened to spell it. Two requests naming one instant
//!   in different spellings are therefore refused with the same value.
//! * A refusal about ONE TAG of a tag set names that tag in the envelope — `<TagKey>`, plus
//!   `<TagValue>` where the VALUE is what was refused — because every sentence S3 states
//!   for a refused tag is fixed text that names no tag; a refusal about the SET itself
//!   names none.
//! * A `FilterRule`'s `Name` is reported in the CANONICAL capitalised spelling of its rule
//!   kind whatever spelling the store holds: `FilterRuleName`'s enum values are the
//!   REQUEST vocabulary (`prefix`, `suffix`,
//!   `smithy_query(query="shape",shape="FilterRuleName")`), while the name S3 reports to
//!   every reader of a bucket's notification document is the capitalised one.
//!
//! The tables span what each rule generalises over: the OPERATION and the CLASS of refusal
//! the envelope answers (a not-found, a bad argument, a bad tag, and a served response
//! beside them); for the timestamp, the INSTANT, the SPELLING the request used and WHERE
//! the date arrived (a `Retention` document, and the `x-amz-object-lock-retain-until-date`
//! header of the three writes that carry one); for the filter rule, the SPELLING the store
//! holds, the rule KIND and the DESTINATION kind whose configuration carries it.
//!
//! Every assertion drives a real HTTP request through the router (the harness
//! [`super::bucket_lifecycle`] owns), so what is pinned is the wire document.

use std::sync::Arc;

use service_com_amazonaws_s3_amazons3::state::{
    canonical_filter_rule_name, BucketNotificationConfiguration, ComAmazonawsS3Amazons3State,
    NotificationFilterRule, NotificationTarget, FILTER_RULE_NAME_VALUES,
    GOVERNANCE_RETENTION_MODE, MAX_TAGS_PER_BUCKET, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};
use super::bucket_lifecycle::{router, router_with_engine, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_legal_hold::lock_enabled_bucket;
use super::object_retention::put_object_retention;

/// How many characters an `x-amz-request-id` carries, and so how wide the `<RequestId>` of
/// an error document is: the fixed-width token S3 names a request by.
const REQUEST_ID_CHARS: usize = 16;

/// How many characters an `x-amz-id-2` carries — the wider token of the pair, which S3
/// writes in lowercase hexadecimal.
const EXTENDED_REQUEST_ID_CHARS: usize = 64;

/// Whether `token` is shaped like the id `x-amz-request-id` carries: an opaque
/// [`REQUEST_ID_CHARS`]-character token of digits and UPPERCASE letters.
///
/// A lowercase hyphenated UUID fails on every count — its width, its case and its
/// separators — which is exactly what this predicate is here to catch.
fn is_request_id(token: &str) -> bool {
    token.len() == REQUEST_ID_CHARS
        && token
            .chars()
            .all(|character| character.is_ascii_digit() || character.is_ascii_uppercase())
}

/// Whether `token` is shaped like the id `x-amz-id-2` carries:
/// [`EXTENDED_REQUEST_ID_CHARS`] characters of LOWERCASE hexadecimal.
fn is_extended_request_id(token: &str) -> bool {
    token.len() == EXTENDED_REQUEST_ID_CHARS
        && token
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
}

/// The pair `answer` named its request by in its HEADERS, failing the test when it carried
/// neither — every S3 response carries both.
fn correlation_headers(answer: &Answer, case: &str) -> (String, String) {
    let request_id = answer
        .header("x-amz-request-id")
        .unwrap_or_else(|| panic!("case {case:?}: the response carried no x-amz-request-id"));
    let extended = answer
        .header("x-amz-id-2")
        .unwrap_or_else(|| panic!("case {case:?}: the response carried no x-amz-id-2"));
    (request_id, extended)
}

/// The fixed text S3's BUCKET tagging path states for a key in the namespace AWS keeps for
/// the tags it generates itself — the sentence the reserved-prefix suite
/// ([`super::reserved_tag_key_prefix`]) spans the forms of that key with.
const SYSTEM_TAGS_MESSAGE: &str = "System tags cannot be added/updated by requester";

/// The `Tagging` payload for one tag, as a tagging write carries it.
fn tagging_payload(key: &str, value: &str) -> String {
    format!(
        "<Tagging xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><TagSet><Tag>\
         <Key>{key}</Key><Value>{value}</Value></Tag></TagSet></Tagging>"
    )
}

#[tokio::test]
async fn every_response_names_its_request_by_an_s3_request_id() {
    let router = router();
    let name = "served-envelope-ids";
    bucket(&router, name).await;
    let written = put_object(&router, name, "ledger.bin", b"envelope-subject", &[]).await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );

    // (case, the answer, whether it carries an error document). The rows differ along the
    // operation and the CLASS of answer: three not-founds reached through different
    // subresources, a bad argument, a bad tag, and two served responses — because the pair
    // is a property of a response, not of a refusal.
    let rows: Vec<(&str, Answer, bool)> = vec![
        (
            "GetBucketIntelligentTieringConfiguration naming a configuration the bucket \
             does not carry",
            send(
                &router,
                "GET",
                &format!("/{name}?intelligent-tiering&id=absent"),
                &[],
                "",
            )
            .await,
            true,
        ),
        (
            "DeleteBucketIntelligentTieringConfiguration naming that same configuration",
            send(
                &router,
                "DELETE",
                &format!("/{name}?intelligent-tiering&id=absent"),
                &[],
                "",
            )
            .await,
            true,
        ),
        (
            "ListBucketIntelligentTieringConfigurations on a bucket that is not there",
            send(&router, "GET", "/served-envelope-absent?intelligent-tiering", &[], "").await,
            true,
        ),
        (
            "GetBucketCors on a bucket carrying no cors configuration",
            send(&router, "GET", &format!("/{name}?cors"), &[], "").await,
            true,
        ),
        (
            "GetObject on a key the bucket holds nothing at",
            send_bytes(&router, "GET", &format!("/{name}/never-written.bin"), &[], b"").await,
            true,
        ),
        (
            "GetObject naming a versionId that is not shaped like one",
            send_bytes(
                &router,
                "GET",
                &format!("/{name}/ledger.bin?versionId=not-a-version"),
                &[],
                b"",
            )
            .await,
            true,
        ),
        (
            "PutBucketTagging carrying a key in the reserved namespace",
            send(
                &router,
                "PUT",
                &format!("/{name}?tagging"),
                &[],
                &tagging_payload("aws:createdBy", "someone"),
            )
            .await,
            true,
        ),
        (
            "GetBucketVersioning, which is served",
            send(&router, "GET", &format!("/{name}?versioning"), &[], "").await,
            false,
        ),
        ("PutObject, which is served", written, false),
    ];

    let mut seen: Vec<(String, String)> = Vec::new();
    for (case, answer, refused) in &rows {
        assert_eq!(
            answer.status >= 400,
            *refused,
            "case {case:?}: expected a {} answer, got {} with body {:?}",
            if *refused { "refusal" } else { "served" },
            answer.status,
            text(answer)
        );
        let (request_id, extended) = correlation_headers(answer, case);
        assert!(
            is_request_id(&request_id),
            "case {case:?}: expected an {REQUEST_ID_CHARS}-character uppercase \
             alphanumeric request id, got {request_id:?}"
        );
        assert!(
            is_extended_request_id(&extended),
            "case {case:?}: expected an {EXTENDED_REQUEST_ID_CHARS}-character lowercase \
             hexadecimal extended request id, got {extended:?}"
        );
        if *refused {
            // The error document mirrors the two headers, so the id a client reads out of
            // the body is the one an operator reads off the response.
            assert!(
                !answer.error_code().trim().is_empty(),
                "case {case:?}: a refusal must carry an error document, got {:?}",
                text(answer)
            );
            assert_eq!(
                answer.element("RequestId").as_deref(),
                Some(request_id.as_str()),
                "case {case:?}: the envelope's RequestId must be the header's \
                 {request_id:?}, got {:?} in {:?}",
                answer.element("RequestId"),
                text(answer)
            );
            assert_eq!(
                answer.element("HostId").as_deref(),
                Some(extended.as_str()),
                "case {case:?}: the envelope's HostId must be the header's {extended:?}, \
                 got {:?} in {:?}",
                answer.element("HostId"),
                text(answer)
            );
        }
        // "A value created by Amazon S3 that uniquely identifies the request": no two of
        // these responses may be named by the same id.
        assert!(
            !seen
                .iter()
                .any(|(earlier, _)| earlier.as_str() == request_id.as_str()),
            "case {case:?}: the request id {request_id:?} already named an earlier \
             response, got {seen:?}"
        );
        seen.push((request_id, extended));
    }
}

/// One row of the refused-timestamp table: the case, the instant the request names, and the
/// rendering the envelope must echo.
///
/// Each rendering is the calendar date of that instant in the zone S3 writes these
/// documents in, in the `EEE MMM dd HH:mm:ss zzz yyyy` form, worked out here rather than by
/// re-running the code under test. The rows span what the rendering generalises over: the
/// daylight-saving abbreviation the instant falls in, an instant whose date in that zone is
/// the day BEFORE its UTC date, and the weekday and month names a calendar has to supply.
/// Every instant has gone by, which is what makes the retention period one the service
/// refuses.
fn refused_instants() -> Vec<(&'static str, i64, &'static str)> {
    vec![
        (
            "the epoch, whose date in the service's zone is the previous year's last day",
            0,
            "Wed Dec 31 16:00:00 PST 1969",
        ),
        (
            "an instant inside the daylight-saving span, on the previous day there",
            1_000_000_000,
            "Sat Sep 08 18:46:40 PDT 2001",
        ),
        (
            "an instant late in a day, so the time of day is two digits throughout",
            1_234_567_890,
            "Fri Feb 13 15:31:30 PST 2009",
        ),
        (
            "midnight on a leap day, which only a real calendar renders",
            951_782_400,
            "Mon Feb 28 16:00:00 PST 2000",
        ),
        (
            "an instant in a recent year",
            1_700_000_000,
            "Tue Nov 14 14:13:20 PST 2023",
        ),
    ]
}

/// The spellings a request may carry one instant in: the canonical ISO 8601 `date-time`
/// (`ObjectLockRetention$RetainUntilDate`'s `timestampFormat`), the same with a fraction of
/// a second, and the same with a fraction that is explicitly zero. The rendering is of the
/// INSTANT, so all three are refused with one value.
fn spellings(epoch_seconds: i64) -> Vec<(&'static str, String)> {
    let canonical = aws_smithy_types::DateTime::from_secs(epoch_seconds)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("the table's own instant renders as an ISO 8601 date-time");
    let without_zone = canonical.trim_end_matches('Z').to_string();
    vec![
        ("the canonical date-time", canonical),
        (
            "a fraction of a second",
            format!("{without_zone}.250Z"),
        ),
        (
            "a fraction that is explicitly zero",
            format!("{without_zone}.000Z"),
        ),
    ]
}

/// Assert `answer` is the `InvalidArgument` refusal naming `argument` and echoing `value`.
fn assert_echoes_argument(answer: &Answer, case: &str, argument: &str, value: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {:?}",
        answer.status,
        text(answer)
    );
    assert_eq!(
        answer.error_code(),
        "InvalidArgument",
        "case {case:?}: expected the InvalidArgument code, got {:?}",
        answer.error_code()
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
        "case {case:?}: expected the rejected date rendered as {value:?}, got {:?} in {:?}",
        answer.element("ArgumentValue"),
        text(answer)
    );
}

#[tokio::test]
async fn a_rejected_timestamp_is_echoed_as_the_service_renders_a_date() {
    let router = router();
    let name = "served-envelope-retention";
    lock_enabled_bucket(&router, name).await;
    let written = put_object(&router, name, "ledger.bin", b"envelope-subject", &[]).await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );

    for (instant_case, epoch_seconds, rendered) in refused_instants() {
        for (spelling_case, spelled) in spellings(epoch_seconds) {
            let case = format!("{instant_case} / {spelling_case}");

            // In the `Retention` DOCUMENT of the retention write, where the member's own
            // name is the argument.
            let payload = format!(
                "<Retention xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                 <Mode>{GOVERNANCE_RETENTION_MODE}</Mode>\
                 <RetainUntilDate>{spelled}</RetainUntilDate></Retention>"
            );
            let refused =
                put_object_retention(&router, name, "ledger.bin", "", &[], &payload).await;
            assert_echoes_argument(
                &refused,
                &format!("{case}/PutObjectRetention"),
                "RetainUntilDate",
                rendered,
            );

            // And in the `x-amz-object-lock-retain-until-date` HEADER of the writes that
            // carry one, where the header itself is the argument.
            let headers = vec![
                ("x-amz-object-lock-mode", GOVERNANCE_RETENTION_MODE),
                ("x-amz-object-lock-retain-until-date", spelled.as_str()),
            ];
            for (call, answer) in [
                (
                    "PutObject",
                    put_object(&router, name, "locked-on-write.bin", b"body", &headers).await,
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
                (
                    "CopyObject",
                    send_bytes(
                        &router,
                        "PUT",
                        &format!("/{name}/locked-copy.bin"),
                        &[
                            ("x-amz-copy-source", &format!("{name}/ledger.bin")),
                            ("x-amz-object-lock-mode", GOVERNANCE_RETENTION_MODE),
                            ("x-amz-object-lock-retain-until-date", spelled.as_str()),
                        ],
                        b"",
                    )
                    .await,
                ),
            ] {
                assert_echoes_argument(
                    &answer,
                    &format!("{case}/{call}"),
                    "x-amz-object-lock-retain-until-date",
                    rendered,
                );
            }
        }
    }

    // A period that ends in the FUTURE is served, so every row above was refused for the
    // instant it named and not for carrying a date at all.
    let future = aws_smithy_types::DateTime::from_secs(
        aws_smithy_types::DateTime::from(std::time::SystemTime::now()).secs() + 24 * 3_600,
    )
    .fmt(aws_smithy_types::date_time::Format::DateTime)
    .expect("the test's own instant renders as an ISO 8601 date-time");
    let placed = put_object_retention(
        &router,
        name,
        "ledger.bin",
        "",
        &[],
        &format!(
            "<Retention xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <Mode>{GOVERNANCE_RETENTION_MODE}</Mode><RetainUntilDate>{future}</RetainUntilDate>\
             </Retention>"
        ),
    )
    .await;
    assert_eq!(
        placed.status, 200,
        "a period that ends in the future must be served, got {} with body {:?}",
        placed.status,
        text(&placed)
    );
}

/// One row of the refused-tag table: the case, the tag set the request carries, the code
/// and sentence it earns, and the `<TagKey>` / `<TagValue>` the envelope must name.
type RefusedTagRow = (
    &'static str,
    Vec<(String, String)>,
    &'static str,
    String,
    Option<String>,
    Option<String>,
);

/// Every row of that table, built from the PUBLISHED bounds and character class rather
/// than from literals, so a row cannot drift from the rule a tag set is judged by.
///
/// The rows differ along WHICH restriction the tag broke and therefore which half of the
/// tag the refusal is about: the key's own bounds and its character class, the reserved
/// namespace, the key the set names twice, the value's bound and character class, and a set
/// refused for its SIZE — which is about no one tag at all.
fn refused_tag_rows() -> Vec<RefusedTagRow> {
    let tag = |key: &str, value: &str| (key.to_string(), value.to_string());
    let over_long_key = "k".repeat(MAX_TAG_KEY_CHARS + 1);
    let over_long_value = "v".repeat(MAX_TAG_VALUE_CHARS + 1);
    // A currency sign is neither a letter, a digit, whitespace nor one of the published
    // punctuation marks (`service_com_amazonaws_s3_amazons3::state::TAG_PUNCTUATION`).
    let outside_class = "owner\u{20ac}";
    vec![
        (
            "a key in the reserved namespace",
            vec![tag("aws:createdBy", "someone")],
            "InvalidTag",
            SYSTEM_TAGS_MESSAGE.to_string(),
            Some("aws:createdBy".to_string()),
            None,
        ),
        (
            "a key the set names twice",
            vec![tag("owner", "platform"), tag("owner", "finance")],
            "InvalidTag",
            "Cannot provide multiple Tags with the same key".to_string(),
            Some("owner".to_string()),
            None,
        ),
        (
            "a key past the published length",
            vec![tag(&over_long_key, "v")],
            "InvalidTag",
            "The TagKey you have provided is invalid".to_string(),
            Some(over_long_key.clone()),
            None,
        ),
        (
            "a key carrying a character outside the published class",
            vec![tag(outside_class, "platform")],
            "InvalidTag",
            "The TagKey you have provided is invalid".to_string(),
            Some(outside_class.to_string()),
            None,
        ),
        (
            "a value past the published length",
            vec![tag("owner", &over_long_value)],
            "InvalidTag",
            "The TagValue you have provided is invalid".to_string(),
            Some("owner".to_string()),
            Some(over_long_value.clone()),
        ),
        (
            "a value carrying a character outside the published class",
            vec![tag("owner", outside_class)],
            "InvalidTag",
            "The TagValue you have provided is invalid".to_string(),
            Some("owner".to_string()),
            Some(outside_class.to_string()),
        ),
        (
            "more tags than the bucket may hold",
            (0..=MAX_TAGS_PER_BUCKET)
                .map(|position| tag(&format!("key-{position}"), "v"))
                .collect(),
            "BadRequest",
            format!("Bucket tags cannot be greater than {MAX_TAGS_PER_BUCKET}"),
            None,
            None,
        ),
    ]
}

#[tokio::test]
async fn a_refused_tag_is_named_by_the_envelope_that_refuses_it() {
    let router = router();
    let name = "served-envelope-tagging";
    bucket(&router, name).await;

    for (case, tags, code, message, tag_key, tag_value) in refused_tag_rows() {
        let tag_set: String = tags
            .iter()
            .map(|(key, value)| {
                format!("<Tag><Key>{key}</Key><Value>{value}</Value></Tag>")
            })
            .collect();
        let refused = send(
            &router,
            "PUT",
            &format!("/{name}?tagging"),
            &[],
            &format!(
                "<Tagging xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><TagSet>{tag_set}\
                 </TagSet></Tagging>"
            ),
        )
        .await;

        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the code {code:?}, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            message,
            "case {case:?}: expected the sentence {message:?}, got {:?}",
            refused.error_message()
        );
        assert_eq!(
            refused.element("TagKey"),
            tag_key,
            "case {case:?}: expected the envelope to name the tag key {tag_key:?}, got {:?} \
             in {:?}",
            refused.element("TagKey"),
            text(&refused)
        );
        assert_eq!(
            refused.element("TagValue"),
            tag_value,
            "case {case:?}: expected the envelope to name the tag value {tag_value:?}, got \
             {:?} in {:?}",
            refused.element("TagValue"),
            text(&refused)
        );
    }

    // A tag set every restriction admits is still stored, so every row above was refused
    // for the restriction it broke rather than for being a tag set at all.
    let served = send(
        &router,
        "PUT",
        &format!("/{name}?tagging"),
        &[],
        &format!(
            "<Tagging xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><TagSet>{}</TagSet>\
             </Tagging>",
            "<Tag><Key>owner</Key><Value>platform</Value></Tag>"
        ),
    )
    .await;
    assert_eq!(
        served.status, 204,
        "a tag set within every published restriction must be stored, got {} with body {:?}",
        served.status,
        text(&served)
    );
}

/// The text of every `<Name>` element inside the FIRST `container` element of `body`, in
/// document order — the filter rule names one destination configuration reports.
fn reported_names(body: &str, container: &str) -> Vec<String> {
    let Some((_, rest)) = body.split_once(&format!("<{container}>")) else {
        return Vec::new();
    };
    let inside = rest
        .split_once(&format!("</{container}>"))
        .map_or(rest, |(inside, _)| inside);
    inside
        .split("<Name>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</Name>"))
        .map(|(name, _)| name.to_string())
        .collect()
}

/// One destination kind: the element its configuration is reported in, the list it is held
/// in and an ARN of the service it publishes to.
type DestinationRow = (&'static str, &'static str, &'static str);
/// The three destination kinds a notification configuration publishes to, each with the
/// element `GetBucketNotificationConfiguration` reports it in — `LambdaFunctionConfigurations`
/// serializes as `CloudFunctionConfiguration`
/// (`smithy_query(query="shape",shape="NotificationConfiguration")`).
const DESTINATIONS: [DestinationRow; 3] = [
    (
        "topic",
        "TopicConfiguration",
        "arn:aws:sns:us-east-1:000000000000:served-envelope-topic",
    ),
    (
        "queue",
        "QueueConfiguration",
        "arn:aws:sqs:us-east-1:000000000000:served-envelope-queue",
    ),
    (
        "lambda function",
        "CloudFunctionConfiguration",
        "arn:aws:lambda:us-east-1:000000000000:function:served-envelope-function",
    ),
];

#[tokio::test]
async fn a_stored_filter_rule_name_is_reported_in_its_canonical_spelling() {
    // The configuration is seeded through the STORE rather than put over the wire, so what
    // is under test is what the READ reports for a rule held under any spelling — the rule
    // the issue states: the name is normalised on output, however it was written.
    let (router, engine) = router_with_engine();
    let name = "served-envelope-notifications";
    bucket(&router, name).await;

    // (case, the name the store holds). The rows differ along the SPELLING held: each
    // published `FilterRuleName` value (the request vocabulary), its canonical capitalised
    // form, an upper-case spelling of it, and a name that is no `FilterRuleName` at all —
    // which has no canonical spelling, so it is reported as it is held.
    let mut held: Vec<(String, String)> = Vec::new();
    for published in FILTER_RULE_NAME_VALUES {
        let canonical = canonical_filter_rule_name(published)
            .unwrap_or_else(|| panic!("the published FilterRuleName {published:?} has a \
                                       canonical spelling"));
        held.push((format!("the published value {published:?}"), (*published).to_string()));
        held.push((format!("the canonical spelling {canonical:?}"), canonical.to_string()));
        held.push((
            format!("an upper-case spelling of {published:?}"),
            published.to_uppercase(),
        ));
    }
    held.push((
        "a name that is no FilterRuleName".to_string(),
        "extension".to_string(),
    ));

    for (case, stored_name) in &held {
        let expected = canonical_filter_rule_name(stored_name)
            .map_or_else(|| stored_name.clone(), str::to_string);
        for (destination, element, arn) in DESTINATIONS {
            let target = NotificationTarget {
                id: Some(format!("{destination}-rule")),
                arn: arn.to_string(),
                events: vec!["s3:ObjectCreated:*".to_string()],
                filter_rules: Some(vec![NotificationFilterRule {
                    name: Some(stored_name.clone()),
                    value: Some("raw/".to_string()),
                }]),
            };
            let mut configuration = BucketNotificationConfiguration::default();
            match element {
                "TopicConfiguration" => configuration.topic_configurations = vec![target],
                "QueueConfiguration" => configuration.queue_configurations = vec![target],
                _ => configuration.lambda_function_configurations = vec![target],
            }
            seed_notification_configuration(&engine, name, configuration);

            let read = send(&router, "GET", &format!("/{name}?notification"), &[], "").await;
            assert_eq!(
                read.status, 200,
                "case {case:?}/{destination}: the read must be served, got {} with body {:?}",
                read.status,
                text(&read)
            );
            let names = reported_names(&text(&read), element);
            assert_eq!(
                names,
                vec![expected.clone()],
                "case {case:?}/{destination}: expected the reported rule name \
                 {expected:?}, got {names:?} in {:?}",
                text(&read)
            );
        }
    }
}

/// Record `configuration` as the notification configuration of `bucket`, through the store
/// the service reads — the one seam a suite reaches past the wire with
/// ([`super::bucket_lifecycle::router_with_engine`] hands it over).
fn seed_notification_configuration(
    engine: &Arc<engine::InMemoryEngine>,
    bucket: &str,
    configuration: BucketNotificationConfiguration,
) {
    let state = engine.state::<ComAmazonawsS3Amazons3State>();
    let mut store = state.lock();
    assert_eq!(
        store.put_notification_configuration(bucket, configuration),
        Some(()),
        "the bucket {bucket:?} must hold the seeded notification configuration"
    );
}
