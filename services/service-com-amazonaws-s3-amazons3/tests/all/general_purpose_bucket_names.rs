// SPDX-License-Identifier: CC0-1.0

//! `CreateBucket` refuses a name Amazon S3 holds no general purpose bucket under.
//!
//! The rule under test: a create whose bucket name is outside the published *General
//! purpose bucket naming rules* is refused with HTTP 400, the error code
//! `InvalidBucketName` and the sentence S3's published error list states for that code,
//! with the rejected name in the `<BucketName>` member of the error document — and nothing
//! is recorded, so the account holds no bucket under a name it could never address. The
//! rules themselves are the ones every service in this workspace judges a caller-supplied
//! bucket name by (`service_common::s3_bucket_name`), which is where the length bounds and
//! the reserved prefix and suffix catalogs below come from.
//!
//! The tables span what the rule generalises over:
//!
//! * which SENTENCE of the rules the name breaks — the character class (an upper-case
//!   letter, an underscore, a space), the first and last character (a leading or trailing
//!   hyphen or period), two adjacent periods, a name formatted as an IP address, and every
//!   reserved prefix and suffix the catalog publishes, not only the one the report cited;
//! * the LENGTH bound, at each published edge and one character past it;
//! * names the rules ADMIT, including ones that carry a reserved prefix or suffix somewhere
//!   other than at the end they are reserved at, so the refusals above are about the rule
//!   and not about the text appearing at all.
//!
//! Every assertion drives a real HTTP request through the router (the harness
//! [`super::bucket_lifecycle`] owns), so what is pinned is the wire document.

use service_common::s3_bucket_name::{
    MAX_BUCKET_NAME_CHARS, MIN_BUCKET_NAME_CHARS, RESERVED_PREFIXES, RESERVED_SUFFIXES,
};

use super::bucket_lifecycle::{create_bucket, head_bucket, router};
use super::list_buckets::list_buckets;
use super::object_and_tagging_roundtrip::text;

/// The code S3's published error list states for a name outside the naming rules, and the
/// sentence it publishes beside it.
const INVALID_BUCKET_NAME: &str = "InvalidBucketName";

/// See [`INVALID_BUCKET_NAME`] — the fixed text of that error class.
const INVALID_BUCKET_NAME_MESSAGE: &str = "The specified bucket is not valid.";

/// One row: the case, the name the create carries, and whether S3 holds a bucket under it.
#[derive(Debug, Clone)]
struct Row {
    case: String,
    bucket: String,
    held: bool,
}

impl Row {
    /// A row whose name the published rules ADMIT.
    fn admitted(case: &str, bucket: &str) -> Self {
        Self {
            case: case.to_string(),
            bucket: bucket.to_string(),
            held: true,
        }
    }

    /// A row whose name the published rules REFUSE.
    fn refused(case: &str, bucket: impl Into<String>) -> Self {
        Self {
            case: case.to_string(),
            bucket: bucket.into(),
            held: false,
        }
    }
}

/// Every row of the naming table: one per sentence of the published rules, plus the names
/// those sentences admit.
///
/// The reserved prefix and suffix rows are generated from the catalogs themselves
/// ([`RESERVED_PREFIXES`], [`RESERVED_SUFFIXES`]), so each published entry is exercised —
/// including the ones the report that asked for this rule never mentioned.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row::admitted("the shortest name the rules admit", &"a".repeat(MIN_BUCKET_NAME_CHARS)),
        Row::admitted("the longest name the rules admit", &"b".repeat(MAX_BUCKET_NAME_CHARS)),
        Row::admitted("lowercase letters, digits and hyphens", "team-7-data"),
        Row::admitted("periods between letters", "logs.2026.archive"),
        Row::admitted("both kinds of punctuation", "a-b.c-d"),
        Row::admitted("digits at both ends", "2026-quarterly-7"),
        // A reserved prefix is reserved at the START of the name and a reserved suffix at
        // its END: the same text elsewhere in the name is just text.
        Row::admitted("a reserved prefix somewhere other than the start", "logs-xn--archive"),
        Row::admitted("a reserved suffix somewhere other than the end", "bucket-s3alias-logs"),
        Row::admitted("a prefix one character short of a reserved one", "xn-bucket"),
        // The recorded case of the report this rule was asked for: a name carrying both an
        // upper-case letter and an underscore.
        Row::refused("an upper-case letter and an underscore", "Invalid_Bucket_Name"),
        Row::refused("upper-case letters alone", "uppercaseletterS"),
        Row::refused("an underscore alone", "under_score"),
        Row::refused("a tilde", "tilde~name"),
        Row::refused("a character outside the class", "br(acket)s"),
        Row::refused("a leading hyphen", "-leading-hyphen"),
        Row::refused("a trailing hyphen", "trailing-hyphen-"),
        Row::refused("a leading period", ".leading-period"),
        Row::refused("a trailing period", "trailing-period."),
        Row::refused("two adjacent periods", "two..periods"),
        Row::refused("a name formatted as an IP address", "192.168.5.4"),
        Row::refused("a name formatted as an IP address of long octets", "255.255.255.255"),
    ];
    for prefix in RESERVED_PREFIXES {
        rows.push(Row::refused(
            &format!("the reserved prefix {prefix:?}"),
            format!("{prefix}quarterly-logs"),
        ));
    }
    for suffix in RESERVED_SUFFIXES {
        rows.push(Row::refused(
            &format!("the reserved suffix {suffix:?}"),
            format!("quarterly-logs{suffix}"),
        ));
    }
    rows
}

#[tokio::test]
async fn a_create_is_refused_for_every_name_the_published_rules_do_not_admit() {
    let router = router();

    // A neighbour held throughout, so a refusal below that wrote something would show up as
    // a change to the account's buckets rather than passing unnoticed.
    let neighbour = "naming-rules-neighbour";
    let held = create_bucket(&router, neighbour, None, None).await;
    assert_eq!(
        held.status,
        200,
        "the neighbour must be created first, got {} with body {:?}",
        held.status,
        text(&held)
    );

    let rows = rows();
    for row in &rows {
        let Row { case, bucket, held } = row;
        let created = create_bucket(&router, bucket, None, None).await;
        if *held {
            assert_eq!(
                created.status, 200,
                "case {case:?}: S3 holds a bucket under {bucket:?}, expected HTTP 200, got \
                 {} with body {:?}",
                created.status,
                text(&created)
            );
            let read = head_bucket(&router, bucket).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the created bucket must read back, expected HTTP 200, got {}",
                read.status
            );
            continue;
        }

        assert_eq!(
            created.status, 400,
            "case {case:?}: {bucket:?} is not a name S3 holds a bucket under, expected HTTP \
             400, got {} with body {:?}",
            created.status,
            text(&created)
        );
        assert_eq!(
            created.error_code(),
            INVALID_BUCKET_NAME,
            "case {case:?}: expected the error code {INVALID_BUCKET_NAME:?}, got {:?}",
            created.error_code()
        );
        assert_eq!(
            created.error_message(),
            INVALID_BUCKET_NAME_MESSAGE,
            "case {case:?}: expected the message {INVALID_BUCKET_NAME_MESSAGE:?}, got {:?}",
            created.error_message()
        );
        assert_eq!(
            created.element("BucketName").as_deref(),
            Some(bucket.as_str()),
            "case {case:?}: the error document must name the rejected bucket, got {:?} in \
             {:?}",
            created.element("BucketName"),
            text(&created)
        );
        // Refused BEFORE anything is written: the bucket is not there afterwards.
        let read = head_bucket(&router, bucket).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must leave no bucket behind, expected HTTP 404, \
             got {}",
            read.status
        );
    }

    // The account holds exactly the neighbour and the names the rules admit — so no refused
    // create recorded a bucket under a name that cannot be addressed.
    let listing = list_buckets(&router, "").await;
    let document = text(&listing);
    for row in &rows {
        let Row { case, bucket, held } = row;
        let reported = document.contains(&format!("<Name>{bucket}</Name>"));
        assert_eq!(
            reported, *held,
            "case {case:?}: expected the listing to report {bucket:?}={held}, got \
             {reported} in {document:?}"
        );
    }
    assert!(
        document.contains(&format!("<Name>{neighbour}</Name>")),
        "the neighbour must still be listed, got {document:?}"
    );
}

#[tokio::test]
async fn the_published_length_bound_holds_at_each_edge() {
    let router = router();

    // Each published edge of "between 3 (min) and 63 (max) characters long", and one
    // character past it on either side.
    for (length, admitted) in [
        (MIN_BUCKET_NAME_CHARS - 1, false),
        (MIN_BUCKET_NAME_CHARS, true),
        (MIN_BUCKET_NAME_CHARS + 1, true),
        (MAX_BUCKET_NAME_CHARS - 1, true),
        (MAX_BUCKET_NAME_CHARS, true),
        (MAX_BUCKET_NAME_CHARS + 1, false),
    ] {
        let bucket = "c".repeat(length);
        let created = create_bucket(&router, &bucket, None, None).await;
        let expected = if admitted { 200 } else { 400 };
        assert_eq!(
            created.status, expected,
            "case {length} characters: expected HTTP {expected}, got {} with body {:?}",
            created.status,
            text(&created)
        );
        if admitted {
            continue;
        }
        assert_eq!(
            created.error_code(),
            INVALID_BUCKET_NAME,
            "case {length} characters: expected the error code {INVALID_BUCKET_NAME:?}, got \
             {:?}",
            created.error_code()
        );
        assert_eq!(
            created.element("BucketName").as_deref(),
            Some(bucket.as_str()),
            "case {length} characters: the error document must name the rejected bucket, got \
             {:?}",
            created.element("BucketName")
        );
    }
}
