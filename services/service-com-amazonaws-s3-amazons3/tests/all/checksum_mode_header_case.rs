// SPDX-License-Identifier: CC0-1.0

//! The CASE a client writes a MODE header's value in, and the read it asks for.
//!
//! The rule under test: **`x-amz-checksum-mode` carries the NAME OF A MODE, which S3 reads
//! case-insensitively — "to retrieve the checksum, this parameter must be enabled"
//! (`ChecksumMode`'s own documentation), and the enum behind it lists the one value
//! `ENABLED`, so a request spelling that value in any case asks for the same thing and is
//! served: the read answers 200 and reports the checksum the object was STORED with, whatever
//! its address form and whichever operation binds the header. Only the CASE is read loosely: a
//! value that is not one of the modes the model lists is still refused `InvalidArgument`
//! (HTTP 400).**
//!
//! That is the form every current AWS client sends. The AWS CLI's `s3 cp` download path (and
//! the SDKs' "validate the response checksum when supported" default behind it) puts the mode
//! on both legs of a download — the `HeadObject` that sizes the object and the `GetObject`
//! that fetches it — so a service that matched the value verbatim would refuse the single most
//! common S3 read with a bare 400 while answering the identical read from a client that
//! happened to upper-case it.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour — status, error code, the `x-amz-checksum-*` response headers — rather than
//! any handler's internals.
//!
//! The rows differ along what the rule generalises over: the case the value is written in, the
//! operation that binds the header (`HeadObject` and `GetObject`, the two the download path
//! uses; the fold is driven off the operation's own input schema, so
//! `GetObjectAnnotation`, the third input in this service binding it, is covered by the same
//! rule), the ADDRESS form the request arrives in (path-style and virtual-hosted-style, the
//! two deserialization paths this service owns), and whether the object was stored under a
//! checksum algorithm at all.

use aws_smithy_types::base64;
use sha2::{Digest, Sha256};

use super::bucket_lifecycle::{router, send_bytes, send_bytes_to_host, Answer};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};

/// The header whose value is read as a mode name, and the one mode the model lists for it.
///
/// Source: `smithy_query(query="shape",shape="com.amazonaws.s3#ChecksumMode")` reports
/// `values: ["ENABLED"]`, bound as `@httpHeader("x-amz-checksum-mode")` on
/// `HeadObjectRequest$ChecksumMode`, `GetObjectRequest$ChecksumMode` and
/// `GetObjectAnnotationRequest$ChecksumMode`.
const MODE_HEADER: &str = "x-amz-checksum-mode";

/// The `ChecksumAlgorithm` the checksummed object below is written under, and the response
/// header the value it recorded travels on.
const ALGORITHM: &str = "SHA256";
/// See [`ALGORITHM`].
const ALGORITHM_HEADER: &str = "x-amz-checksum-sha256";

/// The bytes both objects in this suite are written with.
const BODY: &[u8] = b"id,name\n1,first\n2,second\n";

/// The key of the object stored UNDER a checksum algorithm, and of the one stored without.
const CHECKSUMMED: &str = "products.csv";
const PLAIN: &str = "plain.csv";

/// The Base64 SHA-256 digest of `data`, as a `Checksum*` member carries it.
fn digest_of(data: &[u8]) -> String {
    base64::encode(Sha256::digest(data))
}

/// How a row ADDRESSES the object: the two forms this service resolves a bucket from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Address {
    /// `/<bucket>/<key>` against the Regional endpoint.
    Path,
    /// `/<key>` against `<bucket>.<endpoint>` — what an SDK or CLI sends for an endpoint
    /// addressed by bucket name.
    Host,
}

/// Dispatch `method` at `key` of `bucket`, addressed as `address` says, carrying `headers`.
async fn read(
    router: &smithy_server_runtime::MultiServiceRouter,
    address: Address,
    method: &str,
    bucket: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> Answer {
    match address {
        Address::Path => {
            send_bytes(router, method, &format!("/{bucket}/{key}"), headers, b"").await
        }
        Address::Host => {
            send_bytes_to_host(
                router,
                method,
                &format!("{bucket}.s3.us-east-1.amazonaws.com"),
                &format!("/{key}"),
                headers,
                b"",
            )
            .await
        }
    }
}

/// The spellings of the one mode the model lists — the modelled one, the lower-case one every
/// current AWS client sends, and two mixed ones — each of which must be served.
const SPELLINGS: [&str; 4] = ["ENABLED", "enabled", "Enabled", "eNaBlEd"];

#[tokio::test]
async fn a_mode_header_is_read_whatever_case_its_value_is_written_in() {
    let router = router();
    let name = "checksum-mode-bucket";
    bucket(&router, name).await;

    for (key, headers) in [
        (
            CHECKSUMMED,
            vec![("x-amz-sdk-checksum-algorithm", ALGORITHM)],
        ),
        (PLAIN, Vec::new()),
    ] {
        let stored = put_object(&router, name, key, BODY, &headers).await;
        assert_eq!(
            stored.status,
            200,
            "the write of {key:?} must succeed first, got {} with body {:?}",
            stored.status,
            text(&stored)
        );
    }
    let expected = digest_of(BODY);

    for spelling in SPELLINGS {
        for address in [Address::Path, Address::Host] {
            for method in ["HEAD", "GET"] {
                // The object stored UNDER a checksum algorithm: the read is served and it
                // reports the value the write recorded, which is what says the mode was
                // honoured rather than merely tolerated.
                let case = format!("{spelling:?} {method} {address:?} on a checksummed object");
                let answer = read(
                    &router,
                    address,
                    method,
                    name,
                    CHECKSUMMED,
                    &[(MODE_HEADER, spelling)],
                )
                .await;
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?}: expected HTTP 200, got {} with body {:?}",
                    answer.status,
                    text(&answer)
                );
                assert_eq!(
                    answer.header(ALGORITHM_HEADER),
                    Some(expected.clone()),
                    "case {case:?}: expected the stored checksum {expected:?}, got {:?}",
                    answer.header(ALGORITHM_HEADER)
                );

                // The object stored under NO checksum algorithm: the same read is served and
                // reports no checksum, because there is none to report.
                let case = format!("{spelling:?} {method} {address:?} on a plain object");
                let answer =
                    read(&router, address, method, name, PLAIN, &[(MODE_HEADER, spelling)]).await;
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?}: expected HTTP 200, got {} with body {:?}",
                    answer.status,
                    text(&answer)
                );
                assert_eq!(
                    answer.header(ALGORITHM_HEADER),
                    None,
                    "case {case:?}: an object stored under no checksum algorithm reports \
                     none, got {:?}",
                    answer.header(ALGORITHM_HEADER)
                );
            }
        }
    }
}

#[tokio::test]
async fn a_value_that_is_not_a_mode_the_model_lists_is_still_refused() {
    let router = router();
    let name = "checksum-mode-refusal-bucket";
    bucket(&router, name).await;
    let stored = put_object(
        &router,
        name,
        CHECKSUMMED,
        BODY,
        &[("x-amz-sdk-checksum-algorithm", ALGORITHM)],
    )
    .await;
    assert_eq!(
        stored.status,
        200,
        "the object the rows below read must be written first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    // Values that are NOT the mode the model lists, in the forms a client gets wrong: a mode
    // this parameter has no such value for, a word that is no mode at all, the repeated value
    // a client that set the header twice sends, a value carrying the mode as a substring, and
    // an empty one.
    for value in ["DISABLED", "disabled", "yes", "ENABLED,ENABLED", "ENABLED!", ""] {
        for address in [Address::Path, Address::Host] {
            for method in ["HEAD", "GET"] {
                let case = format!("{value:?} {method} {address:?}");
                let answer = read(
                    &router,
                    address,
                    method,
                    name,
                    CHECKSUMMED,
                    &[(MODE_HEADER, value)],
                )
                .await;
                assert_eq!(
                    answer.status,
                    400,
                    "case {case:?}: a value that is not a mode the model lists must be \
                     refused with HTTP 400, got {} with body {:?}",
                    answer.status,
                    text(&answer)
                );
                if method == "GET" {
                    assert_eq!(
                        answer.error_code(),
                        "InvalidArgument",
                        "case {case:?}: expected the InvalidArgument code, got {:?}",
                        answer.error_code()
                    );
                }
            }
        }
    }

    // And the object is still readable, so the refusals above are of the VALUE rather than of
    // the read.
    let served = read(
        &router,
        Address::Path,
        "GET",
        name,
        CHECKSUMMED,
        &[(MODE_HEADER, "enabled")],
    )
    .await;
    assert_eq!(
        served.status, 200,
        "the object must still read back, got {} with body {:?}",
        served.status,
        text(&served)
    );
}
