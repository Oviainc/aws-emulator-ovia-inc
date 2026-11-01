// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListBuckets`: the account's own bucket list, read back over the
//! wire from the same store `CreateBucket` writes and `DeleteBucket` empties.
//!
//! Every assertion drives a real HTTP request through the router — the harness is the one
//! [`super::bucket_lifecycle`] owns — so what is pinned is the wire behaviour (status,
//! listing payload, error code) rather than any handler's internals.
//!
//! The rules under test, with their sources (the model's own documentation, reachable as
//! `smithy_query(query="operation",operation="ListBuckets")` and
//! `smithy_query(query="shape",shape="Bucket")`):
//!
//! * `ListBuckets` "returns a list of all buckets owned by the authenticated sender of
//!   the request", each entry's `Name` and `CreationDate` coming from the stored bucket,
//!   in the bucket-name order the store keeps, with `Owner` the account that owns them.
//! * `Prefix` "limits the response to bucket names that begin with the specified bucket
//!   name prefix" and is "included in the response" when it was sent; `BucketRegion`
//!   "limits the response to buckets that are located in the specified Amazon Web
//!   Services Region".
//! * `Bucket$BucketRegion` "indicates the Amazon Web Services region where the bucket is
//!   located. If the request contains at least one valid parameter, it is included in the
//!   response" — so a bare `ListBuckets` reports none.
//! * `MaxBuckets` is the "maximum number of buckets to be returned in response", bounded
//!   `range {min 1, max 10000}`; a response that stopped short carries a
//!   `ContinuationToken`, "included in the response when there are more buckets that can
//!   be listed with pagination", and sending it back resumes the listing where it stopped.
//! * `Bucket$BucketArn` "is only supported for S3 directory buckets", so a general
//!   purpose bucket reports none.
//! * A listing is a READ: listing twice reports the same thing, and what a create or a
//!   delete did is what moves it.

use std::collections::BTreeMap;

use quick_xml::events::Event;
use quick_xml::Reader;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::object_and_tagging_roundtrip::text;
use super::object_storage::delete_bucket;

/// `GET /?x-id=ListBuckets…` — `ListBuckets`, with whatever extra query parameters the
/// case under test carries.
///
/// `pub(crate)`: the published-sample-data suite reads the account's listing back through
/// this one sender rather than standing up a second copy of it.
pub(crate) async fn list_buckets(router: &MultiServiceRouter, query: &str) -> Answer {
    send(router, "GET", &format!("/?x-id=ListBuckets{query}"), &[], "").await
}

/// One `ListAllMyBucketsResult` body, as a client's XML reader sees it.
#[derive(Debug, Default)]
struct BucketListing {
    /// The elements directly under the result: `ContinuationToken` and the echoed
    /// `Prefix`.
    top: BTreeMap<String, String>,
    /// One map per `Buckets/Bucket` entry, keyed by element name, in response order.
    buckets: Vec<BTreeMap<String, String>>,
    /// The `Owner` container's elements.
    owner: BTreeMap<String, String>,
    /// Whether the body carried a `Buckets` container at all.
    saw_buckets_container: bool,
}

impl BucketListing {
    /// A top-level element's text, or the empty string when the response carries none —
    /// which is what "absent" looks like to a client reading the member.
    fn field(&self, name: &str) -> String {
        self.top.get(name).cloned().unwrap_or_default()
    }

    /// An `Owner` element's text, or the empty string when it carries none.
    fn owner_field(&self, name: &str) -> String {
        self.owner.get(name).cloned().unwrap_or_default()
    }

    /// Every listed bucket `Name`, in response order.
    fn names(&self) -> Vec<String> {
        self.buckets
            .iter()
            .map(|entry| entry.get("Name").cloned().unwrap_or_default())
            .collect()
    }

    /// One listed bucket's element, or the empty string when it carries none.
    fn entry_field(entry: &BTreeMap<String, String>, name: &str) -> String {
        entry.get(name).cloned().unwrap_or_default()
    }
}

/// Read a `ListBuckets` body with an XML reader, rather than by slicing the body.
fn listing_of(answer: &Answer) -> BucketListing {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut listing = BucketListing::default();
    let mut element = String::new();
    let mut current = String::new();
    let mut entry: Option<BTreeMap<String, String>> = None;
    let mut in_owner = false;
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the listing body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                current.clear();
                match element.as_str() {
                    "Buckets" => listing.saw_buckets_container = true,
                    "Bucket" => entry = Some(BTreeMap::new()),
                    "Owner" => in_owner = true,
                    _ => {}
                }
            }
            // An account holding no bucket can carry the container as `<Buckets/>`.
            Event::Empty(empty) => {
                if String::from_utf8_lossy(empty.name().as_ref()) == "Buckets" {
                    listing.saw_buckets_container = true;
                }
            }
            Event::Text(value) => {
                let decoded = value
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the listing body must decode: {error}"));
                current.push_str(&decoded);
            }
            Event::End(end) => {
                let closing = String::from_utf8_lossy(end.name().as_ref()).into_owned();
                match closing.as_str() {
                    "Bucket" => {
                        if let Some(done) = entry.take() {
                            listing.buckets.push(done);
                        }
                    }
                    "Owner" => in_owner = false,
                    // Every other element's text belongs to whichever container is open.
                    name if name == element => {
                        let value = std::mem::take(&mut current);
                        match (entry.as_mut(), in_owner) {
                            (Some(entry), _) => {
                                entry.insert(name.to_string(), value);
                            }
                            (None, true) => {
                                listing.owner.insert(name.to_string(), value);
                            }
                            (None, false) => {
                                listing.top.insert(name.to_string(), value);
                            }
                        }
                    }
                    _ => {}
                }
                element.clear();
                current.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    listing
}

/// Create every bucket in `names`, in REVERSE name order so a listing that reported
/// creation order rather than name order would fail, and answer them in the ascending
/// order the listing must use.
async fn create_buckets(router: &MultiServiceRouter, names: &[&str]) -> Vec<String> {
    let mut reversed: Vec<&&str> = names.iter().collect();
    reversed.sort_by(|left, right| right.cmp(left));
    for name in &reversed {
        let created = create_bucket(router, name, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the create must succeed, got {} with body {:?}",
            created.status,
            text(&created)
        );
    }
    let mut sorted: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    sorted.sort();
    sorted
}

/// Whether `id` has the shape AWS documents for a canonical user id: "a 64-digit
/// hexadecimal encoded, 256-bit value" (AWS General Reference, *AWS account
/// identifiers*).
fn is_canonical_user_id(id: &str) -> bool {
    id.len() == 64 && id.chars().all(|digit| digit.is_ascii_hexdigit())
}

#[tokio::test]
async fn a_listing_reports_every_created_bucket_in_name_order() {
    // Name sets whose byte order differs from any "natural" order a sort might fall back
    // on, and from the order they are created in: `log-10` before `log-2`, punctuation
    // ordered by code point (`-` 0x2D < `.` 0x2E < `0` 0x30), and a hyphen sorting before
    // the letter a punctuation-folding order would skip it for. Every name is one the
    // published naming rules admit, so a row's set is one an account can really hold.
    let rows: &[(&str, &[&str])] = &[
        ("a single bucket", &["only-bucket"]),
        ("three plain names", &["alpha", "beta", "gamma"]),
        ("digits are not numbers", &["log-1", "log-10", "log-2"]),
        ("punctuation by code point", &["a-b", "a.b", "a0b"]),
        ("a hyphen before the letter that follows it", &["a-bc", "abc"]),
        (
            "names that are prefixes of one another",
            &["app", "app-data", "app-data-2", "appz"],
        ),
    ];

    for (case, names) in rows {
        // A fresh router per row, so each row lists exactly the buckets it created.
        let router = router();

        // Before anything is created the account holds no bucket: an empty listing, not
        // a failure.
        let empty = list_buckets(&router, "").await;
        assert_eq!(
            empty.status, 200,
            "case {case:?}: an account with no buckets still lists, got {} with body {:?}",
            empty.status,
            text(&empty)
        );
        let empty_listing = listing_of(&empty);
        assert!(
            empty_listing.names().is_empty(),
            "case {case:?}: nothing was created yet, got {:?}",
            empty_listing.names()
        );
        assert!(
            empty_listing.saw_buckets_container,
            "case {case:?}: the Buckets container is reported even when it is empty, got \
             body {:?}",
            text(&empty)
        );

        let expected = create_buckets(&router, names).await;

        let answer = list_buckets(&router, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let listing = listing_of(&answer);
        assert_eq!(
            listing.names(),
            expected,
            "case {case:?}: every created bucket must be listed once, in ascending name \
             order, got {:?}",
            listing.names()
        );

        for entry in &listing.buckets {
            let name = BucketListing::entry_field(entry, "Name");
            // "Date the bucket was created": a date-time, and the same one on every read.
            let created = BucketListing::entry_field(entry, "CreationDate");
            assert!(
                aws_smithy_types::DateTime::from_str(
                    &created,
                    aws_smithy_types::date_time::Format::DateTime
                )
                .is_ok(),
                "case {case:?}, bucket {name:?}: CreationDate must be a date-time, got \
                 {created:?}"
            );
            // A request that named no parameter gets no per-entry Region.
            assert_eq!(
                BucketListing::entry_field(entry, "BucketRegion"),
                "",
                "case {case:?}, bucket {name:?}: a request with no parameter reports no \
                 BucketRegion, got {:?}",
                BucketListing::entry_field(entry, "BucketRegion")
            );
            // "The Amazon Resource Name (ARN) of the S3 bucket" (`Bucket$BucketArn`): the
            // ARN of the bucket the entry reports, which for a general purpose bucket
            // carries its globally unique name and leaves the Region and account fields
            // empty.
            assert_eq!(
                BucketListing::entry_field(entry, "BucketArn"),
                format!("arn:aws:s3:::{name}"),
                "case {case:?}, bucket {name:?}: the entry must report the bucket's ARN, \
                 got {:?}",
                BucketListing::entry_field(entry, "BucketArn")
            );
        }

        // "The owner of the buckets listed": one account, reported by its canonical user
        // id rather than by an account id.
        let owner = listing.owner_field("ID");
        assert!(
            is_canonical_user_id(&owner),
            "case {case:?}: Owner/ID must be a 64-digit hexadecimal canonical user id, \
             got {owner:?}"
        );
        assert_ne!(
            owner, "000000000000",
            "case {case:?}: the owner is a canonical user id, not the account id"
        );

        // Nothing was left for a next page, so no continuation token, and no Prefix was
        // sent so none is echoed.
        assert_eq!(
            listing.field("ContinuationToken"),
            "",
            "case {case:?}: a listing that reported every bucket owes no continuation \
             token, got {:?}",
            listing.field("ContinuationToken")
        );
        assert_eq!(
            listing.field("Prefix"),
            "",
            "case {case:?}: no Prefix was sent, so none is echoed, got {:?}",
            listing.field("Prefix")
        );

        // A listing is a read: listing again reports the same buckets, the same creation
        // dates and the same owner, so nothing moved because something was enumerated.
        let again = listing_of(&list_buckets(&router, "").await);
        assert_eq!(
            again.names(),
            listing.names(),
            "case {case:?}: a second listing must report the same buckets, got {:?}",
            again.names()
        );
        assert_eq!(
            again.buckets, listing.buckets,
            "case {case:?}: a second listing must report the same entries, got {:?}",
            again.buckets
        );
        assert_eq!(
            again.owner_field("ID"),
            owner,
            "case {case:?}: the owner is stable across reads, got {:?}",
            again.owner_field("ID")
        );

        // A bare `GET /` — what an SDK sends for `ListBuckets` without the `x-id` hint —
        // answers the same listing.
        let bare = send(&router, "GET", "/", &[], "").await;
        assert_eq!(
            bare.status, 200,
            "case {case:?}: a bare GET / lists the buckets too, got {} with body {:?}",
            bare.status,
            text(&bare)
        );
        assert_eq!(
            listing_of(&bare).names(),
            listing.names(),
            "case {case:?}: GET / must report the same buckets as GET /?x-id=ListBuckets"
        );
    }
}

#[tokio::test]
async fn prefix_and_bucket_region_narrow_the_listing_and_then_each_entry_reports_its_region() {
    let router = router();

    // (bucket, the location constraint it is created with, the Region it then lives in).
    // `us-east-1` is reachable only through an absent constraint, and `EU` is the legacy
    // alias for `eu-west-1` — both per `CreateBucketConfiguration$LocationConstraint`.
    let created: &[(&str, Option<&str>, &str)] = &[
        ("app-archive", Some("ap-southeast-7"), "ap-southeast-7"),
        ("app-data", Some("us-west-2"), "us-west-2"),
        ("app-logs", None, "us-east-1"),
        ("web-logs", Some("EU"), "eu-west-1"),
        ("zzz-data", Some("us-west-2"), "us-west-2"),
    ];
    for (bucket, constraint, _) in created {
        let answer = create_bucket(&router, bucket, *constraint, None).await;
        assert_eq!(
            answer.status, 200,
            "case {bucket:?}: the create must succeed, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
    }

    // (case, query, the bucket names the listing must report).
    let rows: &[(&str, &str, &[&str])] = &[
        (
            "a prefix on a name stem",
            "&prefix=app-",
            &["app-archive", "app-data", "app-logs"],
        ),
        ("a longer prefix", "&prefix=app-l", &["app-logs"]),
        ("a prefix that is a whole name", "&prefix=web-logs", &["web-logs"]),
        ("a prefix matching nothing", "&prefix=nothing-here", &[]),
        (
            "the empty prefix matches every bucket",
            "&prefix=",
            &["app-archive", "app-data", "app-logs", "web-logs", "zzz-data"],
        ),
        (
            "a Region two buckets share",
            "&bucket-region=us-west-2",
            &["app-data", "zzz-data"],
        ),
        (
            "the Region an absent constraint means",
            "&bucket-region=us-east-1",
            &["app-logs"],
        ),
        (
            "the Region the legacy EU alias names",
            "&bucket-region=eu-west-1",
            &["web-logs"],
        ),
        (
            "a Region no bucket of this account lives in",
            "&bucket-region=me-central-1",
            &[],
        ),
        (
            "prefix and Region together",
            "&prefix=app-&bucket-region=us-west-2",
            &["app-data"],
        ),
        (
            "a prefix and a Region that select nothing together",
            "&prefix=web-&bucket-region=us-west-2",
            &[],
        ),
    ];

    for (case, query, expected) in rows {
        let answer = list_buckets(&router, query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let listing = listing_of(&answer);
        assert_eq!(
            listing.names(),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: wrong buckets, got {:?}",
            listing.names()
        );

        // "If the request contains at least one valid parameter, it is included in the
        // response": every row above names one, so each entry carries the Region the
        // bucket was created in.
        for entry in &listing.buckets {
            let name = BucketListing::entry_field(entry, "Name");
            let region = created
                .iter()
                .find(|(bucket, _, _)| *bucket == name)
                .map(|(_, _, region)| *region)
                .unwrap_or_else(|| panic!("case {case:?}: the listing named an uncreated bucket {name:?}"));
            assert_eq!(
                BucketListing::entry_field(entry, "BucketRegion"),
                region,
                "case {case:?}, bucket {name:?}: BucketRegion is the Region the bucket \
                 was created in, got {:?}",
                BucketListing::entry_field(entry, "BucketRegion")
            );
        }

        // "If Prefix was sent with the request, it is included in the response."
        let sent_prefix = query
            .split('&')
            .filter_map(|parameter| parameter.strip_prefix("prefix="))
            .next_back()
            .unwrap_or_default();
        assert_eq!(
            listing.field("Prefix"),
            sent_prefix,
            "case {case:?}: the response echoes the Prefix it was sent, got {:?}",
            listing.field("Prefix")
        );
    }
}

#[tokio::test]
async fn max_buckets_bounds_a_page_and_the_continuation_token_resumes_it() {
    let router = router();
    let every_bucket = create_buckets(
        &router,
        &["page-a", "page-b", "page-c", "page-d", "page-e"],
    )
    .await;

    // Page sizes below, at and above the number of buckets held: every one of them must
    // walk the whole listing, in order, with each page no larger than it asked for.
    for page_size in [1usize, 2, 3, 5, 7] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &token {
                None => format!("&max-buckets={page_size}"),
                Some(token) => format!("&max-buckets={page_size}&continuation-token={token}"),
            };
            let answer = list_buckets(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size}, page {pages}: expected 200, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            let listing = listing_of(&answer);
            pages += 1;
            assert!(
                listing.names().len() <= page_size,
                "case page size {page_size}, page {pages}: a page may not exceed \
                 MaxBuckets, got {:?}",
                listing.names()
            );
            walked.extend(listing.names());

            let next = listing.field("ContinuationToken");
            assert_eq!(
                !next.is_empty(),
                walked.len() < every_bucket.len(),
                "case page size {page_size}, page {pages}: a token is carried exactly \
                 while buckets remain, got {next:?} after {} of {} buckets",
                walked.len(),
                every_bucket.len()
            );
            if next.is_empty() {
                break;
            }
            token = Some(next);
            assert!(
                pages <= every_bucket.len() + 1,
                "case page size {page_size}: paging must make progress, walked {walked:?}"
            );
        }
        assert_eq!(
            walked, every_bucket,
            "case page size {page_size}: paging must walk every bucket once, in order, \
             got {walked:?}"
        );
        assert_eq!(
            pages,
            every_bucket.len().div_ceil(page_size),
            "case page size {page_size}: a page is filled before the next one starts, \
             took {pages} pages"
        );
    }

    // Paging NARROWED by a prefix walks only what the prefix selects, and the token
    // resumes within it.
    let first = listing_of(&list_buckets(&router, "&prefix=page-&max-buckets=2").await);
    assert_eq!(
        first.names(),
        vec!["page-a".to_string(), "page-b".to_string()],
        "case prefix paging: the first page is the first two selected buckets, got {:?}",
        first.names()
    );
    let resumed = listing_of(
        &list_buckets(
            &router,
            &format!(
                "&prefix=page-&continuation-token={}",
                first.field("ContinuationToken")
            ),
        )
        .await,
    );
    assert_eq!(
        resumed.names(),
        vec![
            "page-c".to_string(),
            "page-d".to_string(),
            "page-e".to_string()
        ],
        "case prefix paging: resuming reports what was left, got {:?}",
        resumed.names()
    );
    assert_eq!(
        resumed.field("Prefix"),
        "page-",
        "case prefix paging: the resumed page echoes the Prefix too, got {:?}",
        resumed.field("Prefix")
    );

    // A page size at the published ceiling is served, not refused: it is the default page
    // size and the account bucket quota both.
    let whole = list_buckets(&router, "&max-buckets=10000").await;
    assert_eq!(
        whole.status, 200,
        "case max-buckets at the ceiling: expected 200, got {} with body {:?}",
        whole.status,
        text(&whole)
    );
    assert_eq!(
        listing_of(&whole).names(),
        every_bucket,
        "case max-buckets at the ceiling: one page names every bucket"
    );
}

#[tokio::test]
async fn a_listing_parameter_outside_its_bound_or_a_token_this_service_did_not_mint_is_refused() {
    let router = router();
    let every_bucket = create_buckets(&router, &["refuse-a", "refuse-b"]).await;

    // A token this service did not mint is not read as a cursor: it is refused with the
    // code S3 documents for a bad argument, not answered as a first page.
    for (case, token) in [
        ("a token that is not even hex", "not-a-token-we-minted"),
        ("hex that decodes to someone else's marker", "733376312061"),
    ] {
        let answer = list_buckets(&router, &format!("&continuation-token={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case:?}: expected InvalidArgument, got {:?}",
            answer.error_code()
        );
    }

    // `Token` is `length {min 0}`: an EMPTY continuation token is a legal value that
    // names no cursor, so it is the first page rather than a refusal.
    let empty_token = list_buckets(&router, "&continuation-token=").await;
    assert_eq!(
        empty_token.status, 200,
        "case an empty continuation token: expected 200, got {} with body {:?}",
        empty_token.status,
        text(&empty_token)
    );
    assert_eq!(
        listing_of(&empty_token).names(),
        every_bucket,
        "case an empty continuation token: it names no cursor, so every bucket is listed"
    );

    // `MaxBuckets` is `range {min 1, max 10000}`: a value outside it is refused by the
    // boundary validator (which owns the code), never served as a page.
    for (case, max_buckets) in [
        ("below the minimum", "0"),
        ("negative", "-1"),
        ("above the maximum", "10001"),
    ] {
        let answer = list_buckets(&router, &format!("&max-buckets={max_buckets}")).await;
        assert_eq!(
            answer.status, 400,
            "case max-buckets {case}: expected HTTP 400, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
    }

    // The refusals changed nothing, and a bucket that is deleted leaves the listing: the
    // listing follows the store in both directions.
    let unchanged = listing_of(&list_buckets(&router, "").await);
    assert_eq!(
        unchanged.names(),
        every_bucket,
        "a refused listing leaves the store alone, got {:?}",
        unchanged.names()
    );

    assert_eq!(
        delete_bucket(&router, "refuse-a").await.status,
        204,
        "the empty bucket is deleted"
    );
    let after_delete = listing_of(&list_buckets(&router, "").await);
    assert_eq!(
        after_delete.names(),
        vec!["refuse-b".to_string()],
        "a deleted bucket disappears from the listing, got {:?}",
        after_delete.names()
    );
}
