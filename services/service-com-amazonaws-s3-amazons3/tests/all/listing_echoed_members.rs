// SPDX-License-Identifier: CC0-1.0

//! What a listing of a bucket ECHOES back about the request that asked for it.
//!
//! Two rules, over every listing of what a bucket holds (`ListObjects`, `ListObjectsV2`,
//! `ListObjectVersions`, `ListMultipartUploads`):
//!
//! 1. the elements that say WHERE the listing began — the prefix it selected by and the
//!    marker(s) it started after — are part of EVERY answer, carrying the empty value for a
//!    request that named none, because a client paging with them reads them out of the
//!    response rather than out of its own request (`ListObjectsOutput$Prefix` "keys that
//!    begin with the indicated prefix" and `$Marker` "indicates where in the bucket listing
//!    begins", `ListObjectVersionsOutput$KeyMarker`/`$VersionIdMarker` "marks the last key
//!    returned"/"marks the last version of the key returned",
//!    `ListMultipartUploadsOutput$KeyMarker` "the key at or after which the listing began").
//!    The members the model's own documentation makes conditional stay conditional: a
//!    `Delimiter` is "absent from the response" when the request stated none
//!    (`ListMultipartUploadsOutput$Delimiter`), and so is the `Prefix` of the uploads
//!    listing, "when a prefix is provided in the request, this field contains the specified
//!    prefix";
//! 2. the page-size element echoes the number the REQUEST asked for, while the published
//!    1,000-entry ceiling bounds only how many entries the page carries — "the response
//!    might contain fewer keys but will never contain more"
//!    (`ListObjectsV2Output$MaxKeys`). A client told the ceiling instead cannot tell a
//!    clamped request from the server-side default and mis-sizes its paging loop. The
//!    uploads listing is the one whose own member reports the bound rather than the request
//!    — "maximum number of multipart uploads that could have been included in the response"
//!    (`ListMultipartUploadsOutput$MaxUploads`) — and this suite pins it that way.
//!
//! Beside them, the third rule this suite pins: a listing refuses a value one of its own
//! query parameters cannot carry with S3's published sentence for that parameter and the
//! `<ArgumentName>` / `<ArgumentValue>` pair naming the parameter as the API publishes it —
//! the refusal of an `encoding-type` that names no encoding method, and of a
//! `continuation-token` no listing issued, whose token the document does not echo because it
//! "is obfuscated and is not a real key".
//!
//! The tables span what each rule generalises over: WHICH listing answers (all four, by
//! their four addresses), whether the request stated each echoed member, the page sizes
//! below, at and past the published ceiling, and the values an `encoding-type` may carry
//! that are not the one the model declares.
//!
//! Every call goes over the wire through the harnesses the other object suites own, so what
//! is pinned is the wire document.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::object_and_tagging_roundtrip::{bucket, text};
use super::object_storage::{listing_of, store_keys};

/// The keys every bucket in this suite holds: two under one prefix and one with none, so a
/// prefix-selected listing reports part of the bucket and a plain one reports all of it.
const KEYS: &[&str] = &["notes/april.txt", "notes/may.txt", "plain.txt"];

/// The one `EncodingType` the model declares, which every listing serves.
const ENCODING_TYPE_URL: &str = "url";

/// The code and status a value a request field cannot carry is refused with.
const INVALID_ARGUMENT: &str = "InvalidArgument";

/// The published sentence a listing refuses an `encoding-type` outside the declared set
/// with, and the parameter the document names it against.
const INVALID_ENCODING_METHOD_MESSAGE: &str = "Invalid Encoding Method specified in Request";
const ENCODING_TYPE_PARAMETER: &str = "encoding-type";

/// One listing of what a bucket holds: how it is addressed, and which echoed elements its
/// answer carries whatever the request stated.
struct Surface {
    /// The operation, named in every assertion this row makes.
    call: &'static str,
    /// The query that addresses it — the subresource or discriminator, with no selection.
    address: &'static str,
    /// The elements that are part of every answer, each empty for a request that stated
    /// none, paired with the query parameter that states it (empty where this listing binds
    /// no such parameter, which is every marker but the key one).
    echoed: &'static [(&'static str, &'static str)],
    /// The element this listing reports its page size in, and the parameter that asks for
    /// one.
    page_size: (&'static str, &'static str),
    /// Whether that element reports the number the REQUEST asked for. True for the three
    /// listings of a bucket's contents and versions, whose `MaxKeys` "sets the maximum
    /// number of keys returned in the response"; false for the uploads listing, whose own
    /// member is the bound the page was computed under — "maximum number of multipart
    /// uploads that COULD HAVE BEEN included in the response"
    /// (`ListMultipartUploadsOutput$MaxUploads`).
    page_size_echoes_request: bool,
}

/// The four listings of what a bucket holds, each with the echoed elements its own output
/// members publish.
const SURFACES: &[Surface] = &[
    Surface {
        call: "ListObjects",
        address: "",
        echoed: &[("Prefix", "prefix"), ("Marker", "marker")],
        page_size: ("MaxKeys", "max-keys"),
        page_size_echoes_request: true,
    },
    Surface {
        call: "ListObjectsV2",
        address: "list-type=2",
        echoed: &[("Prefix", "prefix")],
        page_size: ("MaxKeys", "max-keys"),
        page_size_echoes_request: true,
    },
    Surface {
        call: "ListObjectVersions",
        address: "versions",
        echoed: &[
            ("Prefix", "prefix"),
            ("KeyMarker", "key-marker"),
            ("VersionIdMarker", ""),
        ],
        page_size: ("MaxKeys", "max-keys"),
        page_size_echoes_request: true,
    },
    Surface {
        call: "ListMultipartUploads",
        address: "uploads",
        echoed: &[("KeyMarker", "key-marker"), ("UploadIdMarker", "")],
        page_size: ("MaxUploads", "max-uploads"),
        page_size_echoes_request: false,
    },
];

/// `GET /<bucket>?<address><query>` — one listing, however it is addressed.
async fn list(
    router: &MultiServiceRouter,
    bucket_name: &str,
    surface: &Surface,
    query: &str,
) -> Answer {
    let uri = match (surface.address.is_empty(), query.is_empty()) {
        (true, true) => format!("/{bucket_name}"),
        (true, false) => format!("/{bucket_name}?{}", query.trim_start_matches('&')),
        (false, _) => format!("/{bucket_name}?{}{query}", surface.address),
    };
    send(router, "GET", &uri, &[], "").await
}

/// The text of the first `<name>` element of `answer`'s body, or `None` when the document
/// carries no such element at all — which is the difference this suite is about: an element
/// carrying the empty value is `Some("")`.
fn element(answer: &Answer, name: &str) -> Option<String> {
    answer.element(name)
}

#[tokio::test]
async fn every_listing_echoes_where_it_began_even_when_the_request_states_nothing() {
    let router = router();
    let bucket_name = "listing-echoed-members";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, KEYS).await;

    for surface in SURFACES {
        // (case, the extra query, whether each echoed member was stated). A request that
        // states nothing, one that states the prefix, one that states the key marker, and
        // one that states both — the echo must be present in all four, carrying what the
        // request stated or the empty value.
        let stated: Vec<(&str, String)> = vec![
            ("no selection at all", String::new()),
            ("a prefix", "&prefix=notes/".to_string()),
            ("a marker", "&key-marker=notes/april.txt".to_string()),
            (
                "a prefix and a marker",
                "&prefix=notes/&key-marker=notes/april.txt".to_string(),
            ),
        ];
        for (selection, query) in stated {
            // Each row's query is written with the parameter names THIS listing binds, so a
            // marker row addresses `marker` on the revision that calls it that.
            let query = match surface.echoed.iter().any(|(_, parameter)| *parameter == "marker") {
                true => query.replace("key-marker=", "marker="),
                false => query,
            };
            let case = format!("{}/{selection}", surface.call);
            let answer = list(&router, bucket_name, surface, &query).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: a listing of a bucket that exists answers 200, got {} with \
                 body {:?}",
                answer.status,
                text(&answer)
            );
            for (name, parameter) in surface.echoed {
                // What the request stated for this member, or the empty value where it
                // stated none — including a member this listing binds no parameter for.
                let expected = match parameter.is_empty() {
                    true => String::new(),
                    false => query
                        .split('&')
                        .filter_map(|pair| pair.split_once('='))
                        .find(|(key, _)| key == parameter)
                        .map(|(_, value)| value.to_string())
                        .unwrap_or_default(),
                };
                assert_eq!(
                    element(&answer, name).as_deref(),
                    Some(expected.as_str()),
                    "case {case:?}: <{name}> is part of every answer and carries {expected:?} \
                     here, got {:?} in {:?}",
                    element(&answer, name),
                    text(&answer)
                );
            }
            // A `Delimiter` the request did not state is absent from the response, so the
            // rule above is about the members that say where the listing began and not
            // about every optional member.
            assert_eq!(
                element(&answer, "Delimiter"),
                None,
                "case {case:?}: a listing that stated no delimiter reports none, got {:?} in \
                 {:?}",
                element(&answer, "Delimiter"),
                text(&answer)
            );
        }

        // And the uploads listing's own `Prefix` stays conditional, which is what its member
        // publishes: "when a prefix is provided in the request, this field contains the
        // specified prefix".
        if surface.call == "ListMultipartUploads" {
            let plain = list(&router, bucket_name, surface, "").await;
            assert_eq!(
                element(&plain, "Prefix"),
                None,
                "case {}/\"no selection at all\": the uploads listing reports a Prefix only \
                 when the request provided one, got {:?} in {:?}",
                surface.call,
                element(&plain, "Prefix"),
                text(&plain)
            );
            let selected = list(&router, bucket_name, surface, "&prefix=notes/").await;
            assert_eq!(
                element(&selected, "Prefix").as_deref(),
                Some("notes/"),
                "case {}/\"a prefix\": the uploads listing echoes the prefix it was given, \
                 got {:?} in {:?}",
                surface.call,
                element(&selected, "Prefix"),
                text(&selected)
            );
        }
    }
}

/// The published ceiling on one page of a listing: "by default, the action returns up to
/// 1,000 key names. The response might contain fewer keys but will never contain more"
/// (`ListObjectsV2Request$MaxKeys`).
const PAGE_CEILING: i64 = 1000;

#[tokio::test]
async fn a_listing_echoes_the_page_size_the_request_asked_for() {
    let router = router();
    let bucket_name = "listing-echoed-page-size";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, KEYS).await;

    // Page sizes spanning the class: one entry, fewer than the bucket holds, exactly what it
    // holds, more than it holds, one below the published ceiling, the ceiling itself, one
    // past it, a value far past it, and the largest the member admits — plus `0`, which asks
    // for no entries at all. A request that states none is answered with the default, which
    // is the ceiling.
    let sizes: &[i64] = &[
        0,
        1,
        2,
        KEYS.len() as i64,
        KEYS.len() as i64 + 1,
        PAGE_CEILING - 1,
        PAGE_CEILING,
        PAGE_CEILING + 1,
        5_000,
        i32::MAX as i64,
    ];

    for surface in SURFACES {
        let (element_name, parameter) = surface.page_size;
        for size in sizes {
            let case = format!("{}/{parameter}={size}", surface.call);
            let answer = list(&router, bucket_name, surface, &format!("&{parameter}={size}")).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: a page size the member admits is served, got {} with body \
                 {:?}",
                answer.status,
                text(&answer)
            );
            let bound = (*size).min(PAGE_CEILING);
            // The echo is the caller's number where the member reports the request, and
            // the ceiling-bounded page size where it reports the bound the page was
            // computed under.
            let expected = match surface.page_size_echoes_request {
                true => *size,
                false => bound,
            };
            assert_eq!(
                element(&answer, element_name).as_deref(),
                Some(expected.to_string().as_str()),
                "case {case:?}: <{element_name}> reports {expected}, got {:?} in {:?}",
                element(&answer, element_name),
                text(&answer)
            );
            // And the page itself never carries more than the ceiling, nor more than the
            // request asked for: the echo above is what the caller asked for, not a promise
            // about the page.
            if surface.call == "ListObjectsV2" || surface.address.is_empty() {
                let reported = listing_of(&answer).keys().len() as i64;
                assert!(
                    reported <= bound,
                    "case {case:?}: a page may carry at most {bound} entries, got {reported}"
                );
            }
        }

        // A request that asks for no page size at all is answered with the default.
        let case = format!("{}/no {parameter}", surface.call);
        let answer = list(&router, bucket_name, surface, "").await;
        assert_eq!(
            element(&answer, element_name).as_deref(),
            Some(PAGE_CEILING.to_string().as_str()),
            "case {case:?}: a request that asked for no page size is answered with the \
             default, got {:?} in {:?}",
            element(&answer, element_name),
            text(&answer)
        );
    }
}

#[tokio::test]
async fn a_listing_refuses_an_encoding_type_by_naming_the_parameter_and_the_value() {
    let router = router();
    let bucket_name = "listing-encoding-method";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, KEYS).await;

    // Values an `encoding-type` may carry that name no encoding method: a word, the declared
    // value in the wrong case (the enum is exact), another encoding a client might guess at,
    // and the empty value.
    let rejected: &[(&str, &str)] = &[
        ("a word that is no encoding method", "bogus"),
        ("the declared value upper-cased", "URL"),
        ("an encoding this service does not write", "base64"),
        ("no value at all", ""),
    ];

    for surface in SURFACES {
        for (why, value) in rejected {
            let case = format!("{}/{why}", surface.call);
            let answer = list(
                &router,
                bucket_name,
                surface,
                &format!("&encoding-type={value}"),
            )
            .await;
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: expected HTTP 400, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                INVALID_ARGUMENT,
                "case {case:?}: expected the {INVALID_ARGUMENT:?} code, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                INVALID_ENCODING_METHOD_MESSAGE,
                "case {case:?}: expected the published sentence \
                 {INVALID_ENCODING_METHOD_MESSAGE:?}, got {:?}",
                answer.error_message()
            );
            assert_eq!(
                element(&answer, "ArgumentName").as_deref(),
                Some(ENCODING_TYPE_PARAMETER),
                "case {case:?}: the envelope must name the parameter it rejected, got {:?} \
                 in {:?}",
                element(&answer, "ArgumentName"),
                text(&answer)
            );
            assert_eq!(
                element(&answer, "ArgumentValue").as_deref(),
                Some(*value),
                "case {case:?}: the envelope must echo the value that parameter carried, got \
                 {:?} in {:?}",
                element(&answer, "ArgumentValue"),
                text(&answer)
            );
        }

        // The ONE value the model declares is served, so every refusal above was about the
        // method named rather than about the parameter being there.
        let case = format!("{}/encoding-type={ENCODING_TYPE_URL}", surface.call);
        let served = list(
            &router,
            bucket_name,
            surface,
            &format!("&encoding-type={ENCODING_TYPE_URL}"),
        )
        .await;
        assert_eq!(
            served.status,
            200,
            "case {case:?}: the declared encoding type is served, got {} with body {:?}",
            served.status,
            text(&served)
        );
        assert_eq!(
            element(&served, "EncodingType").as_deref(),
            Some(ENCODING_TYPE_URL),
            "case {case:?}: a listing that applied an encoding echoes it, got {:?} in {:?}",
            element(&served, "EncodingType"),
            text(&served)
        );
    }
}

#[tokio::test]
async fn a_continuation_token_no_listing_issued_is_refused_by_naming_the_parameter() {
    let router = router();
    let bucket_name = "listing-continuation-token";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, KEYS).await;

    // Tokens no listing of this service issued: a word, hex that decodes to bytes carrying
    // no cursor of this service's own, and the empty value.
    for (why, token) in [
        ("a word that is no token", "not-a-token"),
        ("hex carrying no cursor", "6162636465"),
        ("no value at all", ""),
    ] {
        let case = format!("ListObjectsV2/{why}");
        let answer = send(
            &router,
            "GET",
            &format!("/{bucket_name}?list-type=2&continuation-token={token}"),
            &[],
            "",
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            INVALID_ARGUMENT,
            "case {case:?}: expected the {INVALID_ARGUMENT:?} code, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            "The continuation token provided is incorrect",
            "case {case:?}: expected the published sentence, got {:?}",
            answer.error_message()
        );
        assert_eq!(
            element(&answer, "ArgumentName").as_deref(),
            Some("continuation-token"),
            "case {case:?}: the envelope must name the parameter it rejected, got {:?} in \
             {:?}",
            element(&answer, "ArgumentName"),
            text(&answer)
        );
        // The token is "obfuscated and is not a real key", so the document echoes no value
        // for it: the parameter name is the whole of what the caller has to fix.
        assert_eq!(
            element(&answer, "ArgumentValue"),
            None,
            "case {case:?}: the envelope echoes no token back, got {:?} in {:?}",
            element(&answer, "ArgumentValue"),
            text(&answer)
        );
    }

    // A token the listing itself handed back resumes the walk, so every refusal above was
    // about the token provided rather than about the parameter being there.
    let first = send(
        &router,
        "GET",
        &format!("/{bucket_name}?list-type=2&max-keys=1"),
        &[],
        "",
    )
    .await;
    let page = listing_of(&first);
    let issued = page.field("NextContinuationToken");
    assert!(
        !issued.is_empty(),
        "a truncated page must hand back a continuation token, got body {:?}",
        text(&first)
    );
    let resumed = send(
        &router,
        "GET",
        &format!("/{bucket_name}?list-type=2&continuation-token={issued}"),
        &[],
        "",
    )
    .await;
    assert_eq!(
        resumed.status,
        200,
        "the token this listing issued must resume it, got {} with body {:?}",
        resumed.status,
        text(&resumed)
    );
    assert_eq!(
        listing_of(&resumed).keys(),
        KEYS[1..].iter().map(|key| (*key).to_string()).collect::<Vec<_>>(),
        "the resumed page must report the keys the first page left, got {:?}",
        listing_of(&resumed).keys()
    );
}
