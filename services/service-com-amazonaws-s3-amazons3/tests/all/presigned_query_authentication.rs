// SPDX-License-Identifier: CC0-1.0

//! SigV4 QUERY-STRING authentication — what a presigned URL is — on the two things a client
//! reads off the refusal rather than off the body it did not get.
//!
//! Every assertion drives a real HTTP request through a router with request authentication
//! ON, as the emulator runs it, so what is pinned is the wire behaviour the fetcher of a
//! presigned URL sees: the status, the `<Code>`, the `<Message>` and the elements of S3's
//! error document.
//!
//! The rules under test:
//!
//! 1. The validity window a presigned URL asks for must be a non-negative count of seconds
//!    no greater than one week ("the maximum expiration time for the signed URL is one
//!    week", AWS General Reference, *Authenticating requests: using query parameters*). A
//!    URL asking for a window outside that range carries a query string the service cannot
//!    use at all, rather than a caller it declined to authorize, so it is refused HTTP 400
//!    with the code `AuthorizationQueryParametersError` and the fixed sentence AWS
//!    publishes for that end of the range — and that is decided BEFORE anything about the
//!    credential or the signature, so the same refusal answers a URL whose credential was
//!    never issued and one whose signing date has long passed.
//! 2. A refusal whose code is `SignatureDoesNotMatch` carries the SigV4 signing diagnostics
//!    beside its code and message: the access key id the request named, the canonical
//!    request the service built, the string it signed, each of the two also as the bytes it
//!    hashed, and the signature the request itself presented. Every one of them is derived
//!    from the request AS RECEIVED — which is what lets a caller see that a parameter it
//!    altered after signing (`X-Amz-Expires` is the one this suite alters) is covered by
//!    the signature, and tell a tampered URL from a wrong secret key.
//!
//! The rows differ along what the rules generalise over: for the first, the value of the
//! window (both bounds of the range, one past each, a far-over value, one with more digits
//! than a 64-bit count holds), the operation addressed (a read of an object, a write of
//! one, a read of a bucket subresource, a listing of the account's buckets), whether the
//! bucket named exists, and whether the credential was issued or the signing date has
//! passed. For the second, how the credential travelled (a presigned query and an
//! `Authorization` header), the method, the FORM of the key, the shape of the query (a bare
//! subresource flag beside the signed parameters), the window the URL states, the signature
//! value presented, and whether the request carried a payload at all or declared the hash of
//! one.

use super::signature_refusal_wording::{
    amz_date_now, authorization, credential_scope, presigned_query, Emulator,
    SIGNATURE_MISMATCH_MESSAGE, SIGV4_ALGORITHM, WRONG_SIGNATURE,
};

/// The error code S3 answers a SigV4 query-authenticated request whose authentication
/// parameters are themselves unusable.
const QUERY_PARAMETERS_CODE: &str = "AuthorizationQueryParametersError";

/// The code a request whose signature did not verify is refused with.
const SIGNATURE_MISMATCH_CODE: &str = "SignatureDoesNotMatch";

/// The sentence AWS answers a window longer than the one-week ceiling. Fixed text in full,
/// the ceiling included: nothing in it comes from the request.
const OVER_CEILING_MESSAGE: &str =
    "X-Amz-Expires must be less than a week (in seconds); that is, the given X-Amz-Expires \
     must be less than 604800 seconds";

/// The sentence AWS answers a NEGATIVE window — the other end of the same range.
const NEGATIVE_MESSAGE: &str = "X-Amz-Expires must be non-negative";

/// A second signature value that is not the one the emulator computes either, so a row can
/// show that the signature the refusal reports is the one the REQUEST presented rather than
/// a constant.
const OTHER_WRONG_SIGNATURE: &str =
    "1111111111111111111111111111111111111111111111111111111111111111";

/// The access key id of a credential this emulator never issued.
const UNISSUED_ACCESS_KEY_ID: &str = "AKIDNOTISSUEDBYTHISEMULATOR";

/// The host every request in this suite is addressed to, which is therefore the `host`
/// header its signature covers.
const HOST: &str = "s3.us-east-1.amazonaws.com";

/// The marker a presigned S3 URL signs in place of a payload hash.
const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

/// `uri` with `query` appended, whichever separator it needs.
fn with_query(uri: &str, query: &str) -> String {
    let separator = if uri.contains('?') { '&' } else { '?' };
    format!("{uri}{separator}{query}")
}

/// RULE 1: a presigned URL whose `X-Amz-Expires` is outside the accepted range is refused
/// HTTP 400 `AuthorizationQueryParametersError` with AWS's fixed sentence for that end of
/// the range, whatever it addresses and whatever else about it would not have verified.
#[tokio::test]
async fn a_window_outside_the_accepted_range_is_refused_for_the_query_string() {
    let emulator = Emulator::new();
    let now = amz_date_now();
    let issued = emulator.access_key_id.clone();

    // (case, method, address, access key id, signing date, the window asked for, the
    // sentence AWS answers it with)
    let rows: [(&str, &str, &str, &str, &str, &str, &str); 10] = [
        (
            "one second past the one-week ceiling",
            "GET",
            "/range-bucket/object.txt",
            &issued,
            &now,
            "604801",
            OVER_CEILING_MESSAGE,
        ),
        (
            "a week and a day",
            "GET",
            "/range-bucket/object.txt",
            &issued,
            &now,
            "691200",
            OVER_CEILING_MESSAGE,
        ),
        (
            "a year",
            "PUT",
            "/range-bucket/written.txt",
            &issued,
            &now,
            "31536000",
            OVER_CEILING_MESSAGE,
        ),
        (
            "more seconds than a 64-bit count holds",
            "GET",
            "/range-bucket/object.txt",
            &issued,
            &now,
            "99999999999999999999999999",
            OVER_CEILING_MESSAGE,
        ),
        (
            "a bucket subresource with an over-ceiling window",
            "GET",
            "/range-bucket?acl",
            &issued,
            &now,
            "604801",
            OVER_CEILING_MESSAGE,
        ),
        (
            "the service root with an over-ceiling window",
            "GET",
            "/",
            &issued,
            &now,
            "604801",
            OVER_CEILING_MESSAGE,
        ),
        (
            // The refusal is at the boundary, so it cannot depend on the resource: no
            // bucket of this name was ever created.
            "a bucket this emulator does not hold",
            "GET",
            "/absent-bucket/absent.txt",
            &issued,
            &now,
            "604801",
            OVER_CEILING_MESSAGE,
        ),
        (
            // The query string is judged BEFORE the credential, so a key id that resolves
            // to nothing is still answered for the window it asked for.
            "a credential this emulator never issued",
            "GET",
            "/range-bucket/object.txt",
            UNISSUED_ACCESS_KEY_ID,
            &now,
            "604801",
            OVER_CEILING_MESSAGE,
        ),
        (
            // And before the window is compared against the clock, so a URL signed years
            // ago is answered for its range rather than for its expiry.
            "a signing date long past",
            "GET",
            "/range-bucket/object.txt",
            &issued,
            "20200101T000000Z",
            "604801",
            OVER_CEILING_MESSAGE,
        ),
        (
            "a negative window",
            "GET",
            "/range-bucket/object.txt",
            &issued,
            &now,
            "-1",
            NEGATIVE_MESSAGE,
        ),
    ];

    for (case, method, address, key_id, amz_date, expires, expected) in rows {
        let uri = with_query(
            address,
            &presigned_query(key_id, amz_date, expires, WRONG_SIGNATURE),
        );
        let refused = emulator.send(method, &uri, &[]).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            QUERY_PARAMETERS_CODE,
            "case {case:?}: expected the {QUERY_PARAMETERS_CODE} code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected AWS's fixed sentence for that end of the range, got {:?}",
            refused.error_message()
        );
        // A query string the service could not use is not a signature comparison, so it
        // carries none of the signing diagnostics the comparison's own refusal does.
        assert_eq!(
            refused.element("CanonicalRequest"),
            None,
            "case {case:?}: a query-parameter refusal must carry no canonical request, got {:?}",
            refused.element("CanonicalRequest")
        );
    }
}

/// RULE 1, its BOUNDARY: a window INSIDE the accepted range is not a query-string refusal
/// at all — the published ceiling itself is accepted, and what answers such a URL is the
/// condition that really refused it.
#[tokio::test]
async fn a_window_inside_the_accepted_range_is_judged_on_its_own_terms() {
    let emulator = Emulator::new();
    let now = amz_date_now();
    let key_id = emulator.access_key_id.clone();

    // (case, the window asked for, the code the refusal carries)
    let rows: [(&str, &str, &str); 5] = [
        ("one second of validity", "1", SIGNATURE_MISMATCH_CODE),
        ("the quarter hour a signer defaults to", "900", SIGNATURE_MISMATCH_CODE),
        ("one second under the ceiling", "604799", SIGNATURE_MISMATCH_CODE),
        ("the published ceiling itself", "604800", SIGNATURE_MISMATCH_CODE),
        // Zero is inside the range, so it is not a query-string refusal — it is a window
        // that has already closed, which AWS states as the expiry it is.
        ("no validity at all", "0", "AccessDenied"),
    ];

    for (case, expires, expected_code) in rows {
        let uri = with_query(
            "/range-bucket/object.txt",
            &presigned_query(&key_id, &now, expires, WRONG_SIGNATURE),
        );
        let refused = emulator.send("GET", &uri, &[]).await;
        assert_ne!(
            refused.error_code(),
            QUERY_PARAMETERS_CODE,
            "case {case:?}: a window inside the accepted range must not be refused for the \
             query string, got status {} and body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.status, 403,
            "case {case:?}: expected HTTP 403, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            expected_code,
            "case {case:?}: expected the {expected_code} code, got {:?}",
            refused.error_code()
        );
    }
}

/// One refused request as a row names it.
struct Row {
    /// What the row is, for the assertions to name.
    case: &'static str,
    /// The method it was made with, the canonical request's first line.
    method: &'static str,
    /// The path it addressed, the canonical request's second line.
    /// The path it addressed.
    path: &'static str,
    /// The canonical URI of that path, the canonical request's second line: SigV4 encodes it
    /// ONCE over the unreserved characters with `/` left a separator, which is what S3's
    /// `disableDoubleEncoding` selects — so a path carrying an ENCODED separator canonicalises
    /// as the path it names, and every other path here canonicalises as itself.
    canonical_path: String,
    /// The query it carried, signature included.
    query: String,
    /// The headers it carried.
    headers: Vec<(String, String)>,
    /// The bytes it carried.
    body: Vec<u8>,
    /// The signature it presented.
    signature: &'static str,
    /// The canonical query string SigV4 states for it: every pair but the signature, sorted
    /// and encoded. The query above is written in that order already, so this is that query
    /// with its one uncoverable pair removed.
    canonical_query: String,
    /// The payload hash its signature covered.
    payload: String,
}

/// RULE 2: every refusal that reports a signature mismatch carries the signing diagnostics,
/// each derived from the request as received.
#[tokio::test]
async fn a_signature_mismatch_carries_the_diagnostics_of_the_request_as_received() {
    let emulator = Emulator::new();
    let amz_date = amz_date_now();
    let key_id = emulator.access_key_id.clone();

    let presigned = |expires: &str, signature: &'static str| {
        presigned_query(&key_id, &amz_date, expires, signature)
    };
    let signature_free = |query: &str| {
        query
            .split("&X-Amz-Signature=")
            .next()
            .expect("the presigned query must carry a signature pair")
            .to_string()
    };
    let header_signed = |signature: &str| {
        vec![
            (
                "authorization".to_string(),
                authorization(&key_id, &amz_date, signature),
            ),
            ("x-amz-date".to_string(), amz_date.clone()),
        ]
    };
    let written = b"the bytes a signed write carries".to_vec();

    let presigned_rows: [(&str, &str, &str, &str, &'static str); 5] = [
        (
            "a presigned read of an object",
            "GET",
            "/diagnostics-bucket/object.txt",
            "300",
            WRONG_SIGNATURE,
        ),
        (
            // The recorded instance: the signed window widened after signing, every other
            // pair left byte-identical.
            "a presigned read whose X-Amz-Expires was widened after signing",
            "GET",
            "/diagnostics-bucket/object.txt",
            "600",
            WRONG_SIGNATURE,
        ),
        (
            "a presigned write of a key with / separators",
            "PUT",
            "/diagnostics-bucket/deep/dir/nested.bin",
            "900",
            OTHER_WRONG_SIGNATURE,
        ),
        (
            "a presigned read at the one-week ceiling",
            "GET",
            "/diagnostics-bucket/object.txt",
            "604800",
            WRONG_SIGNATURE,
        ),
        (
            // A key whose name carries an ENCODED separator: the canonical URI SigV4 states
            // for it is the path it names, which is the same canonicalisation the emulator
            // compared the signature against.
            "a presigned read of a key carrying an encoded separator",
            "GET",
            "/diagnostics-bucket/a%2Fb.txt",
            "300",
            WRONG_SIGNATURE,
        ),
    ];

    let mut rows: Vec<Row> = presigned_rows
        .into_iter()
        .map(|(case, method, path, expires, signature)| {
            let query = presigned(expires, signature);
            Row {
                case,
                method,
                canonical_path: path.replace("%2F", "/"),
                path,
                canonical_query: signature_free(&query),
                query,
                headers: Vec::new(),
                body: Vec::new(),
                signature,
                payload: UNSIGNED_PAYLOAD.to_string(),
            }
        })
        .collect();

    // A bare subresource flag beside the signed parameters: SigV4 states it as a pair with
    // an empty value, and sorts it after them.
    let subresource = presigned("300", WRONG_SIGNATURE);
    rows.push(Row {
        case: "a presigned read of a bucket subresource",
        method: "GET",
        path: "/diagnostics-bucket",
        canonical_path: "/diagnostics-bucket".to_string(),
        canonical_query: format!("{}&acl=", signature_free(&subresource)),
        query: format!("acl&{subresource}"),
        headers: Vec::new(),
        body: Vec::new(),
        signature: WRONG_SIGNATURE,
        payload: UNSIGNED_PAYLOAD.to_string(),
    });
    // The other form the credential travels in: a header-signed request, whose query is
    // empty and whose signature covers the payload that arrived.
    rows.push(Row {
        case: "a header-signed read of an object",
        method: "GET",
        path: "/diagnostics-bucket/object.txt",
        canonical_path: "/diagnostics-bucket/object.txt".to_string(),
        query: String::new(),
        canonical_query: String::new(),
        headers: header_signed(WRONG_SIGNATURE),
        body: Vec::new(),
        signature: WRONG_SIGNATURE,
        payload: sha256_hex(b""),
    });
    rows.push(Row {
        case: "a header-signed write carrying a payload",
        method: "PUT",
        path: "/diagnostics-bucket/written.bin",
        canonical_path: "/diagnostics-bucket/written.bin".to_string(),
        query: String::new(),
        canonical_query: String::new(),
        headers: header_signed(OTHER_WRONG_SIGNATURE),
        body: written.clone(),
        signature: OTHER_WRONG_SIGNATURE,
        payload: sha256_hex(&written),
    });
    // A request that DECLARES its payload hash signs the value it declared, so that is what
    // the canonical request states: here the marker a client sends when the signature covers
    // no payload at all.
    let mut declared = header_signed(WRONG_SIGNATURE);
    declared.push((
        "x-amz-content-sha256".to_string(),
        UNSIGNED_PAYLOAD.to_string(),
    ));
    rows.push(Row {
        case: "a header-signed write declaring an unsigned payload",
        method: "PUT",
        path: "/diagnostics-bucket/declared.bin",
        canonical_path: "/diagnostics-bucket/declared.bin".to_string(),
        query: String::new(),
        canonical_query: String::new(),
        headers: declared,
        body: written.clone(),
        signature: WRONG_SIGNATURE,
        payload: UNSIGNED_PAYLOAD.to_string(),
    });

    for row in rows {
        let case = row.case;
        let uri = match row.query.is_empty() {
            true => row.path.to_string(),
            false => with_query(row.path, &row.query),
        };
        let refused = emulator
            .send_body(row.method, &uri, &row.headers, &row.body)
            .await;

        assert_eq!(
            refused.status, 403,
            "case {case:?}: expected HTTP 403, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            SIGNATURE_MISMATCH_CODE,
            "case {case:?}: expected the {SIGNATURE_MISMATCH_CODE} code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            SIGNATURE_MISMATCH_MESSAGE,
            "case {case:?}: expected AWS's signature-mismatch sentence, got {:?}",
            refused.error_message()
        );

        // The credential the request named, and the signature it presented — unchanged, so
        // a caller can compare it against the URL it fetched.
        assert_eq!(
            refused.element("AWSAccessKeyId").as_deref(),
            Some(key_id.as_str()),
            "case {case:?}: expected the access key id the request named, got {:?}",
            refused.element("AWSAccessKeyId")
        );
        assert_eq!(
            refused.element("SignatureProvided").as_deref(),
            Some(row.signature),
            "case {case:?}: expected the signature the request presented, got {:?}",
            refused.element("SignatureProvided")
        );

        // The canonical request, line by line: the method, the path, the query as received
        // (the altered window included), the headers the request signed, that same list,
        // and the payload the signature covered.
        let canonical = refused
            .element("CanonicalRequest")
            .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry a canonical request"));
        let lines: Vec<&str> = canonical.split('\n').collect();
        let expected: Vec<String> = vec![
            row.method.to_string(),
            row.canonical_path.clone(),
            row.canonical_query.clone(),
            format!("host:{HOST}"),
            String::new(),
            "host".to_string(),
            row.payload.clone(),
        ];
        assert_eq!(
            lines,
            expected.iter().map(String::as_str).collect::<Vec<&str>>(),
            "case {case:?}: expected the canonical request of the request as received, got \
             {canonical:?}"
        );

        // The string the service signed: the algorithm, the signing instant, the scope the
        // request named with its key id dropped, and the hash of that canonical request.
        let scope = credential_scope(&key_id, &amz_date);
        let scope = scope
            .split_once('/')
            .expect("a credential scope names a key id and a scope")
            .1;
        assert_eq!(
            refused.element("StringToSign").as_deref(),
            Some(
                format!(
                    "{SIGV4_ALGORITHM}\n{amz_date}\n{scope}\n{}",
                    sha256_hex(canonical.as_bytes())
                )
                .as_str()
            ),
            "case {case:?}: expected the string to sign of that canonical request, got {:?}",
            refused.element("StringToSign")
        );

        // And each of the two as the BYTES the service hashed, which is what shows the
        // whitespace a rendered string cannot.
        for (string_element, bytes_element) in [
            ("StringToSign", "StringToSignBytes"),
            ("CanonicalRequest", "CanonicalRequestBytes"),
        ] {
            let text = refused.element(string_element).unwrap_or_default();
            let hex = refused.element(bytes_element).unwrap_or_default();
            let decoded = ::hex::decode(hex.replace(' ', "")).unwrap_or_else(|error| {
                panic!("case {case:?}: {bytes_element} must be hex bytes: {error}")
            });
            assert_eq!(
                decoded,
                text.as_bytes(),
                "case {case:?}: {bytes_element} must be the bytes of {string_element}, got \
                 {hex:?}"
            );
        }
    }
}

/// The hex SHA-256 of `bytes`, computed here by the digest crate rather than restated, so
/// the payload line a row expects is derived from the bytes that row sent.
fn sha256_hex(bytes: &[u8]) -> String {
    use ::sha2::Digest;
    ::hex::encode(::sha2::Sha256::digest(bytes))
}
