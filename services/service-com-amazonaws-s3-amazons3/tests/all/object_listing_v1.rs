// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListObjects` — the first revision of the bucket listing,
//! `GET /<bucket>` with no `list-type`.
//!
//! Every assertion drives a real HTTP request through the router — the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_and_tagging_roundtrip`] and
//! [`super::object_storage`] own — so what is pinned is the wire behaviour (status,
//! listing payload, error code) rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `ListObjects` "returns some or all (up to 1,000) of the objects in a bucket", and
//!   "Amazon S3 lists objects in alphabetical order" (`NextMarker`), each entry's `Key`,
//!   `Size`, `ETag`, `LastModified`, `StorageClass` and `Owner` coming from the stored
//!   object, with `Name` the bucket. `Owner` is "the owner of the object" (`Object$Owner`)
//!   and this revision reports it on every entry — it is `ListObjectsV2` where "the owner
//!   field is not present … by default" (`ListObjectsV2Request$FetchOwner`).
//! * `Prefix` "limits the response to keys that begin with the specified prefix", `Marker`
//!   is "where you want Amazon S3 to start listing from. Amazon S3 starts listing after
//!   this specified key", and with a `Delimiter` the keys between the prefix and the next
//!   delimiter roll up into a `CommonPrefixes` entry, which "counts as only one return
//!   against the MaxKeys value"; a rolled-up prefix is "filtered out from results if it is
//!   not lexicographically greater than the key-marker" (`Delimiter`).
//! * `MaxKeys` "sets the maximum number of keys returned in the response"; a response that
//!   stopped short is `IsTruncated`, and `NextMarker` names the key to resume from — but
//!   "this element is returned only if you have the delimiter request parameter specified.
//!   If the response does not include the NextMarker element and it is truncated, you can
//!   use the value of the last Key element in the response as the marker parameter"
//!   (`NextMarker`).
//! * `EncodingType` is the "encoding type used by Amazon S3 to encode the object keys in
//!   the response", echoed when asked for and applied to every key-name element.
//! * A parameter whose answer this emulator cannot render keeps the scaffold's 501, one
//!   outside its published range is refused with `InvalidArgument`, an
//!   `x-amz-expected-bucket-owner` that is not the owner "fails with the HTTP status code
//!   403 Forbidden (access denied)", and a bucket that does not exist is `NoSuchBucket` —
//!   none of them is answered as a plain, empty success.

use service_com_amazonaws_s3_amazons3::state::canonical_user_id;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::object_and_tagging_roundtrip::{bucket, expected_etag, text};
use super::object_storage::{list_objects_v2, listing_of, sorted_keys, store_keys, Listing};

/// `GET /<bucket>[?…]` — `ListObjects`, with whatever query parameters the case under test
/// carries and no `list-type`, which is what separates it from `ListObjectsV2`.
async fn list_objects(router: &MultiServiceRouter, bucket: &str, query: &str) -> Answer {
    list_objects_with(router, bucket, query, &[]).await
}

/// The same request, carrying the headers the case under test needs.
async fn list_objects_with(
    router: &MultiServiceRouter,
    bucket: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let uri = if query.is_empty() {
        format!("/{bucket}")
    } else {
        format!("/{bucket}?{query}")
    };
    send(router, "GET", &uri, headers, "").await
}

/// The entries and rolled-up prefixes of one page, as the strings a case compares against.
fn page_of(answer: &Answer) -> (Vec<String>, Vec<String>) {
    let listing = listing_of(answer);
    (listing.keys(), listing.common_prefixes.clone())
}

/// One expected page of a delimited walk: the keys it reports, the rolled-up prefixes it
/// reports, and the `NextMarker` it hands back — empty for the page that ends the walk.
type DelimitedPage<'a> = (&'a [&'a str], &'a [&'a str], &'a str);

/// `expected` as owned strings, so a row can write its expectation as `&[&str]`.
fn owned(expected: &[&str]) -> Vec<String> {
    expected.iter().map(|value| (*value).to_string()).collect()
}

#[tokio::test]
async fn a_v1_listing_reports_every_stored_key_with_its_owner_in_byte_order() {
    // Key sets whose byte order differs from any "natural" order a sort might fall back
    // on: upper case before lower case, `file-10` before `file-2`, and punctuation
    // ordered by code point (`!` 0x21 < `-` 0x2D < `/` 0x2F < `0` 0x30 < `_` 0x5F < `~`
    // 0x7E).
    let rows: &[(&str, &[&str])] = &[
        ("single key", &["only.txt"]),
        ("upper case before lower case", &["a.txt", "B.txt", "b.txt"]),
        ("digits are not numbers", &["file-1", "file-10", "file-2"]),
        (
            "punctuation by code point",
            &["a!b", "a-b", "a/b", "a0b", "a_b", "a~b"],
        ),
        (
            "nested prefixes",
            &["a", "a/b", "a/b/c", "ab", "b/a", "b/a/c"],
        ),
    ];

    for (case, keys) in rows {
        let router = router();
        let bucket_name = "v1-listing-order";
        bucket(&router, bucket_name).await;
        let written = store_keys(&router, bucket_name, keys).await;

        let answer = list_objects(&router, bucket_name, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a listing of a bucket that exists is 200, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let listing = listing_of(&answer);

        assert_eq!(
            listing.keys(),
            sorted_keys(&written),
            "case {case:?}: keys must be listed in ascending UTF-8 byte order, got {:?}",
            listing.keys()
        );
        assert_eq!(
            listing.field("Name"),
            bucket_name,
            "case {case:?}: Name is the bucket, got {:?}",
            listing.field("Name")
        );
        assert_eq!(
            listing.field("MaxKeys"),
            "1000",
            "case {case:?}: a request that named no max-keys is answered with the default \
             page size, got {:?}",
            listing.field("MaxKeys")
        );
        assert_eq!(
            listing.field("IsTruncated"),
            "false",
            "case {case:?}: a page holding every key is not truncated, got {:?}",
            listing.field("IsTruncated")
        );
        assert_eq!(
            listing.field("NextMarker"),
            "",
            "case {case:?}: an untruncated page owes no NextMarker, got {:?}",
            listing.field("NextMarker")
        );
        assert!(
            listing.common_prefixes.is_empty(),
            "case {case:?}: without a delimiter nothing rolls up, got {:?}",
            listing.common_prefixes
        );

        // Every entry is computed from the object stored at that key.
        for entry in &listing.contents {
            let key = Listing::entry_field(entry, "Key");
            let body = written.get(&key).unwrap_or_else(|| {
                panic!("case {case:?}: the listing named an unwritten key {key:?}")
            });
            assert_eq!(
                Listing::entry_field(entry, "Size"),
                body.len().to_string(),
                "case {case:?}, key {key:?}: Size is the stored byte length, got {:?}",
                Listing::entry_field(entry, "Size")
            );
            assert_eq!(
                Listing::entry_field(entry, "ETag"),
                expected_etag(body),
                "case {case:?}, key {key:?}: ETag is the stored object's entity tag, got {:?}",
                Listing::entry_field(entry, "ETag")
            );
            assert_eq!(
                Listing::entry_field(entry, "StorageClass"),
                "STANDARD",
                "case {case:?}, key {key:?}: an object written without a storage class is \
                 STANDARD, got {:?}",
                Listing::entry_field(entry, "StorageClass")
            );
            // The `Owner` container this revision reports on every entry: the canonical
            // user id of the one account this emulator serves.
            assert_eq!(
                Listing::entry_field(entry, "ID"),
                canonical_user_id(),
                "case {case:?}, key {key:?}: Owner/ID is the account's canonical user id, \
                 got {:?}",
                Listing::entry_field(entry, "ID")
            );
            let last_modified = Listing::entry_field(entry, "LastModified");
            assert!(
                aws_smithy_types::DateTime::from_str(
                    &last_modified,
                    aws_smithy_types::date_time::Format::DateTime
                )
                .is_ok(),
                "case {case:?}, key {key:?}: LastModified must be a date-time, got \
                 {last_modified:?}"
            );
        }

        // The two revisions of the listing read ONE store, so they agree on every entry's
        // key, size and entity tag.
        let v2 = listing_of(&list_objects_v2(&router, bucket_name, "").await);
        let fields = |listing: &Listing| -> Vec<(String, String, String)> {
            listing
                .contents
                .iter()
                .map(|entry| {
                    (
                        Listing::entry_field(entry, "Key"),
                        Listing::entry_field(entry, "Size"),
                        Listing::entry_field(entry, "ETag"),
                    )
                })
                .collect()
        };
        assert_eq!(
            fields(&listing),
            fields(&v2),
            "case {case:?}: ListObjects and ListObjectsV2 must agree on every entry's key, \
             size and entity tag, got {:?} and {:?}",
            fields(&listing),
            fields(&v2)
        );

        // A listing is a read: listing again answers the same thing, so nothing about the
        // store moved because something was enumerated.
        let again = listing_of(&list_objects(&router, bucket_name, "").await);
        assert_eq!(
            again.keys(),
            listing.keys(),
            "case {case:?}: a second listing must report the same keys, got {:?}",
            again.keys()
        );
    }
}

#[tokio::test]
async fn prefix_marker_and_delimiter_select_what_a_v1_page_reports() {
    let keys: &[&str] = &["a/1", "a/2", "a/b/3", "b/1", "c"];

    // (case, query, the keys the page must report, the common prefixes it must report).
    let rows: &[(&str, &str, &[&str], &[&str])] = &[
        ("no selection", "", keys, &[]),
        ("prefix on a folder", "prefix=a/", &["a/1", "a/2", "a/b/3"], &[]),
        ("prefix matching nothing", "prefix=zz", &[], &[]),
        ("prefix that is a whole key", "prefix=c", &["c"], &[]),
        (
            "prefix shorter than a segment",
            "prefix=a",
            &["a/1", "a/2", "a/b/3"],
            &[],
        ),
        (
            "delimiter rolls up every folder",
            "delimiter=/",
            &["c"],
            &["a/", "b/"],
        ),
        (
            "prefix with a delimiter rolls up what is below it",
            "prefix=a/&delimiter=/",
            &["a/1", "a/2"],
            &["a/b/"],
        ),
        (
            "marker at a stored key is exclusive",
            "marker=a/2",
            &["a/b/3", "b/1", "c"],
            &[],
        ),
        (
            "marker at a key that is not stored",
            "marker=a/10",
            &["a/2", "a/b/3", "b/1", "c"],
            &[],
        ),
        ("marker at the last key", "marker=c", &[], &[]),
        (
            "prefix and marker together",
            "prefix=a/&marker=a/1",
            &["a/2", "a/b/3"],
            &[],
        ),
        (
            // A rolled-up prefix is not greater than a marker naming it, so it is filtered
            // out — which is what stops the page after one that ended on it from reporting
            // the prefix, and every key under it, a second time.
            "a marker naming a rolled-up prefix consumes it",
            "delimiter=/&marker=a/",
            &["c"],
            &["b/"],
        ),
        (
            // The same filter on a marker naming a key UNDER a rolled-up prefix: `a/` is
            // still not lexicographically greater than the marker, so the prefix and the
            // keys it stands for are behind the caller either way.
            "a marker inside a rolled-up prefix consumes it too",
            "delimiter=/&marker=a/1",
            &["c"],
            &["b/"],
        ),
    ];

    let router = router();
    let bucket_name = "v1-listing-selection";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, keys).await;

    for (case, query, expected_keys, expected_prefixes) in rows {
        let answer = list_objects(&router, bucket_name, query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let listing = listing_of(&answer);
        assert_eq!(
            listing.keys(),
            owned(expected_keys),
            "case {case:?}: wrong keys, got {:?}",
            listing.keys()
        );
        assert_eq!(
            listing.common_prefixes,
            owned(expected_prefixes),
            "case {case:?}: wrong common prefixes, got {:?}",
            listing.common_prefixes
        );
        assert_eq!(
            listing.field("IsTruncated"),
            "false",
            "case {case:?}: nothing was left for a next page, got {:?}",
            listing.field("IsTruncated")
        );
        assert_eq!(
            listing.field("NextMarker"),
            "",
            "case {case:?}: an untruncated page owes no NextMarker, got {:?}",
            listing.field("NextMarker")
        );
    }

    // The marker the request carried is echoed back; a request that named none echoes
    // nothing.
    for (case, query, expected) in [
        ("a marker was sent", "marker=a/2", "a/2"),
        ("no marker was sent", "", ""),
    ] {
        let listing = listing_of(&list_objects(&router, bucket_name, query).await);
        assert_eq!(
            listing.field("Marker"),
            expected,
            "case {case:?}: Marker is included in the response if it was sent with the \
             request, got {:?}",
            listing.field("Marker")
        );
    }
}

#[tokio::test]
async fn max_keys_bounds_a_v1_page_and_the_marker_resumes_it() {
    let keys: &[&str] = &["k1", "k2", "k3", "k4", "k5"];
    let router = router();
    let bucket_name = "v1-listing-pages";
    bucket(&router, bucket_name).await;
    let written = store_keys(&router, bucket_name, keys).await;
    let every_key = sorted_keys(&written);

    // Page sizes below, at and above the number of stored keys: every one of them must
    // walk the whole listing, in order, with each page no larger than it asked for. With
    // no delimiter the page owes no `NextMarker`, so the client resumes from the last
    // `Key` it was given.
    for page_size in [1usize, 2, 3, 5, 7] {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &marker {
                None => format!("max-keys={page_size}"),
                Some(marker) => format!("max-keys={page_size}&marker={marker}"),
            };
            let answer = list_objects(&router, bucket_name, &query).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size}, page {pages}: expected 200, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            let listing = listing_of(&answer);
            pages += 1;
            assert!(
                listing.keys().len() <= page_size,
                "case page size {page_size}, page {pages}: a page may not exceed MaxKeys, got \
                 {:?}",
                listing.keys()
            );
            assert_eq!(
                listing.field("MaxKeys"),
                page_size.min(1000).to_string(),
                "case page size {page_size}, page {pages}: MaxKeys is what the page was \
                 bounded by, got {:?}",
                listing.field("MaxKeys")
            );
            walked.extend(listing.keys());

            let truncated = listing.field("IsTruncated");
            assert_eq!(
                truncated,
                (walked.len() < every_key.len()).to_string(),
                "case page size {page_size}, page {pages}: truncated exactly while keys remain, \
                 got {truncated:?} after {} of {} keys",
                walked.len(),
                every_key.len()
            );
            assert_eq!(
                listing.field("NextMarker"),
                "",
                "case page size {page_size}, page {pages}: NextMarker is returned only for a \
                 listing that named a delimiter, got {:?}",
                listing.field("NextMarker")
            );
            if truncated == "true" {
                marker = Some(
                    listing
                        .keys()
                        .last()
                        .cloned()
                        .expect("a truncated page without a delimiter reported at least one key"),
                );
            } else {
                break;
            }
            assert!(
                pages <= every_key.len() + 1,
                "case page size {page_size}: paging must make progress, walked {walked:?}"
            );
        }
        assert_eq!(
            walked, every_key,
            "case page size {page_size}: paging must walk every key once, in order, got {walked:?}"
        );
        assert_eq!(
            pages,
            every_key.len().div_ceil(page_size),
            "case page size {page_size}: a page is filled before the next one starts, took \
             {pages} pages"
        );
    }

    // `max-keys=0` asks for no entries: an empty page, and NOT a truncated one — a marker
    // there would make no progress.
    let empty = listing_of(&list_objects(&router, bucket_name, "max-keys=0").await);
    assert!(
        empty.keys().is_empty(),
        "case max-keys=0: no entries are returned, got {:?}",
        empty.keys()
    );
    assert_eq!(
        empty.field("IsTruncated"),
        "false",
        "case max-keys=0: an empty page is not a truncated one, got {:?}",
        empty.field("IsTruncated")
    );
}

#[tokio::test]
async fn a_delimited_v1_page_hands_back_the_next_marker_to_resume_from() {
    let router = router();
    let bucket_name = "v1-listing-pages-folded";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, &["x/1", "x/2", "y/1", "z"]).await;

    // Page sizes 1 and 2 over the same four keys, which fold into two rolled-up prefixes
    // and one plain key: each page is bounded by `max-keys` whichever kind of entry fills
    // it, and `NextMarker` names the LAST entry the page reported — including a rolled-up
    // prefix, which appears under no `Key` at all, which is the case the element exists
    // for.
    let expectations: &[(usize, &[DelimitedPage])] = &[
        (
            1,
            &[
                (&[], &["x/"], "x/"),
                (&[], &["y/"], "y/"),
                (&["z"], &[], ""),
            ],
        ),
        (2, &[(&[], &["x/", "y/"], "y/"), (&["z"], &[], "")]),
    ];

    for (page_size, pages) in expectations {
        let mut marker: Option<String> = None;
        for (index, (expected_keys, expected_prefixes, next_marker)) in pages.iter().enumerate() {
            let query = match &marker {
                None => format!("delimiter=/&max-keys={page_size}"),
                Some(marker) => format!("delimiter=/&max-keys={page_size}&marker={marker}"),
            };
            let answer = list_objects(&router, bucket_name, &query).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size}, page {index}: expected 200, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            let listing = listing_of(&answer);
            assert_eq!(
                page_of(&answer),
                (owned(expected_keys), owned(expected_prefixes)),
                "case page size {page_size}, page {index}: wrong page, got {:?}",
                page_of(&answer)
            );
            assert_eq!(
                listing.field("Delimiter"),
                "/",
                "case page size {page_size}, page {index}: the delimiter is echoed, got {:?}",
                listing.field("Delimiter")
            );
            assert_eq!(
                listing.field("IsTruncated"),
                (!next_marker.is_empty()).to_string(),
                "case page size {page_size}, page {index}: truncated exactly while entries \
                 remain, got {:?}",
                listing.field("IsTruncated")
            );
            assert_eq!(
                listing.field("NextMarker"),
                *next_marker,
                "case page size {page_size}, page {index}: NextMarker names the last entry \
                 this page reported, got {:?}",
                listing.field("NextMarker")
            );
            marker = (!next_marker.is_empty()).then(|| (*next_marker).to_string());
        }
        assert!(
            marker.is_none(),
            "case page size {page_size}: the walk must end on an untruncated page"
        );
    }
}

#[tokio::test]
async fn the_encoding_type_a_v1_listing_asks_for_is_what_its_key_names_carry() {
    let router = router();
    let bucket_name = "v1-listing-encoded";
    bucket(&router, bucket_name).await;
    // Parentheses are the characters the model's own example names — "the object
    // `test_file(3).png` will appear as `test_file%283%29.png`" — and they travel in a
    // request line unescaped, so both keys can be written over the wire.
    store_keys(&router, bucket_name, &["sub(x)/y", "test_file(3).png"]).await;

    let (keys, prefixes) = page_of(&list_objects(&router, bucket_name, "delimiter=/").await);
    assert_eq!(
        (keys, prefixes),
        (owned(&["test_file(3).png"]), owned(&["sub(x)/"])),
        "case \"no encoding type\": every key name is reported verbatim"
    );

    let answer = list_objects(
        &router,
        bucket_name,
        "delimiter=/&marker=sub(x)&encoding-type=url",
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"encoding-type=url\": a listing that asks for an encoding type is 200, not a \
         refusal, got {} with body {:?}",
        answer.status,
        text(&answer)
    );
    let listing = listing_of(&answer);
    assert_eq!(
        page_of(&answer),
        (
            owned(&["test_file%283%29.png"]),
            owned(&["sub%28x%29/"])
        ),
        "case \"encoding-type=url\": every key name is percent-encoded per UTF-8 code value, \
         got {:?}",
        page_of(&answer)
    );
    for (element, expected) in [
        ("EncodingType", "url"),
        ("Delimiter", "/"),
        ("Marker", "sub%28x%29"),
    ] {
        assert_eq!(
            listing.field(element),
            expected,
            "case \"encoding-type=url\": {element} must be {expected:?}, got {:?}",
            listing.field(element)
        );
    }
}

#[tokio::test]
async fn a_v1_listing_parameter_this_emulator_cannot_serve_is_refused_not_guessed() {
    let router = router();
    let bucket_name = "v1-listing-refusals";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, &["one", "two"]).await;

    // (case, query, status, error code). `encoding-type=url` is NOT here: it is served
    // above, because it decides how this listing's key names are written rather than which
    // entries it reports. A value the model's enum does not declare IS refused, because
    // the response would otherwise echo an encoding the keys do not carry.
    let rows: &[(&str, &str, u16, &str)] = &[
        ("negative max-keys", "max-keys=-1", 400, "InvalidArgument"),
        (
            "an encoding type the model does not declare",
            "encoding-type=base64",
            400,
            "InvalidArgument",
        ),
    ];
    for (case, query, status, code) in rows {
        let answer = list_objects(&router, bucket_name, query).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: expected {code}, got {:?}",
            answer.error_code()
        );
    }

    // The parameters this emulator holds no stored fact for are HEADERS: restore state no
    // stored object here has, and a Requester Pays charge nothing here models. Each keeps
    // the 501 that means "nobody has written it yet".
    for (case, header) in [
        (
            "optional object attributes",
            ("x-amz-optional-object-attributes", "RestoreStatus"),
        ),
        ("request payer", ("x-amz-request-payer", "requester")),
    ] {
        let answer = list_objects_with(&router, bucket_name, "", &[header]).await;
        assert_eq!(
            answer.status, 501,
            "case {case:?}: expected HTTP 501, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "NotImplemented",
            "case {case:?}: expected NotImplemented, got {:?}",
            answer.error_code()
        );
    }

    // An expected bucket owner that is not the owner is access denied; the real owner is
    // served.
    let denied = list_objects_with(
        &router,
        bucket_name,
        "",
        &[("x-amz-expected-bucket-owner", "111122223333")],
    )
    .await;
    assert_eq!(
        denied.status, 403,
        "case \"another account as the expected bucket owner\": expected HTTP 403, got {} with \
         body {:?}",
        denied.status,
        text(&denied)
    );
    assert_eq!(
        denied.error_code(),
        "AccessDenied",
        "case \"another account as the expected bucket owner\": expected AccessDenied, got {:?}",
        denied.error_code()
    );
    let allowed = list_objects_with(
        &router,
        bucket_name,
        "",
        &[("x-amz-expected-bucket-owner", engine::emulator_account_id())],
    )
    .await;
    assert_eq!(
        allowed.status, 200,
        "case \"this account as the expected bucket owner\": expected HTTP 200, got {} with \
         body {:?}",
        allowed.status,
        text(&allowed)
    );

    // The refusals changed nothing: the listing still reports both keys.
    let listing = listing_of(&list_objects(&router, bucket_name, "").await);
    assert_eq!(
        listing.keys(),
        owned(&["one", "two"]),
        "a refused listing leaves the store alone, got {:?}",
        listing.keys()
    );

    // A listing of a bucket that was never created is not an empty listing.
    let absent = list_objects(&router, "bucket-that-was-never-created", "").await;
    assert_eq!(
        absent.status, 404,
        "listing a bucket that does not exist is not found, got {} with body {:?}",
        absent.status,
        text(&absent)
    );
    assert_eq!(
        absent.error_code(),
        "NoSuchBucket",
        "it answers NoSuchBucket, got {:?}",
        absent.error_code()
    );
}
