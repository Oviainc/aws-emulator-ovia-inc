// SPDX-License-Identifier: CC0-1.0

//! S3 Express One Zone: what the two bucket LISTINGS report, and what a listing of the
//! objects IN a directory bucket may ask for.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate already own), so what is pinned is the
//! wire behaviour — status, error code, error message and the names a listing reports.
//!
//! The rules under test, with their sources:
//!
//! * the two kinds of bucket are reported by two different operations, each on its own
//!   endpoint: `ListBuckets` "returns a list of all buckets owned by the authenticated
//!   sender of the request" and "is not supported for directory buckets", while
//!   `ListDirectoryBuckets` "returns a list of all Amazon S3 directory buckets owned by the
//!   authenticated sender of the request" (their own documentation) — so neither listing
//!   reports the other's buckets. The directory-bucket listing is served by
//!   `s3express-control.<region>.amazonaws.com`, whose signing name is `s3express`, which is
//!   what tells the two apart on one emulator host;
//! * that listing pages by `MaxDirectoryBuckets` (`range {min 0, max 1000}`) and resumes at
//!   the `ContinuationToken` the previous page handed back, which "is obfuscated and is not a
//!   real bucket name";
//! * a listing of the objects in a directory bucket may ask only for what that bucket type
//!   serves: "for directory buckets, / is the only supported delimiter"
//!   (`ListObjectsV2Request$Delimiter`), `StartAfter`'s "functionality is not supported for
//!   directory buckets" (`$StartAfter`), and "only prefixes that end in a delimiter (/) are
//!   supported" (`$Prefix`) — each refused with the code, status and sentence S3 answers it
//!   with, and each served as asked on a GENERAL PURPOSE bucket, which is the other half of
//!   the same rule.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes_signed, Answer};
use super::directory_bucket_sessions_and_appends::directory_bucket;
use super::object_and_tagging_roundtrip::{bucket, put_object, text};

/// The credential scope a client of the S3 EXPRESS control endpoint presents: the signing
/// name `s3express`, which is what tells that endpoint's `GET /` (the directory-bucket
/// listing) from the general purpose endpoint's. Neither the key id nor the signature is a
/// credential here (this suite's router verifies none, as the other suites' do).
const S3_EXPRESS_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/s3express/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The host the S3 Express control endpoint is addressed as.
const S3_EXPRESS_CONTROL_HOST: &str = "s3express-control.us-east-1.amazonaws.com";

/// `GET /` signed for the S3 EXPRESS signing name — `ListDirectoryBuckets`, as a client
/// addressing `s3express-control.<region>.amazonaws.com` sends it. Driven through the one
/// place this crate's harness puts a request on the wire.
async fn list_directory_buckets(router: &MultiServiceRouter, query: &str) -> Answer {
    send_bytes_signed(
        router,
        S3_EXPRESS_AUTHORIZATION,
        "GET",
        S3_EXPRESS_CONTROL_HOST,
        &format!("/{query}"),
        &[],
        b"",
    )
    .await
}

/// Every `<Name>` element of a bucket listing's body, in the order the document carries them
/// — read through the harness's own repeated-element reader ([`Answer::elements`]).
fn listed_names(answer: &Answer) -> Vec<String> {
    answer.elements("Name")
}

/// The `<ContinuationToken>` a page handed back, when it handed one back.
fn continuation_token(answer: &Answer) -> Option<String> {
    answer
        .element("ContinuationToken")
        .filter(|token| !token.is_empty())
}

/// RULE: the directory-bucket listing reports this account's directory buckets and nothing
/// else, and the general purpose listing reports its general purpose buckets and nothing
/// else.
///
/// The rows differ along the kind of bucket (directory in three different zones, general
/// purpose in two Regions' namespaces), so what is asserted is the PARTITION of the account's
/// buckets between the two listings rather than one name appearing in one of them.
#[tokio::test]
async fn each_listing_reports_its_own_kind_of_bucket() {
    let router = router();
    // (case, name, zone id when it is a directory bucket)
    let rows: [(&str, &str, Option<&str>); 5] = [
        ("a directory bucket in use1-az4", "dirlist-a--use1-az4--x-s3", Some("use1-az4")),
        ("a directory bucket in use1-az5", "dirlist-b--use1-az5--x-s3", Some("use1-az5")),
        ("a directory bucket in usw2-az1", "dirlist-c--usw2-az1--x-s3", Some("usw2-az1")),
        ("a general purpose bucket", "dirlist-general-purpose", None),
        ("another general purpose bucket", "dirlist-plain-bucket", None),
    ];
    for (case, name, zone) in rows {
        match zone {
            Some(zone) => directory_bucket(&router, name, zone).await,
            None => bucket(&router, name).await,
        }
        let _ = case;
    }

    let expected_directory: Vec<String> = rows
        .iter()
        .filter(|(_, _, zone)| zone.is_some())
        .map(|(_, name, _)| name.to_string())
        .collect();
    let expected_general_purpose: Vec<String> = rows
        .iter()
        .filter(|(_, _, zone)| zone.is_none())
        .map(|(_, name, _)| name.to_string())
        .collect();

    // The directory-bucket listing, reached both ways a client names it: by the signing name
    // of the endpoint that serves it, and by the `x-id` discriminator the model's own URI
    // pattern carries (`/?x-id=ListDirectoryBuckets`).
    for (case, query) in [
        ("signed for s3express", ""),
        ("naming the model's x-id", "?x-id=ListDirectoryBuckets"),
    ] {
        let listed = list_directory_buckets(&router, query).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            listed.status,
            text(&listed)
        );
        assert_eq!(
            listed_names(&listed),
            expected_directory,
            "case {case:?}: ListDirectoryBuckets must report the account's directory buckets \
             in name order, got {:?}",
            listed_names(&listed)
        );
    }

    let general_purpose = send(&router, "GET", "/", &[], "").await;
    assert_eq!(
        general_purpose.status, 200,
        "case \"the general purpose listing\": expected HTTP 200, got {} with body {:?}",
        general_purpose.status,
        text(&general_purpose)
    );
    assert_eq!(
        listed_names(&general_purpose),
        expected_general_purpose,
        "case \"the general purpose listing\": ListBuckets must report the account's general \
         purpose buckets alone, got {:?}",
        listed_names(&general_purpose)
    );

    // A request that merely LOOKS root-path at the routing seam is not this listing: a Zonal
    // `GET /{Bucket}?list-type=2` against a directory bucket this emulator does not hold
    // carries that bucket in its HOST and nothing in its path-style address, and its query
    // names another operation's listing — so it is answered about the address it named and
    // never with the account's directory buckets.
    let absent = send_bytes_signed(
        &router,
        S3_EXPRESS_AUTHORIZATION,
        "GET",
        "dirlist-absent--use1-az4--x-s3.s3express-use1-az4.us-east-1.amazonaws.com",
        "/?list-type=2",
        &[],
        b"",
    )
    .await;
    for name in &expected_directory {
        assert!(
            !listed_names(&absent).contains(name),
            "case \"a zonal listing of a bucket this emulator does not hold\": it must not \
             report the account's directory buckets, got {:?}",
            listed_names(&absent)
        );
    }
}

/// RULE: the directory-bucket listing pages by `MaxDirectoryBuckets` and resumes at the
/// `ContinuationToken` the previous page handed back, so paging through it reports every
/// directory bucket exactly once whatever the page size.
///
/// The rows differ along the page size: each bound the model's `range {min 0, max 1000}`
/// admits, one inside it, and the absent member.
#[tokio::test]
async fn paging_the_directory_bucket_listing_reports_every_bucket_once() {
    let router = router();
    let zones = ["use1-az4", "use1-az5", "usw2-az1", "usw2-az2"];
    let names: Vec<String> = zones
        .iter()
        .enumerate()
        .map(|(index, zone)| format!("dirpage-{index}--{zone}--x-s3"))
        .collect();
    for (name, zone) in names.iter().zip(zones) {
        directory_bucket(&router, name, zone).await;
    }

    // (case, the `max-directory-buckets` the request names, the most buckets a page may then
    // carry). The model's minimum is its own case below: a page of zero carries no bucket, so
    // paging with it never advances.
    let rows: [(&str, Option<i32>, usize); 4] = [
        ("one bucket a page", Some(1), 1),
        ("two buckets a page", Some(2), 2),
        ("the model's maximum", Some(1_000), 4),
        ("no page size at all", None, 4),
    ];
    for (case, page_size, page_limit) in rows {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("max-directory-buckets={size}"));
            }
            if let Some(resume) = token.as_deref() {
                query.push(format!("continuation-token={resume}"));
            }
            let page = list_directory_buckets(
                &router,
                &match query.is_empty() {
                    true => String::new(),
                    false => format!("?{}", query.join("&")),
                },
            )
            .await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected HTTP 200 for page {pages}, got {} with body {:?}",
                page.status,
                text(&page)
            );
            let listed = listed_names(&page);
            assert!(
                listed.len() <= page_limit,
                "case {case:?}: a page may carry at most {page_limit} buckets, got {:?}",
                listed
            );
            collected.extend(listed);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case:?}: paging must terminate, got {pages} pages for {} buckets",
                names.len()
            );
            match continuation_token(&page) {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            collected, names,
            "case {case:?}: paging must report every directory bucket once, in name order, \
             got {collected:?}"
        );
    }

    // The model's minimum page size, which the analogous listing of this service answers the
    // same way: a page that carries no bucket, and a token saying there is more.
    let empty = list_directory_buckets(&router, "?max-directory-buckets=0").await;
    assert_eq!(
        (empty.status, listed_names(&empty).len()),
        (200, 0),
        "case \"the model's minimum page size\": expected HTTP 200 with no bucket, got {} and \
         {:?}",
        empty.status,
        listed_names(&empty)
    );
    assert!(
        continuation_token(&empty).is_some(),
        "case \"the model's minimum page size\": a page that carried none of the {} buckets \
         must hand back the cursor they start at, got {:?}",
        names.len(),
        text(&empty)
    );

    // A token this listing did not mint names no cursor into it, and is refused rather than
    // read as a position the caller chose.
    let refused = list_directory_buckets(&router, "?continuation-token=not-a-token").await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "InvalidArgument"),
        "case \"a continuation token this listing did not mint\": expected 400 \
         InvalidArgument, got {} {:?} with body {:?}",
        refused.status,
        refused.error_code(),
        text(&refused)
    );
}

/// RULE: a listing of the objects in a DIRECTORY bucket may ask only for what that bucket
/// type serves — one delimiter, no start-after, and a prefix that ends in the delimiter —
/// and the same request against a general purpose bucket is served as asked.
///
/// The rows differ along the member asked with and its value: the delimiter S3 Express
/// supports and two it does not, a prefix with and without the delimiter, and `start-after`
/// at all.
#[tokio::test]
async fn a_directory_bucket_listing_serves_only_what_that_bucket_type_supports() {
    let router = router();
    let directory = "dirlisting--use1-az4--x-s3";
    let general_purpose = "dirlisting-general-purpose";
    directory_bucket(&router, directory, "use1-az4").await;
    bucket(&router, general_purpose).await;
    for name in [directory, general_purpose] {
        for key in ["k/part-1", "k/part-2"] {
            let stored = put_object(&router, name, key, b"body", &[]).await;
            assert_eq!(
                stored.status, 200,
                "the object {key:?} must be stored in {name:?} first, got {} with body {:?}",
                stored.status,
                text(&stored)
            );
        }
    }

    // (case, the listing's query, the refusal a DIRECTORY bucket answers it with as
    // `(code, status, message)`, or `None` for a listing it serves)
    type Refusal = Option<(&'static str, u16, &'static str)>;
    let rows: [(&str, &str, Refusal); 7] = [
        ("no member at all", "", None),
        ("the one delimiter directory buckets support", "&delimiter=%2F", None),
        (
            "a prefix that ends in the delimiter",
            "&prefix=k%2F",
            None,
        ),
        (
            "a delimiter directory buckets do not support",
            "&delimiter=-",
            Some((
                "InvalidRequest",
                400,
                "This bucket only supports '/' as a delimiter",
            )),
        ),
        (
            "another delimiter directory buckets do not support",
            "&delimiter=%7C",
            Some((
                "InvalidRequest",
                400,
                "This bucket only supports '/' as a delimiter",
            )),
        ),
        (
            "a prefix that does not end in the delimiter",
            "&prefix=k",
            Some((
                "InvalidRequest",
                400,
                "This bucket does not support a prefix that does not end in a delimiter. \
                 Specify a prefix path ending with a delimiter and try again.",
            )),
        ),
        (
            "a start-after key",
            "&start-after=k%2Fpart-1",
            Some((
                "NotSupported",
                501,
                "This bucket does not support start-after query parameter for ListObjectsV2 \
                 API.",
            )),
        ),
    ];
    for (case, query, refusal) in rows {
        let listed = send(
            &router,
            "GET",
            &format!("/{directory}?list-type=2{query}"),
            &[],
            "",
        )
        .await;
        match refusal {
            None => assert_eq!(
                listed.status, 200,
                "case {case:?}: a directory bucket serves this listing, got {} with body {:?}",
                listed.status,
                text(&listed)
            ),
            Some((code, status, message)) => assert_eq!(
                (
                    listed.status,
                    listed.error_code().as_str(),
                    listed.error_message().as_str()
                ),
                (status, code, message),
                "case {case:?}: expected {status} {code:?} {message:?}, got {} {:?} {:?}",
                listed.status,
                listed.error_code(),
                listed.error_message()
            ),
        }

        // The other half of the rule: a GENERAL PURPOSE bucket serves every one of them, so
        // what is refused is the directory bucket's own constraint and not the member.
        let served = send(
            &router,
            "GET",
            &format!("/{general_purpose}?list-type=2{query}"),
            &[],
            "",
        )
        .await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: a general purpose bucket serves this listing, got {} with body \
             {:?}",
            served.status,
            text(&served)
        );
    }
}
