// SPDX-License-Identifier: CC0-1.0

//! Every LEG of a high-level client's DOWNLOAD of an object this emulator holds.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour — status, the bound metadata headers, the bytes — rather than any handler's
//! internals.
//!
//! The rule under test: **a read of an object the store holds is ADMITTED on the input
//! bindings the model declares for it, so every leg of a download — the `HeadObject` that
//! sizes the transfer and the `GetObject`s that fetch it — is answered 200, or 206 for a leg
//! that names a span of the object, with the metadata those bindings carry; the HEAD answer
//! "is identical to the GET response except that there is no response body" (`HeadObject`'s
//! own documentation), and the legs' bytes reassemble the stored object. An address the store
//! holds nothing at answers 404 — never a bare 400, which on a HEAD, carrying no error
//! document, would be the whole answer a client gets and would say the REQUEST was
//! malformed.**
//!
//! That rule is what a transfer manager needs, and it is what a download leg refused for a
//! member the model binds breaks: the AWS CLI's `s3 cp` download path HEADs the key to size
//! the transfer and then GETs it, both legs carrying the checksum mode it asks for — in the
//! case that client writes it — and fanning the fetch out over byte ranges or part numbers for
//! an object above its threshold. One refused leg fails the whole download, and the status is
//! all the caller is told.
//!
//! The rows differ along what the rule generalises over: how the object was stored (one
//! `PutObject`, a multipart upload, under a checksum algorithm and under none), its SIZE (a
//! handful of bytes, and several MiB), the STRATEGY the download is driven with (the whole
//! object at once, byte-range chunks, one leg per part), whether the mode header is sent and
//! the CASE its value is written in, and the ADDRESS form the request arrives in (path-style
//! and virtual-hosted-style, the two this service resolves a bucket from).

use aws_smithy_types::base64;
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, send_bytes_to_host, Answer};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_partial_reads::{multipart_object, seeded_payload};

/// The header a read asks for the stored checksum with, and the one the value it reports
/// travels on.
///
/// Source: `smithy_query(query="operation",operation="HeadObject")` reports
/// `ChecksumMode` bound as `@httpHeader("x-amz-checksum-mode")` on the input and
/// `ChecksumSHA256` bound as `@httpHeader("x-amz-checksum-sha256")` on the output, the same
/// two bindings `GetObject` carries.
const MODE_HEADER: &str = "x-amz-checksum-mode";
/// See [`MODE_HEADER`].
const CHECKSUM_HEADER: &str = "x-amz-checksum-sha256";

/// The `ChecksumAlgorithm` the objects in this suite that were stored under one use.
const ALGORITHM: &str = "SHA256";

/// What a download leg puts in [`MODE_HEADER`]: nothing — a client asking for no checksum —
/// the spelling the AWS CLI's `s3 cp` download path sends, and the one the model's own enum
/// lists (`smithy_query(query="shape",shape="com.amazonaws.s3#ChecksumMode")` reports
/// `values: ["ENABLED"]`).
const MODES: [Option<&str>; 3] = [None, Some("enabled"), Some("ENABLED")];

/// The fewest bytes a part other than the last one may carry: "part size: 5 MiB to 5 GiB.
/// There is no minimum size limit on the last part of your multipart upload" (Amazon S3 User
/// Guide, *Amazon S3 multipart upload limits*).
///
/// Also the chunk the range legs below fan a download out over, so an object of several MiB
/// takes more than one leg whichever strategy fetches it.
const CHUNK_BYTES: usize = 5 * 1024 * 1024;

/// How large the multi-MiB objects here are: one full part plus a short tail, which is the
/// shape every transfer manager's split produces.
const LARGE_BYTES: usize = CHUNK_BYTES + 1024 * 1024;

/// The bytes of the small objects — the shape of the CSV a client fetches to stdout.
const SMALL_BODY: &[u8] = b"id,name\n1,first\n2,second\n";

/// The status HTTP gives an answer carrying PART of the selected representation (RFC 9110,
/// §15.3.7), which is what a leg naming a span of the object is answered with.
const PARTIAL_CONTENT: u16 = 206;

/// The Base64 SHA-256 digest of `data`, as a `Checksum*` member carries it — derived here
/// rather than read back out of the code under test.
fn digest_of(data: &[u8]) -> String {
    base64::encode(Sha256::digest(data))
}

/// How a leg ADDRESSES the object: the two forms this service resolves a bucket from.
#[derive(Debug, Clone, Copy)]
enum Address {
    /// `/<bucket>/<key>` against the Regional endpoint.
    Path,
    /// `/<key>` against `<bucket>.<endpoint>` — what a client sends for an endpoint addressed
    /// by bucket name.
    Host,
}

/// Dispatch `method` at `key` of `bucket`, addressed as `address` says, carrying `query` and
/// `headers`.
async fn dispatch(
    router: &MultiServiceRouter,
    address: Address,
    method: &str,
    bucket: &str,
    key: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    match address {
        Address::Path => {
            send_bytes(
                router,
                method,
                &format!("/{bucket}/{key}{query}"),
                headers,
                b"",
            )
            .await
        }
        Address::Host => {
            send_bytes_to_host(
                router,
                method,
                &format!("{bucket}.s3.us-east-1.amazonaws.com"),
                &format!("/{key}{query}"),
                headers,
                b"",
            )
            .await
        }
    }
}

/// One object the download rows are driven against.
struct Stored {
    /// The case name, for the assertion messages.
    case: &'static str,
    key: &'static str,
    /// Everything the object holds.
    bytes: Vec<u8>,
    /// The parts a multipart upload assembled it from, in order — empty for an object one
    /// write stored whole.
    parts: Vec<Vec<u8>>,
    /// The checksum a read of the WHOLE object reports when it asks for one, or `None` for an
    /// object stored under no checksum algorithm.
    whole_checksum: Option<String>,
}

impl Stored {
    /// The spans a part-scoped read names, in part order: a part's own place in the object
    /// ("effectively performs a 'ranged' GET request for the part specified",
    /// `GetObjectRequest$PartNumber`). An object stored by ONE write has a single part, the
    /// whole object.
    fn part_spans(&self) -> Vec<(usize, usize)> {
        if self.parts.is_empty() {
            return vec![(0, self.bytes.len() - 1)];
        }
        let mut spans = Vec::new();
        let mut first = 0usize;
        for part in &self.parts {
            spans.push((first, first + part.len() - 1));
            first += part.len();
        }
        spans
    }

    /// How many parts a part-scoped read reports as `PartsCount` — "this value is only
    /// returned if you specify `partNumber` in your request and the object was uploaded as a
    /// multipart upload" (`HeadObjectOutput$PartsCount`), so an object one write stored
    /// reports none.
    fn parts_count(&self) -> Option<usize> {
        (!self.parts.is_empty()).then_some(self.parts.len())
    }

    /// The checksum a read of `span` reports: the object's own for a read of the whole of it,
    /// that part's own digest for a read narrowed to exactly one part, and none for any other
    /// span — the value has to describe the bytes the answer carries.
    fn checksum_of(&self, span: Option<(usize, usize)>) -> Option<String> {
        self.whole_checksum.as_ref()?;
        let Some((first, last)) = span else {
            return self.whole_checksum.clone();
        };
        if first == 0 && last + 1 >= self.bytes.len() {
            return self.whole_checksum.clone();
        }
        self.part_spans()
            .into_iter()
            .zip(self.parts.iter())
            .find(|(part_span, _)| *part_span == (first, last))
            .map(|(_, part)| digest_of(part))
    }
}

/// How a download fetches the object.
#[derive(Debug, Clone, Copy)]
enum Strategy {
    /// One leg for the whole object.
    Whole,
    /// One leg per byte-range chunk, which is how a transfer manager fans a large download
    /// out.
    Chunked,
    /// One leg per part, which is how a part-aware client fans the same download out.
    PartScoped,
}

/// One leg of a download: how it names the span it asks for, and which span that is.
struct Leg {
    /// The case name, for the assertion messages.
    case: String,
    /// The query string the leg carries, or empty when it carries none.
    query: String,
    /// The `Range` header the leg carries, when it names its span that way.
    range: Option<String>,
    /// The span of the object the leg asks for, as `(first, last)` inclusive, or `None` for a
    /// leg that asks for the whole of it.
    span: Option<(usize, usize)>,
    /// Whether the leg named a `partNumber`.
    part_scoped: bool,
}

/// The legs `strategy` drives a download of `object` over — together they cover every byte of
/// it, whichever strategy it is.
fn legs(object: &Stored, strategy: Strategy) -> Vec<Leg> {
    match strategy {
        Strategy::Whole => vec![Leg {
            case: String::from("the whole object"),
            query: String::new(),
            range: None,
            span: None,
            part_scoped: false,
        }],
        Strategy::Chunked => (0..object.bytes.len())
            .step_by(CHUNK_BYTES)
            .map(|first| {
                let last = (first + CHUNK_BYTES).min(object.bytes.len()) - 1;
                Leg {
                    case: format!("the chunk bytes {first}-{last}"),
                    query: String::new(),
                    range: Some(format!("bytes={first}-{last}")),
                    span: Some((first, last)),
                    part_scoped: false,
                }
            })
            .collect(),
        Strategy::PartScoped => object
            .part_spans()
            .into_iter()
            .enumerate()
            .map(|(index, span)| {
                let number = index + 1;
                Leg {
                    case: format!("part {number}"),
                    query: format!("?partNumber={number}"),
                    range: None,
                    span: Some(span),
                    part_scoped: true,
                }
            })
            .collect(),
    }
}

/// The response headers a HEAD and the GET of the same request must report alike — "the
/// response is identical to the GET response except that there is no response body"
/// (`HeadObject`'s own documentation). Each is an `@httpHeader` of both operations' outputs.
const COMPARED_HEADERS: [&str; 9] = [
    "etag",
    "content-length",
    "content-type",
    "last-modified",
    "accept-ranges",
    "content-range",
    "x-amz-mp-parts-count",
    "x-amz-checksum-sha256",
    "x-amz-checksum-type",
];

/// Write the four objects the download rows are driven against: a small one and a multi-MiB
/// one, each stored under a checksum algorithm and under none, and one of them assembled by a
/// multipart upload.
async fn stored_objects(router: &MultiServiceRouter, name: &str) -> Vec<Stored> {
    let mut objects = Vec::new();

    for (case, key, bytes, checksummed) in [
        (
            "a small object stored by one write",
            "products.csv",
            SMALL_BODY.to_vec(),
            false,
        ),
        (
            "a small object stored by one write under a checksum algorithm",
            "products-sha256.csv",
            SMALL_BODY.to_vec(),
            true,
        ),
        (
            "a multi-MiB object stored by one write",
            "large.bin",
            seeded_payload(0x0bad_f00d, LARGE_BYTES),
            false,
        ),
    ] {
        let headers: Vec<(&str, &str)> = match checksummed {
            true => vec![("x-amz-sdk-checksum-algorithm", ALGORITHM)],
            false => Vec::new(),
        };
        let stored = put_object(router, name, key, &bytes, &headers).await;
        assert_eq!(
            stored.status,
            200,
            "case {case:?}: the object the download rows read must be written first, got {} \
             with body {:?}",
            stored.status,
            text(&stored)
        );
        let whole_checksum = checksummed.then(|| digest_of(&bytes));
        objects.push(Stored {
            case,
            key,
            bytes,
            parts: Vec::new(),
            whole_checksum,
        });
    }

    // The object a transfer manager's UPLOAD leg produces: a multipart upload split at the
    // part size, initiated under a checksum algorithm, so each part carries its own digest and
    // the object carries the composition of them.
    let parts = vec![
        seeded_payload(0x1234_5678, CHUNK_BYTES),
        seeded_payload(0x8765_4321, LARGE_BYTES - CHUNK_BYTES),
    ];
    let completed = multipart_object(
        router,
        name,
        "large-sha256.bin",
        &parts,
        &[("x-amz-checksum-algorithm", ALGORITHM)],
        &[],
    )
    .await;
    assert_eq!(
        completed.status,
        200,
        "the multipart object the download rows read must be completed first, got {} with \
         body {:?}",
        completed.status,
        text(&completed)
    );
    let whole_checksum = completed.element(&format!("Checksum{ALGORITHM}"));
    assert!(
        whole_checksum.is_some(),
        "the completion must report the object's own checksum, got body {:?}",
        text(&completed)
    );
    objects.push(Stored {
        case: "a multi-MiB object assembled by a multipart upload under a checksum algorithm",
        key: "large-sha256.bin",
        bytes: parts.concat(),
        parts,
        whole_checksum,
    });

    objects
}

/// What one leg's HEAD and GET must answer.
fn assert_leg(
    case: &str,
    object: &Stored,
    leg: &Leg,
    mode: Option<&str>,
    head: &Answer,
    got: &Answer,
) {
    let total = object.bytes.len();
    let expected_bytes = match leg.span {
        None => &object.bytes[..],
        Some((first, last)) => &object.bytes[first..=last],
    };
    // A leg that names a span of the object is answered with HTTP's partial-content status;
    // one that asks for the whole of it with 200. Neither is ever a refusal: every binding
    // these legs carry is one the model declares for the operation.
    let expected_status = match leg.span {
        Some(_) => PARTIAL_CONTENT,
        None => 200,
    };
    assert_eq!(
        head.status,
        expected_status,
        "case {case:?}: the HEAD leg must answer HTTP {expected_status}, got {} with body {:?}",
        head.status,
        text(head)
    );
    assert_eq!(
        got.status,
        expected_status,
        "case {case:?}: the GET leg must answer HTTP {expected_status}, got {} with body {:?}",
        got.status,
        text(got)
    );

    // The HEAD carries the metadata and none of the bytes; the GET carries exactly the span's
    // bytes.
    assert!(
        head.body.is_empty(),
        "case {case:?}: a HEAD answer carries no response body, got {} bytes",
        head.body.len()
    );
    assert!(
        got.body.as_ref() == expected_bytes,
        "case {case:?}: expected the {} bytes of the span, got {}",
        expected_bytes.len(),
        got.body.len()
    );

    // Identical to the GET's answer in every header both outputs bind.
    for header in COMPARED_HEADERS {
        assert_eq!(
            head.header(header),
            got.header(header),
            "case {case:?}: the HEAD and the GET of one request must report the same \
             {header:?}, got {:?} and {:?}",
            head.header(header),
            got.header(header)
        );
    }

    // The size a transfer manager reads off the HEAD to drive the fetch: the length of the
    // span a GET of the same request would carry.
    assert_eq!(
        head.header("content-length"),
        Some(expected_bytes.len().to_string()),
        "case {case:?}: expected a Content-Length of {}, got {:?}",
        expected_bytes.len(),
        head.header("content-length")
    );
    // "The portion of the object returned in the response" — stated for a leg that asked for
    // a portion, and for no other.
    assert_eq!(
        head.header("content-range"),
        leg.span.map(|(first, last)| format!("bytes {first}-{last}/{total}")),
        "case {case:?}: expected the Content-Range of the span, got {:?}",
        head.header("content-range")
    );
    assert_eq!(
        head.header("x-amz-mp-parts-count"),
        leg.part_scoped
            .then(|| object.parts_count())
            .flatten()
            .map(|count| count.to_string()),
        "case {case:?}: expected the parts count of a part-scoped read of a multipart \
         object and nothing otherwise, got {:?}",
        head.header("x-amz-mp-parts-count")
    );
    // The checksum is reported to a leg that ASKED for one, of bytes this store holds a digest
    // of — and the mode is honoured however its value is spelled.
    assert_eq!(
        head.header(CHECKSUM_HEADER),
        mode.and_then(|_| object.checksum_of(leg.span)),
        "case {case:?}: expected the stored checksum of the span, got {:?}",
        head.header(CHECKSUM_HEADER)
    );
    // The identity of the object every leg of one download read.
    assert!(
        head.header("etag").is_some_and(|etag| !etag.is_empty()),
        "case {case:?}: expected the object's ETag, got {:?}",
        head.header("etag")
    );
    assert!(
        head.header("last-modified").is_some(),
        "case {case:?}: expected the object's Last-Modified, got {:?}",
        head.header("last-modified")
    );
}

#[tokio::test]
async fn every_leg_of_a_download_of_a_held_object_is_served() {
    let router = router();
    let name = "cli-download-bucket";
    bucket(&router, name).await;

    for object in stored_objects(&router, name).await {
        for strategy in [Strategy::Whole, Strategy::Chunked, Strategy::PartScoped] {
            for mode in MODES {
                for address in [Address::Path, Address::Host] {
                    let mut fetched: Vec<u8> = Vec::new();
                    for leg in legs(&object, strategy) {
                        let case = format!(
                            "{} / {strategy:?} / mode {mode:?} / {address:?} / {}",
                            object.case, leg.case
                        );
                        let mut headers: Vec<(&str, &str)> = Vec::new();
                        if let Some(mode) = mode {
                            headers.push((MODE_HEADER, mode));
                        }
                        if let Some(range) = leg.range.as_deref() {
                            headers.push(("range", range));
                        }
                        let head = dispatch(
                            &router,
                            address,
                            "HEAD",
                            name,
                            object.key,
                            &leg.query,
                            &headers,
                        )
                        .await;
                        let got = dispatch(
                            &router, address, "GET", name, object.key, &leg.query, &headers,
                        )
                        .await;
                        assert_leg(&case, &object, &leg, mode, &head, &got);
                        fetched.extend_from_slice(&got.body);
                    }

                    // The download as a whole: the legs reassemble the stored object.
                    let case = format!(
                        "{} / {strategy:?} / mode {mode:?} / {address:?}",
                        object.case
                    );
                    assert!(
                        fetched == object.bytes,
                        "case {case:?}: the legs must reassemble the stored object's {} \
                         bytes, got {}",
                        object.bytes.len(),
                        fetched.len()
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_download_leg_of_an_address_holding_nothing_is_not_found() {
    let router = router();
    let name = "cli-download-absence-bucket";
    bucket(&router, name).await;
    let stored = put_object(&router, name, "products.csv", SMALL_BODY, &[]).await;
    assert_eq!(
        stored.status,
        200,
        "the object the control below reads must be written first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    // The two addresses a download can be pointed at that hold nothing — a key the bucket
    // holds no object at, and a bucket this emulator does not hold — each read by every leg a
    // download is made of, with and without the mode header. The status is the whole answer a
    // HEAD gets, so what it must never be is the 400 that says the request was malformed.
    for (case, bucket_name, key, expected_code) in [
        (
            "a key the bucket holds no object at",
            name,
            "absent.csv",
            "NoSuchKey",
        ),
        (
            "a bucket this emulator does not hold",
            "cli-download-no-such-bucket",
            "products.csv",
            "NoSuchBucket",
        ),
    ] {
        for mode in MODES {
            for (leg, query, range) in [
                ("the whole object", "", None),
                ("a byte-range chunk", "", Some("bytes=0-4")),
                ("one part", "?partNumber=1", None),
            ] {
                let mut headers: Vec<(&str, &str)> = Vec::new();
                if let Some(mode) = mode {
                    headers.push((MODE_HEADER, mode));
                }
                if let Some(range) = range {
                    headers.push(("range", range));
                }
                for method in ["HEAD", "GET"] {
                    let case = format!("{case} / {leg} / mode {mode:?} / {method}");
                    let answer = dispatch(
                        &router,
                        Address::Path,
                        method,
                        bucket_name,
                        key,
                        query,
                        &headers,
                    )
                    .await;
                    assert_eq!(
                        answer.status,
                        404,
                        "case {case:?}: expected HTTP 404 for an address holding nothing, got \
                         {} with body {:?}",
                        answer.status,
                        text(&answer)
                    );
                    assert_eq!(
                        answer.error_code(),
                        expected_code,
                        "case {case:?}: expected {expected_code}, got {:?}",
                        answer.error_code()
                    );
                }
            }
        }
    }

    // And the object that IS held still downloads, so the rows above were refused for the
    // address rather than for the legs they carry.
    for mode in MODES {
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(mode) = mode {
            headers.push((MODE_HEADER, mode));
        }
        let case = format!("the held object / mode {mode:?}");
        let head = dispatch(
            &router,
            Address::Path,
            "HEAD",
            name,
            "products.csv",
            "",
            &headers,
        )
        .await;
        assert_eq!(
            head.status,
            200,
            "case {case:?}: the held object must still be sized, got {} with body {:?}",
            head.status,
            text(&head)
        );
        let got = dispatch(
            &router,
            Address::Path,
            "GET",
            name,
            "products.csv",
            "",
            &headers,
        )
        .await;
        assert!(
            got.body.as_ref() == SMALL_BODY,
            "case {case:?}: the held object must still be fetched byte-exact, got {} bytes",
            got.body.len()
        );
    }
}
