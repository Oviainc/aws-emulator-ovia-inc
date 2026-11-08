// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a bucket's CREATE, its LISTING and the names a request may
//! address a bucket by are answered with.
//!
//! Every assertion drives a real HTTP request through the router (the harness
//! [`super::bucket_lifecycle`] owns), so what is pinned is the wire document — status,
//! headers, body — rather than any handler's internals.
//!
//! The rules under test, each with its source:
//!
//! * a successful `CreateBucket` reports the new bucket's ARN in the `x-amz-bucket-arn`
//!   header its `BucketArn` member is bound to
//!   (`smithy_query(query="operation",operation="CreateBucket")`), which for a general
//!   purpose bucket is `arn:aws:s3:::<bucket>` — the form every other reader of this
//!   service reports for the same bucket;
//! * a create carrying `CreateBucketConfiguration$Tags`, "an array of tags that you can
//!   apply to the bucket that you're creating", brings the bucket into being carrying that
//!   tag set, which `GetBucketTagging` then reports;
//! * a `LocationConstraint` outside the Regions that member publishes
//!   (`smithy_query(query="shape",shape="BucketLocationConstraint")`) names no location a
//!   bucket can be created in, and is refused with the code, the sentence and the
//!   `<LocationConstraint>` member S3's published error list states for that class — not as
//!   a document that does not validate against the published schema;
//! * every instant `ListBuckets` reports is in the fixed-width form S3 writes an instant
//!   in, to the millisecond (`Bucket$CreationDate` is a `timestamp` whose restXml format is
//!   `date-time`);
//! * the `ContinuationToken` is a NAME cursor: "the next `ListBuckets` request to Amazon S3
//!   can be continued with this ContinuationToken" (`ListBucketsOutput$ContinuationToken`),
//!   so the page after one begins at the next bucket name in order — whatever the account
//!   holds by then — and a token is carried exactly while buckets remain;
//! * a request addressing a bucket by a name S3 holds no bucket under is refused for the
//!   NAME (`InvalidBucketName`, HTTP 400, naming the rejected name in `<BucketName>`) rather
//!   than reported as a bucket that is absent, whichever operation it names;
//! * a `max-buckets` that bounds no page and an `x-amz-object-ownership` outside the values
//!   its member publishes are refused with the sentence S3 states for each and the
//!   `<ArgumentName>` / `<ArgumentValue>` pair naming the field and the value as sent.

use std::collections::BTreeSet;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_s3_amazons3::service::wire::DOCUMENT_TIMESTAMP_ELEMENTS;
use service_com_amazonaws_s3_amazons3::state::{
    INVALID_BUCKET_NAME_CODE, INVALID_BUCKET_NAME_MESSAGE, INVALID_LOCATION_CONSTRAINT_CODE,
    INVALID_LOCATION_CONSTRAINT_MESSAGE, MAX_BUCKETS_BOUNDS,
};
use service_common::s3_bucket_name::MAX_BUCKET_NAME_CHARS;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{
    constraint_region, create_bucket, head_bucket, router, send, send_bytes_to_host,
    send_in_region, Answer,
};
use super::bucket_tagging::{get_bucket_tagging, tag, tag_set_of};
use super::list_buckets::list_buckets;
use super::object_and_tagging_roundtrip::{put_object, text};
use super::object_storage::{list_objects_v2, listing_of, Listing};

/// The XML namespace a client's writer puts on a `CreateBucketConfiguration`.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// `PUT /<bucket>` carrying a `CreateBucketConfiguration` with the location constraint
/// (when any) and the tag set (when any) — the create a client that tags at creation sends.
///
/// Addressed to the endpoint of the Region the constraint names, the way
/// [`super::bucket_lifecycle::create_bucket`] is: an endpoint for one Region cannot place a
/// bucket in another.
async fn create_bucket_with_tags(
    router: &MultiServiceRouter,
    bucket: &str,
    location_constraint: Option<&str>,
    tags: &[(String, String)],
) -> Answer {
    let mut configuration = format!("<CreateBucketConfiguration xmlns=\"{XMLNS}\">");
    if let Some(constraint) = location_constraint {
        configuration.push_str(&format!(
            "<LocationConstraint>{constraint}</LocationConstraint>"
        ));
    }
    if !tags.is_empty() {
        // The wrapped list layout the model declares for `CreateBucketConfiguration$Tags`
        // (a `TagSet` whose member is spelled `Tag`, with no `@xmlFlattened`).
        configuration.push_str("<Tags>");
        for (key, value) in tags {
            configuration.push_str(&format!(
                "<Tag><Key>{key}</Key><Value>{value}</Value></Tag>"
            ));
        }
        configuration.push_str("</Tags>");
    }
    configuration.push_str("</CreateBucketConfiguration>");
    send_in_region(
        router,
        &constraint_region(location_constraint),
        "PUT",
        &format!("/{bucket}"),
        &[],
        &configuration,
    )
    .await
}

/// The header `CreateBucketOutput$BucketArn` is bound to.
const BUCKET_ARN_HEADER: &str = "x-amz-bucket-arn";

/// The ARN a general purpose bucket is named by: its globally unique name alone, with the
/// Region and account-id fields of the ARN empty.
fn general_purpose_bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// One row of the create table: the case, the bucket the create names, the location
/// constraint it carries (when any) and the tags it carries.
type CreateRow<'a> = (&'a str, &'a str, Option<&'a str>, &'a [(&'a str, &'a str)]);

#[tokio::test]
async fn a_create_reports_the_new_buckets_arn_and_the_tags_it_asked_for_read_back() {
    let router = router();

    // The rows differ along what the two rules generalise over: how many tags the set
    // carries (none, one, several), what their keys and values hold (a space, a colon, an
    // empty value, a non-ASCII character), and which Region the bucket is created in — the
    // ARN of a general purpose bucket carries no Region, so a row in another Region is what
    // pins that.
    let rows: &[CreateRow<'_>] = &[
        ("no tags at all", "arn-plain", None, &[]),
        ("one tag", "arn-one-tag", None, &[("Environment", "test")]),
        (
            "the four-tag set a configuring client applies",
            "arn-four-tags",
            None,
            &[
                ("Environment", "production"),
                ("Owner", "platform team"),
                ("CostCenter", "cc:4711"),
                ("Retention", ""),
            ],
        ),
        (
            "tags beside a location constraint, in another Region",
            "arn-regional-tags",
            Some("ap-southeast-7"),
            &[("Zone", "regional"), ("Sütterlin", "ümlaut")],
        ),
        (
            "tags beside the legacy EU constraint",
            "arn-legacy-eu-tags",
            Some("EU"),
            &[("Legacy", "EU")],
        ),
    ];

    for (case, bucket, constraint, tags) in rows {
        let owned: Vec<(String, String)> = tags.iter().map(|(key, value)| tag(key, value)).collect();
        let created = create_bucket_with_tags(&router, bucket, *constraint, &owned).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must succeed, got {} with body {:?}",
            created.status,
            text(&created)
        );
        assert_eq!(
            created.header(BUCKET_ARN_HEADER).as_deref(),
            Some(general_purpose_bucket_arn(bucket).as_str()),
            "case {case:?}: the create must report the new bucket's ARN, got {:?}",
            created.header(BUCKET_ARN_HEADER)
        );

        // The tag set the create asked for, read back through the reader that reports a
        // bucket's tags: present and complete when the create carried one, and the
        // "this bucket carries no tag set" refusal when it carried none.
        let reported = get_bucket_tagging(&router, bucket).await;
        if owned.is_empty() {
            assert_eq!(
                reported.status, 404,
                "case {case:?}: a bucket created with no tags carries none, got {} with \
                 body {:?}",
                reported.status,
                text(&reported)
            );
            assert_eq!(
                reported.error_code(),
                "NoSuchTagSet",
                "case {case:?}: expected the NoSuchTagSet code, got {:?}",
                reported.error_code()
            );
            continue;
        }
        assert_eq!(
            reported.status, 200,
            "case {case:?}: the tags the create carried must read back, got {} with body {:?}",
            reported.status,
            text(&reported)
        );
        assert_eq!(
            tag_set_of(&reported),
            owned,
            "case {case:?}: expected the tag set the create carried, got {:?}",
            tag_set_of(&reported)
        );
    }

    // The ARN is reported by the LEGACY re-create too — "if you re-create an existing
    // bucket that you already own in the North Virginia Region, Amazon S3 returns 200 OK"
    // (`BucketAlreadyOwnedByYou`) — so a client that re-creates a bucket to learn its ARN
    // reads the same string.
    let again = create_bucket(&router, "arn-plain", None, None).await;
    assert_eq!(
        again.status, 200,
        "case \"the legacy re-create\": expected 200, got {} with body {:?}",
        again.status,
        text(&again)
    );
    assert_eq!(
        again.header(BUCKET_ARN_HEADER).as_deref(),
        Some(general_purpose_bucket_arn("arn-plain").as_str()),
        "case \"the legacy re-create\": expected the bucket's ARN, got {:?}",
        again.header(BUCKET_ARN_HEADER)
    );

    // And the tags a create applied are a bucket's own tag set, not a second store: a
    // later read of the listing still reports the bucket, and the tag set survives it.
    let listed = list_buckets(&router, "").await;
    for (case, bucket, _, _) in rows {
        assert!(
            text(&listed).contains(&format!("<Name>{bucket}</Name>")),
            "case {case:?}: the created bucket must be listed, got {:?}",
            text(&listed)
        );
    }
}

#[tokio::test]
async fn a_create_whose_tag_set_s3_refuses_records_no_bucket() {
    let router = router();

    // A tag set the published rules refuse — one key twice — is refused, and the refused
    // create leaves no bucket behind, so nothing of it is half applied.
    let duplicated = vec![tag("Environment", "test"), tag("Environment", "production")];
    let created = create_bucket_with_tags(&router, "create-duplicate-tag", None, &duplicated).await;
    assert_eq!(
        created.status, 400,
        "case \"one key twice\": expected HTTP 400, got {} with body {:?}",
        created.status,
        text(&created)
    );
    assert_eq!(
        head_bucket(&router, "create-duplicate-tag").await.status,
        404,
        "case \"one key twice\": a refused create must leave no bucket behind"
    );
}

/// The location constraints `BucketLocationConstraint` publishes, read from the model
/// (`smithy_query(query="shape",shape="BucketLocationConstraint")`): a create naming one of
/// them is served. `us-east-1` is deliberately NOT among the published values — that Region
/// is the one an ABSENT constraint means — and neither is any spelling of a Region in
/// another case.
const PUBLISHED_LOCATION_CONSTRAINTS: &[&str] = &["EU", "ap-southeast-7", "mx-central-1"];

#[tokio::test]
async fn a_location_constraint_that_names_no_region_is_refused_as_an_invalid_constraint() {
    let router = router();

    // Values outside the published set, differing along WHY each is outside it: the Region
    // only an absent constraint can mean, a Region of another partition's shape that does
    // not exist, a published value in the wrong case, a value that is not a Region code at
    // all, and the empty constraint.
    for (case, constraint) in [
        ("the Region an absent constraint means", "us-east-1"),
        ("a Region code nothing publishes", "eu-west-9"),
        ("a published value in upper case", "US-WEST-2"),
        ("a published value with a stray space", "us-west-2 "),
        ("text that is not a Region code", "somewhere-else"),
    ] {
        let bucket = "constraint-probe";
        // Sent to one endpoint for every row: the value is refused for what it names, not
        // for which endpoint the request reached.
        let created = send_in_region(
            &router,
            "us-east-1",
            "PUT",
            &format!("/{bucket}"),
            &[],
            &format!(
                "<CreateBucketConfiguration xmlns=\"{XMLNS}\">\
                 <LocationConstraint>{constraint}</LocationConstraint>\
                 </CreateBucketConfiguration>"
            ),
        )
        .await;
        assert_eq!(
            created.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            created.status,
            text(&created)
        );
        assert_eq!(
            created.error_code(),
            INVALID_LOCATION_CONSTRAINT_CODE,
            "case {case:?}: expected the code {INVALID_LOCATION_CONSTRAINT_CODE:?}, got {:?}",
            created.error_code()
        );
        assert_eq!(
            created.error_message(),
            INVALID_LOCATION_CONSTRAINT_MESSAGE,
            "case {case:?}: expected the message {INVALID_LOCATION_CONSTRAINT_MESSAGE:?}, \
             got {:?}",
            created.error_message()
        );
        assert_eq!(
            created.element("LocationConstraint").as_deref(),
            Some(constraint),
            "case {case:?}: the document must name the rejected constraint, got {:?} in {:?}",
            created.element("LocationConstraint"),
            text(&created)
        );
        // Refused BEFORE anything is written: no bucket is there afterwards.
        assert_eq!(
            head_bucket(&router, bucket).await.status,
            404,
            "case {case:?}: a refused create must leave no bucket behind"
        );
    }

    // Every PUBLISHED value is served, so the refusals above are about the value naming no
    // Region and not about a constraint being present at all.
    for constraint in PUBLISHED_LOCATION_CONSTRAINTS {
        let bucket = format!("constraint-ok-{}", constraint.to_lowercase());
        let created = create_bucket(&router, &bucket, Some(constraint), None).await;
        assert_eq!(
            created.status, 200,
            "case {constraint:?}: a published constraint must be served, got {} with body {:?}",
            created.status,
            text(&created)
        );
    }
}

/// Whether `rendered` is in the fixed-width form S3 writes an instant in:
/// `YYYY-MM-DDTHH:MM:SS.000Z` — 24 characters, whole seconds, and the three-digit fraction
/// every value S3 sends carries, since the instants it records have one-second resolution.
fn is_s3_instant(rendered: &str) -> bool {
    let shape = "0000-00-00T00:00:00.000Z";
    rendered.len() == shape.len()
        && rendered
            .chars()
            .zip(shape.chars())
            .all(|(character, expected)| match expected {
                '0' => character.is_ascii_digit(),
                _ => character == expected,
            })
        && rendered.ends_with(".000Z")
        && DateTime::from_str(rendered, Format::DateTime).is_ok()
}

#[tokio::test]
async fn every_instant_a_document_reports_is_in_the_fixed_width_millisecond_form() {
    let router = router();

    // The form is a property of the wire, not of one operation, so it is pinned on the two
    // documents that report an instant from the clock: a bucket's creation date and an
    // object's last-modified date.
    for bucket in ["instant-a", "instant-b", "instant-c"] {
        let created = create_bucket(&router, bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "the bucket {bucket:?} must be created, got {} with body {:?}",
            created.status,
            text(&created)
        );
    }

    let listing = list_buckets(&router, "").await;
    let document = text(&listing);
    let dates: Vec<&str> = document
        .split("<CreationDate>")
        .skip(1)
        .filter_map(|chunk| chunk.split("</CreationDate>").next())
        .collect();
    assert_eq!(
        dates.len(),
        3,
        "the listing must report one CreationDate per bucket, got {document:?}"
    );
    for date in &dates {
        assert!(
            is_s3_instant(date),
            "case \"a bucket CreationDate\": expected the fixed-width millisecond form, got \
             {date:?}"
        );
    }

    // A listing is a READ, so the same instant is reported again, in the same form.
    let again = text(&list_buckets(&router, "").await);
    for date in &dates {
        assert!(
            again.contains(&format!("<CreationDate>{date}</CreationDate>")),
            "case \"a second listing\": expected {date:?} again, got {again:?}"
        );
    }

    // Every size of payload lands on the same form, including one whose write happens to
    // fall on a whole millisecond.
    for (case, key, body) in [
        ("an empty object", "empty.txt", &b""[..]),
        ("a small object", "small.txt", &b"payload"[..]),
        ("a larger object", "larger.bin", &[7u8; 4096][..]),
    ] {
        let written = put_object(&router, "instant-a", key, body, &[]).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the write must succeed, got {} with body {:?}",
            written.status,
            text(&written)
        );
    }
    let objects = listing_of(&list_objects_v2(&router, "instant-a", "").await);
    assert_eq!(
        objects.keys().len(),
        3,
        "the listing must report every written object, got {:?}",
        objects.keys()
    );
    for entry in &objects.contents {
        let key = Listing::entry_field(entry, "Key");
        let modified = Listing::entry_field(entry, "LastModified");
        assert!(
            is_s3_instant(&modified),
            "case \"the LastModified of {key:?}\": expected the fixed-width millisecond \
             form, got {modified:?}"
        );
    }
}

#[test]
fn the_document_timestamp_catalog_matches_the_model() {
    // Every element an operation's RESPONSE DOCUMENT reports a `timestamp` in and no REQUEST
    // document carries, read from the model itself rather than from a second hand-written
    // list: each shape's own members and the members of the shapes those reach, under the
    // element name restXml writes each with (its `@xmlName`, else the member's own name). A
    // member bound to a response HEADER is not in the document at all — such a header goes
    // out in the format its own binding states — so it is left out, as is an `@httpPayload`
    // whose target is a scalar, which IS the body rather than a document.
    //
    // The subtraction is the rule's own: an element a request document carries is a value the
    // CALLER sent and the service stored (a lifecycle rule's `Date`, an Object Lock
    // `RetainUntilDate`), and a stored configuration reads back as the caller wrote it, so
    // only the instants S3 itself records are rendered in S3's own form.
    let model_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-s3-amazons3.json"
    );
    let model: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(model_path).expect("the service's Smithy model must be readable"),
    )
    .expect("the Smithy model must parse as JSON");
    let shapes = model["shapes"]
        .as_object()
        .expect("the model must carry a shapes object");

    /// The element a member is written under: its `@xmlName`, else its own name.
    fn element_of(name: &str, member: &serde_json::Value) -> String {
        member["traits"]["smithy.api#xmlName"]
            .as_str()
            .unwrap_or(name)
            .to_string()
    }

    /// Every element a `timestamp` is reported in, walked breadth-first from the operation
    /// shapes' `side` ("output" or "input") and following each member's target, so a
    /// timestamp nested in a listing's entry (or in that entry's own container) is found.
    fn timestamp_elements(
        shapes: &serde_json::Map<String, serde_json::Value>,
        side: &str,
    ) -> BTreeSet<String> {
        let mut pending: Vec<(String, bool)> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(_, shape)| shape[side]["target"].as_str())
            .map(|target| (target.to_string(), true))
            .collect();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut found: BTreeSet<String> = BTreeSet::new();
        while let Some((shape_id, is_bound)) = pending.pop() {
            if !seen.insert(shape_id.clone()) {
                continue;
            }
            let Some(shape) = shapes.get(&shape_id) else {
                continue;
            };
            // A list or map carries its element shape rather than members.
            for target in [
                shape["member"]["target"].as_str(),
                shape["value"]["target"].as_str(),
            ]
            .into_iter()
            .flatten()
            {
                pending.push((target.to_string(), false));
            }
            let Some(members) = shape["members"].as_object() else {
                continue;
            };
            for (name, member) in members {
                let bound_outside_the_body = member["traits"]
                    .get("smithy.api#httpHeader")
                    .or_else(|| member["traits"].get("smithy.api#httpPrefixHeaders"))
                    .or_else(|| member["traits"].get("smithy.api#httpResponseCode"))
                    .or_else(|| member["traits"].get("smithy.api#httpLabel"))
                    .or_else(|| member["traits"].get("smithy.api#httpQuery"))
                    .is_some();
                if is_bound && bound_outside_the_body {
                    continue;
                }
                let Some(target) = member["target"].as_str() else {
                    continue;
                };
                let kind = shapes
                    .get(target)
                    .and_then(|shape| shape["type"].as_str())
                    .unwrap_or_default();
                if kind == "timestamp" {
                    found.insert(element_of(name, member));
                    continue;
                }
                // An `@httpPayload` that is a scalar IS the body, so nothing below it is a
                // document element.
                if is_bound
                    && member["traits"].get("smithy.api#httpPayload").is_some()
                    && !matches!(kind, "structure" | "union" | "list" | "map")
                {
                    continue;
                }
                pending.push((target.to_string(), false));
            }
        }
        found
    }

    let stored_by_the_caller = timestamp_elements(shapes, "input");
    let derived: BTreeSet<String> = timestamp_elements(shapes, "output")
        .into_iter()
        .filter(|element| !stored_by_the_caller.contains(element))
        .collect();

    let declared: BTreeSet<String> = DOCUMENT_TIMESTAMP_ELEMENTS
        .iter()
        .map(|element| (*element).to_string())
        .collect();
    assert_eq!(
        declared, derived,
        "DOCUMENT_TIMESTAMP_ELEMENTS must name exactly the elements the model reports an \
         instant S3 itself recorded in; declared {declared:?}, the model reports {derived:?}"
    );
}

#[tokio::test]
async fn the_continuation_token_is_a_name_cursor_into_the_listing_as_it_stands() {
    // (case, the names held when the walk starts, the name created after the first page,
    // the names the walk must then report in order).
    //
    // The rows differ along where the mid-walk name falls relative to the cursor: between
    // the page that was served and the one that follows, before everything the walk has
    // already reported, and after everything it has left — which is what a NAME cursor
    // generalises over.
    let rows: &[(&str, &[&str], &str, &[&str])] = &[
        (
            "a name between the page served and the next one",
            &["walk-bucket1", "walk-bucket3"],
            "walk-bucket2",
            &["walk-bucket1", "walk-bucket2", "walk-bucket3"],
        ),
        (
            "a name before everything already reported",
            &["walk-bucket5", "walk-bucket6"],
            "walk-bucket0",
            &["walk-bucket5", "walk-bucket6"],
        ),
        (
            "a name after everything left",
            &["walk-bucket1", "walk-bucket2"],
            "walk-bucket9",
            &["walk-bucket1", "walk-bucket2", "walk-bucket9"],
        ),
    ];

    for (case, held, inserted, expected) in rows {
        let router = router();
        for name in *held {
            let created = create_bucket(&router, name, None, None).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: {name:?} must be created, got {} with body {:?}",
                created.status,
                text(&created)
            );
        }

        // One bucket per page, so every page but the last hands back a cursor.
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut inserted_yet = false;
        for page in 0..expected.len() + 2 {
            let query = match &token {
                None => "&max-buckets=1".to_string(),
                Some(token) => format!("&max-buckets=1&continuation-token={token}"),
            };
            let answer = list_buckets(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}, page {page}: expected 200, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            let document = text(&answer);
            let names: Vec<String> = document
                .split("<Name>")
                .skip(1)
                .filter_map(|chunk| chunk.split("</Name>").next())
                .map(str::to_string)
                .collect();
            walked.extend(names.clone());

            let next = document
                .split("<ContinuationToken>")
                .nth(1)
                .and_then(|chunk| chunk.split("</ContinuationToken>").next())
                .map(str::to_string);

            // The cursor is REUSABLE: sent again it answers the same page, because it
            // names a point in the listing's order rather than a step of one walk.
            if let Some(token) = &token {
                let repeated = list_buckets(
                    &router,
                    &format!("&max-buckets=1&continuation-token={token}"),
                )
                .await;
                assert_eq!(
                    text(&repeated)
                        .split("<Name>")
                        .skip(1)
                        .filter_map(|chunk| chunk.split("</Name>").next())
                        .map(str::to_string)
                        .collect::<Vec<String>>(),
                    names,
                    "case {case:?}, page {page}: the same token must answer the same page"
                );
            }

            // The bucket that arrives mid-walk, created after the FIRST page was served.
            if !inserted_yet {
                let created = create_bucket(&router, inserted, None, None).await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: {inserted:?} must be created mid-walk, got {} with body \
                     {:?}",
                    created.status,
                    text(&created)
                );
                inserted_yet = true;
            }

            match next {
                Some(next) if !next.is_empty() => token = Some(next),
                _ => break,
            }
        }

        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: the walk must report each name in order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_token_is_carried_exactly_while_buckets_remain_whatever_the_page_holds() {
    let router = router();
    for name in ["token-a", "token-b", "token-c"] {
        assert_eq!(
            create_bucket(&router, name, None, None).await.status,
            200,
            "the bucket {name:?} must be created"
        );
    }

    // (case, the page size, how many pages the walk takes).
    for (case, page_size, pages) in [
        ("a page per bucket", 1, 3),
        ("two buckets a page", 2, 2),
        ("a page the account fits in", 3, 1),
        ("a page larger than the account holds", 7, 1),
    ] {
        let mut token: Option<String> = None;
        let mut walked = 0usize;
        for page in 0..pages {
            let query = match &token {
                None => format!("&max-buckets={page_size}"),
                Some(token) => format!("&max-buckets={page_size}&continuation-token={token}"),
            };
            let document = text(&list_buckets(&router, &query).await);
            walked += document.matches("<Name>").count();
            let next = document
                .split("<ContinuationToken>")
                .nth(1)
                .and_then(|chunk| chunk.split("</ContinuationToken>").next())
                .map(str::to_string);
            let last_page = page + 1 == pages;
            assert_eq!(
                next.is_none(),
                last_page,
                "case {case:?}, page {page}: a token is carried exactly while buckets \
                 remain, got {next:?} after {walked} of 3 buckets"
            );
            token = next;
        }
        assert_eq!(
            walked, 3,
            "case {case:?}: the walk must report every bucket once, got {walked}"
        );
    }
}

#[tokio::test]
async fn a_page_bound_or_an_ownership_setting_s3_cannot_serve_names_the_argument_it_refused() {
    let router = router();
    assert_eq!(
        create_bucket(&router, "argument-probe-bucket", None, None)
            .await
            .status,
        200,
        "the bucket must be created first"
    );

    // `MaxBuckets` is "the maximum number of buckets to be returned in response", bounded
    // `range {min 1, max 10000}` (`smithy_query(query="operation",operation="ListBuckets")`).
    // S3 states one sentence for a value that is no integer at all and another for an
    // integer outside the bounds, and names the parameter and the value as sent in the
    // `<ArgumentName>` / `<ArgumentValue>` pair either way.
    //
    // (case, the value as sent, the sentence it earns).
    let not_an_integer = "Provided max-buckets not an integer or within integer range";
    let outside_bounds = format!(
        "Argument max-buckets must be an integer between {} and {}",
        MAX_BUCKETS_BOUNDS.start(),
        MAX_BUCKETS_BOUNDS.end()
    );
    let rows: &[(&str, &str, &str)] = &[
        ("a fraction", "1.5", not_an_integer),
        ("text", "many", not_an_integer),
        ("a number too large for the member", "9999999999999", not_an_integer),
        ("one below the minimum", "0", &outside_bounds),
        ("a negative value", "-1", &outside_bounds),
        ("one above the maximum", "10001", &outside_bounds),
        ("far above the maximum", "100000", &outside_bounds),
    ];
    for (case, value, message) in rows {
        let answer = list_buckets(&router, &format!("&max-buckets={value}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case:?}: expected the InvalidArgument code, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            *message,
            "case {case:?}: expected the message {message:?}, got {:?}",
            answer.error_message()
        );
        assert_eq!(
            answer.element("ArgumentName").as_deref(),
            Some("max-buckets"),
            "case {case:?}: the document must name the parameter it refused, got {:?} in {:?}",
            answer.element("ArgumentName"),
            text(&answer)
        );
        assert_eq!(
            answer.element("ArgumentValue").as_deref(),
            Some(*value),
            "case {case:?}: the document must echo the value as sent, got {:?}",
            answer.element("ArgumentValue")
        );
    }

    // Each published BOUND is served, and so is the parameter stated with NO value at all,
    // which names no bound: the refusals above are about the value and not about the
    // parameter being there.
    for (case, query) in [
        ("the minimum", format!("&max-buckets={}", MAX_BUCKETS_BOUNDS.start())),
        ("the maximum", format!("&max-buckets={}", MAX_BUCKETS_BOUNDS.end())),
        ("no value at all", "&max-buckets=".to_string()),
    ] {
        let answer = list_buckets(&router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert!(
            text(&answer).contains("<Name>argument-probe-bucket</Name>"),
            "case {case:?}: the page must report the account's bucket, got {:?}",
            text(&answer)
        );
    }

    // `ObjectOwnership` is an enum over three values
    // (`smithy_query(query="shape",shape="ObjectOwnership")`): a create naming anything else
    // is refused with the sentence naming the header, and the same pair naming the header and
    // the value as sent.
    for (case, value) in [
        ("a published value in another case", "bucketownerpreferred"),
        ("a published value with a space", "Bucket Owner Enforced"),
        ("a setting nothing publishes", "BucketOwnerOnly"),
        ("the empty setting", ""),
    ] {
        let answer = send_in_region(
            &router,
            "us-east-1",
            "PUT",
            "/ownership-probe-bucket",
            &[("x-amz-object-ownership", value)],
            "",
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case:?}: expected the InvalidArgument code, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            format!("Invalid x-amz-object-ownership header: {value}"),
            "case {case:?}: expected the sentence naming the header, got {:?}",
            answer.error_message()
        );
        assert_eq!(
            answer.element("ArgumentName").as_deref(),
            Some("x-amz-object-ownership"),
            "case {case:?}: the document must name the header it refused, got {:?} in {:?}",
            answer.element("ArgumentName"),
            text(&answer)
        );
        assert_eq!(
            answer.element("ArgumentValue").as_deref(),
            Some(value),
            "case {case:?}: the document must echo the value as sent, got {:?}",
            answer.element("ArgumentValue")
        );
        // Refused BEFORE anything is written: no bucket is there afterwards.
        assert_eq!(
            head_bucket(&router, "ownership-probe-bucket").await.status,
            404,
            "case {case:?}: a refused create must leave no bucket behind"
        );
    }
}

/// Bucket names Amazon S3 can ADDRESS no bucket by, one per sentence of the published
/// addressing syntax (`service_com_amazonaws_s3_amazons3::service::names_a_bucket`, the
/// legacy US East (N. Virginia) naming rules), as `(case, the name as the URI spells it, the
/// name the request addresses)`.
///
/// The rules a bucket may be CREATED under today are narrower in every direction — lowercase
/// only, at most 63 characters, no underscore, no reserved prefix or suffix — and they are
/// NOT here: a name no new bucket may carry is still a name an existing bucket can be
/// addressed by ([`super::general_purpose_bucket_names`] pins the create's own rule), so a
/// request naming one is answered about the bucket rather than about the name.
fn names_no_bucket() -> Vec<(String, String, String)> {
    let row = |case: &str, spelled: &str, addressed: &str| {
        (
            case.to_string(),
            spelled.to_string(),
            addressed.to_string(),
        )
    };
    let mut rows = vec![
        // Characters outside "uppercase letters, lowercase letters, numbers, periods (.),
        // hyphens (-), and underscores (_)", each one a URI path may legally carry.
        row("a tilde", "probe~bucket", "probe~bucket"),
        row("a comma", "probe,bucket", "probe,bucket"),
        row("a colon", "probe:bucket", "probe:bucket"),
        row("parentheses", "probe(bucket)", "probe(bucket)"),
        // A space, which a request can only spell percent-encoded — so this row also pins
        // that the name judged is the name the request ADDRESSES rather than the text of its
        // path.
        row("a percent-encoded space", "probe%20bucket", "probe bucket"),
        row("a leading hyphen", "-probe-bucket", "-probe-bucket"),
        row("a leading period", ".probe-bucket", ".probe-bucket"),
    ];
    rows.push(row(
        "one character short of the length bound",
        "aa",
        "aa",
    ));
    let too_long = "b".repeat(MAX_ADDRESSABLE_BUCKET_NAME_CHARS + 1);
    rows.push(row(
        "one character past the length bound",
        &too_long,
        &too_long,
    ));
    rows
}

/// The longest name a request may address a bucket by: "bucket names must be between 3 and
/// 255 characters long" (Amazon S3 User Guide, *Bucket naming rules*, the legacy US East
/// (N. Virginia) rules) — the bound [`names_a_bucket`] reads.
const MAX_ADDRESSABLE_BUCKET_NAME_CHARS: usize = 255;

#[tokio::test]
async fn a_name_s3_holds_no_bucket_under_is_refused_for_the_name_whichever_call_names_it() {
    let router = router();

    // A bucket held throughout, so a refusal that touched the store would show up.
    assert_eq!(
        create_bucket(&router, "name-probe-neighbour", None, None)
            .await
            .status,
        200,
        "the neighbour must be created first"
    );

    for (case, spelled, addressed) in names_no_bucket() {
        // One call per kind of bucket-addressed route: a sub-resource read, a sub-resource
        // write, a bucket read, a listing of the bucket's objects and the bucket's own
        // delete — every one of them bound under the `{Bucket}` label.
        let calls: Vec<(&str, Answer)> = vec![
            (
                "GetBucketEncryption",
                send(&router, "GET", &format!("/{spelled}?encryption"), &[], "").await,
            ),
            (
                "GetBucketTagging",
                send(&router, "GET", &format!("/{spelled}?tagging"), &[], "").await,
            ),
            (
                "PutBucketTagging",
                send(
                    &router,
                    "PUT",
                    &format!("/{spelled}?tagging"),
                    &[],
                    "<Tagging><TagSet><Tag><Key>k</Key><Value>v</Value></Tag></TagSet></Tagging>",
                )
                .await,
            ),
            (
                "GetBucketLocation",
                send(&router, "GET", &format!("/{spelled}?location"), &[], "").await,
            ),
            (
                "ListObjectsV2",
                send(&router, "GET", &format!("/{spelled}?list-type=2"), &[], "").await,
            ),
            (
                "DeleteBucket",
                send(&router, "DELETE", &format!("/{spelled}"), &[], "").await,
            ),
        ];
        for (operation, answer) in &calls {
            assert_eq!(
                answer.status, 400,
                "case {case:?} ({addressed:?}) on {operation}: a name S3 can address no \
                 bucket by is refused for the name, got {} with body {:?}",
                answer.status,
                text(answer)
            );
            assert_eq!(
                answer.error_code(),
                INVALID_BUCKET_NAME_CODE,
                "case {case:?} on {operation}: expected the code \
                 {INVALID_BUCKET_NAME_CODE:?}, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                INVALID_BUCKET_NAME_MESSAGE,
                "case {case:?} on {operation}: expected the message \
                 {INVALID_BUCKET_NAME_MESSAGE:?}, got {:?}",
                answer.error_message()
            );
            assert_eq!(
                answer.element("BucketName").as_deref(),
                Some(addressed.as_str()),
                "case {case:?} on {operation}: the document must name the rejected bucket, \
                 got {:?} in {:?}",
                answer.element("BucketName"),
                text(answer)
            );
        }
        // `HeadBucket` answers the same class with no document at all, the way HEAD does.
        assert_eq!(
            send(&router, "HEAD", &format!("/{spelled}"), &[], "")
                .await
                .status,
            400,
            "case {case:?} on HeadBucket: expected HTTP 400 for a name S3 can address no \
             bucket by"
        );
    }

    // A name the ADDRESSING syntax admits is a different answer: the bucket is absent, not
    // invalid — so the refusal above is about the name and not about every call on a bucket
    // that is not there. The rules a bucket may be CREATED under are narrower, and every row
    // below breaks one of them: a bucket cannot be created with an upper-case letter, an
    // underscore, a reserved prefix or suffix, two adjacent periods, a name formatted as an
    // IP address or more than 63 characters — but an existing bucket could carry such a name
    // under the legacy rules, so a request naming one addresses a bucket and the answer is
    // about the bucket.
    for (case, name) in [
        ("a plain name nobody created", "absent-but-valid".to_string()),
        ("a name with periods", "absent.but.valid".to_string()),
        ("an upper-case letter", "Absent-But-Addressable".to_string()),
        ("an underscore", "absent_but_addressable".to_string()),
        ("a reserved prefix", "amzn-s3-demo-probe".to_string()),
        ("a reserved suffix", "probe-s3alias".to_string()),
        ("the directory-bucket suffix", "probe-zone--x-s3".to_string()),
        ("two adjacent periods", "probe..bucket".to_string()),
        ("a name formatted as an IP address", "192.168.5.4".to_string()),
        (
            "longer than a bucket may be created with",
            "c".repeat(MAX_BUCKET_NAME_CHARS + 1),
        ),
        (
            "the longest name a request may address",
            "d".repeat(MAX_ADDRESSABLE_BUCKET_NAME_CHARS),
        ),
    ] {
        let read = send(&router, "GET", &format!("/{name}?encryption"), &[], "").await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: an addressable name that holds no bucket is absent, got {} with \
             body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.error_code(),
            "NoSuchBucket",
            "case {case:?}: expected NoSuchBucket, got {:?}",
            read.error_code()
        );
    }

    // And a VIRTUAL-HOSTED-STYLE request to a bucket this emulator HOLDS is answered about
    // that bucket, never about a fragment of its key: such a request's path IS the key, and a
    // key may begin with anything a key may hold. Every row below is a key whose first segment
    // is NOT a name S3 could address a bucket by, read out of a bucket that is held — so
    // judging the path's first segment here would refuse a request S3 serves.
    let held = "name-probe-neighbour";
    for (case, key, bytes) in [
        ("a short first segment", "ab", &b"short"[..]),
        ("a versioned prefix", "v1/data", &b"versioned"[..]),
        ("a leading hyphen", "-report.csv", &b"report"[..]),
        ("a space in the key", "my%20file.txt", &b"spaced"[..]),
        ("parentheses in the key", "report(final).pdf", &b"final"[..]),
    ] {
        let written = send_bytes_to_host(
            &router,
            "PUT",
            &format!("{held}.s3.us-east-1.amazonaws.com"),
            &format!("/{key}"),
            &[],
            bytes,
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: a host-style write of the key {key:?} must be served, got {} with \
             body {:?}",
            written.status,
            text(&written)
        );
        let read = send_bytes_to_host(
            &router,
            "GET",
            &format!("{held}.s3.us-east-1.amazonaws.com"),
            &format!("/{key}"),
            &[],
            b"",
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a host-style read of the key {key:?} must be served, got {} with \
             body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            bytes,
            "case {case:?}: the host-style read must answer the bytes written"
        );
    }

    // Nothing above changed the store: the neighbour is still the only bucket.
    let document = text(&list_buckets(&router, "").await);
    assert_eq!(
        document.matches("<Name>").count(),
        1,
        "the refusals must leave the account's buckets alone, got {document:?}"
    );
}
