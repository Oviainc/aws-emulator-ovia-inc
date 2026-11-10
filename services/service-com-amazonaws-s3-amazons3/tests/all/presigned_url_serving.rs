// SPDX-License-Identifier: CC0-1.0

//! A CORRECTLY SIGNED presigned URL, fetched inside the window it asks for, is served — and
//! the same URL fetched after that window has closed is refused with the window it was
//! refused on.
//!
//! Every assertion drives a real HTTP request through a router with request authentication
//! ON, as the emulator runs it, so what is pinned is the wire behaviour a browser, a CDN
//! origin fetch or any tool that cannot set headers sees: the status, the bytes, and the
//! elements of S3's error document.
//!
//! The rules under test:
//!
//! 1. WHERE a SigV4 credential travels is a protocol concern, not a per-operation one: a
//!    request whose credential and signature travel in the query string is authenticated
//!    exactly as one carrying an `Authorization` header, so a presigned URL inside its
//!    validity window is served the answer the operation it addresses would give — HTTP 200
//!    and the object's exact bytes for a read of a stored key.
//! 2. A presigned request whose validity window has PASSED is refused 403 `AccessDenied`
//!    with the sentence AWS words that one condition in, and the document states the window
//!    it was refused on: `X-Amz-Expires` (the seconds the request itself asked for),
//!    `Expires` (the signing instant plus those seconds) and `ServerTime` (the clock the
//!    refusal was taken at, at or after `Expires`). Without them a caller cannot tell an
//!    expired URL from any other `AccessDenied`, nor whether its clock or its window is what
//!    to correct.
//!
//! The rows differ along what the rules generalise over: the method the URL was signed for
//! (a read, a write, a read of the object's metadata), the FORM of the key (a plain name,
//! `/` separators, a space, non-ASCII), the extra query the URL carries beside the signed
//! fields, the window asked for, and how far past that window the fetch is.

use std::time::{Duration, SystemTime};

use super::bucket_lifecycle::Answer;
use super::signature_refusal_wording::{Emulator, REGIONAL_ENDPOINT_HOST};

/// The endpoint every URL here is signed for and fetched at unless the row names another, and
/// so the `host` its signature covers.
const HOST: &str = REGIONAL_ENDPOINT_HOST;

/// That endpoint as a URL, which is what a presigner is handed.
const ENDPOINT: &str = "https://s3.us-east-1.amazonaws.com";

/// The Region the credential scope names, and the signing name S3 verifies under.
const REGION: &str = "us-east-1";
const SIGNING_NAME: &str = "s3";

/// The code AWS refuses a presigned request whose window has passed with, and the sentence
/// it carries. Spelled out here rather than imported, so a wording regression fails this
/// suite.
const EXPIRED_CODE: &str = "AccessDenied";
/// See [`EXPIRED_CODE`].
const EXPIRED_MESSAGE: &str = "Request has expired";

/// The presigned URL for `method` on `path` (decoded, as the model lays it out), carrying
/// `query` beside the signed fields, signed at `signed_at` and valid for `expires`.
///
/// Signed with the key pair the `emulator` published, through the emulator's OWN presigner —
/// the one every handler hands a URL out through — so what this suite drives is a URL a
/// client's signer and this emulator both compute the same signature for, rather than a
/// signature written by hand.
fn presigned(
    emulator: &Emulator,
    method: &str,
    path: &str,
    query: &[(&str, &str)],
    signed_at: SystemTime,
    expires: Duration,
) -> String {
    presigned_at(emulator, ENDPOINT, method, path, query, signed_at, expires)
}

/// [`presigned`] for a URL signed for `endpoint` rather than for the Regional one — the
/// address form is part of what the signature covers, since the `host` is signed.
fn presigned_at(
    emulator: &Emulator,
    endpoint: &str,
    method: &str,
    path: &str,
    query: &[(&str, &str)],
    signed_at: SystemTime,
    expires: Duration,
) -> String {
    let target = emulator_server::request_authentication::PresignTarget {
        endpoint,
        method,
        signing_name: SIGNING_NAME,
        region: REGION,
        path,
        query,
        headers: &[],
    };
    emulator_server::request_authentication::presign_url(
        &target,
        &emulator.access_key_id,
        &emulator.secret_access_key,
        signed_at,
        expires.as_secs(),
    )
    .expect("the target must be signable")
}

/// Fetch `url` the way a client fetches a presigned URL: no `Authorization` header at all,
/// and nothing but the `host` it was signed for.
async fn fetch(emulator: &Emulator, method: &str, url: &str, body: &[u8]) -> Answer {
    fetch_at(emulator, HOST, method, url, body).await
}

/// [`fetch`] of a URL signed for `host`, which is the `host` header it carries.
async fn fetch_at(
    emulator: &Emulator,
    host: &str,
    method: &str,
    url: &str,
    body: &[u8],
) -> Answer {
    emulator.fetch(host, method, url, &[], body).await
}

/// Stage one fixture call, unsigned: the bucket and the objects a presigned read resolves.
///
/// The credential scope is what routes the request and names the Region; no signature is
/// computed, because the harness has authentication off while a fixture is staged.
async fn staged(emulator: &Emulator, method: &str, uri: &str, body: &[u8]) -> Answer {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
         SignedHeaders=host, Signature=not-verified",
        emulator.access_key_id
    );
    emulator
        .send_body(
            method,
            uri,
            &[("authorization".to_string(), authorization)],
            body,
        )
        .await
}

/// `length` bytes whose value depends on the position, so a served body that is not the
/// stored one cannot pass by accident.
fn payload(seed: u8, length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| (index as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

/// The bucket every row of this suite addresses.
const BUCKET: &str = "presigned-serving";

/// RULE 1: a correctly signed presigned URL fetched inside its window serves the object.
#[tokio::test]
async fn a_correctly_signed_presigned_url_is_served() {
    let mut emulator = Emulator::new();
    emulator.enforce_signatures(false);
    let created = staged(&emulator, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );

    // The rows differ along the FORM of the key: a plain name, `/` separators, a space, and
    // non-ASCII UTF-8 — every one of which a presigner encodes and the signature covers.
    let keys: [(&str, &str); 7] = [
        ("a plain name", "object.bin"),
        ("a key with / separators", "deep/dir/nested.bin"),
        ("a key with a space", "model data.tar.gz"),
        ("a key with non-ASCII characters", "données/café.txt"),
        ("a key with a plus", "a+b.txt"),
        ("a key with parentheses", "test_file(3).png"),
        ("a key with query punctuation", "a=b&c.txt"),
    ];
    let mut stored: Vec<(&str, &str, Vec<u8>)> = Vec::new();
    for (index, (case, key)) in keys.into_iter().enumerate() {
        let body = payload(index as u8, 512 + index);
        let written = staged(
            &emulator,
            "PUT",
            &format!(
                "/{BUCKET}/{}",
                ::percent_encoding::utf8_percent_encode(key, PATH_ENCODE_SET)
            ),
            &body,
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the object must be stored first, got {} with body {:?}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        stored.push((case, key, body));
    }

    emulator.enforce_signatures(true);
    let now = SystemTime::now();
    for (case, key, body) in &stored {
        let url = presigned(&emulator, 
            "GET",
            &format!("/{BUCKET}/{key}"),
            &[],
            now,
            Duration::from_secs(900),
        );
        let served = fetch(&emulator, "GET", &url, b"").await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: a presigned GET inside its window must serve the object, got {} \
             with body {:?}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
        assert_eq!(
            served.body.as_ref(),
            body.as_slice(),
            "case {case:?}: a presigned GET must serve the stored bytes, got {} bytes",
            served.body.len()
        );
    }

    // The same rule over the other things a presigned URL varies in: the METHOD it was
    // signed for, the extra query it carries beside the signed fields, and the window it
    // asks for.
    let (_, first_key, first_body) = &stored[0];
    let head = presigned(&emulator, 
        "HEAD",
        &format!("/{BUCKET}/{first_key}"),
        &[],
        now,
        Duration::from_secs(60),
    );
    let answered = fetch(&emulator, "HEAD", &head, b"").await;
    assert_eq!(
        answered.status, 200,
        "case \"a presigned HEAD\": expected HTTP 200, got {}",
        answered.status
    );
    assert_eq!(
        answered.header("content-length"),
        Some(first_body.len().to_string()),
        "case \"a presigned HEAD\": expected the object's size, got {:?}",
        answered.header("content-length")
    );

    let overridden = presigned(&emulator, 
        "GET",
        &format!("/{BUCKET}/{first_key}"),
        &[("response-content-type", "application/octet-stream")],
        now,
        Duration::from_secs(604_800),
    );
    let answered = fetch(&emulator, "GET", &overridden, b"").await;
    assert_eq!(
        answered.status, 200,
        "case \"a presigned GET carrying a response override\": expected HTTP 200, got {} with \
         body {:?}",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
    assert_eq!(
        answered.body.as_ref(),
        first_body.as_slice(),
        "case \"a presigned GET carrying a response override\": expected the stored bytes, got \
         {} bytes",
        answered.body.len()
    );

    let put = presigned(&emulator, 
        "PUT",
        &format!("/{BUCKET}/uploaded.bin"),
        &[],
        now,
        Duration::from_secs(300),
    );
    let uploaded = payload(7, 97);
    let answered = fetch(&emulator, "PUT", &put, &uploaded).await;
    assert_eq!(
        answered.status, 200,
        "case \"a presigned PUT\": expected HTTP 200, got {} with body {:?}",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
    let read_back = presigned(&emulator, 
        "GET",
        &format!("/{BUCKET}/uploaded.bin"),
        &[],
        now,
        Duration::from_secs(300),
    );
    let served = fetch(&emulator, "GET", &read_back, b"").await;
    assert_eq!(
        served.body.as_ref(),
        uploaded.as_slice(),
        "case \"a presigned PUT\": the bytes it wrote must read back, got {} bytes with status \
         {}",
        served.body.len(),
        served.status
    );

    // The boundary of the rule: the presigned fetch resolves the OPERATION the address names,
    // so a key the bucket does not hold is absent rather than unauthorized.
    let absent = presigned(&emulator, 
        "GET",
        &format!("/{BUCKET}/absent.bin"),
        &[],
        now,
        Duration::from_secs(300),
    );
    let refused = fetch(&emulator, "GET", &absent, b"").await;
    assert_eq!(
        refused.error_code(),
        "NoSuchKey",
        "case \"an absent key\": expected NoSuchKey, got {:?} with status {}",
        refused.error_code(),
        refused.status
    );
}

/// RULE 1, over the ADDRESS the URL is signed for: a presigned URL is served whichever
/// endpoint form it names, because where the bucket travels is not part of authentication —
/// the `host` the signature covers is the one the fetch carries.
#[tokio::test]
async fn a_presigned_url_is_served_at_every_address_form() {
    let mut emulator = Emulator::new();
    emulator.enforce_signatures(false);
    let created = staged(&emulator, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let body = payload(11, 321);
    let written = staged(
        &emulator,
        "PUT",
        &format!("/{BUCKET}/addressed.bin"),
        &body,
    )
    .await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} with body {:?}",
        written.status,
        String::from_utf8_lossy(&written.body)
    );
    emulator.enforce_signatures(true);

    // (case, the endpoint the URL is signed for, the host it is fetched at, the path)
    let rows: [(&str, &str, &str, String); 5] = [
        (
            "the Regional endpoint, path-style",
            ENDPOINT,
            HOST,
            format!("/{BUCKET}/addressed.bin"),
        ),
        (
            "the Regional endpoint, virtual-hosted-style",
            &format!("https://{BUCKET}.{HOST}"),
            &format!("{BUCKET}.{HOST}"),
            "/addressed.bin".to_string(),
        ),
        (
            "the global endpoint, virtual-hosted-style",
            &format!("https://{BUCKET}.s3.amazonaws.com"),
            &format!("{BUCKET}.s3.amazonaws.com"),
            "/addressed.bin".to_string(),
        ),
        (
            // The address an emulator is really reached at: a local endpoint on a port,
            // which the signature covers host and port together.
            "a local endpoint on a port, path-style",
            "https://localhost:9297",
            "localhost:9297",
            format!("/{BUCKET}/addressed.bin"),
        ),
        (
            "a local endpoint on a port, virtual-hosted-style",
            &format!("https://{BUCKET}.localhost:9297"),
            &format!("{BUCKET}.localhost:9297"),
            "/addressed.bin".to_string(),
        ),
    ];

    let now = SystemTime::now();
    for (case, endpoint, host, path) in rows {
        let url = presigned_at(&emulator, 
            endpoint,
            "GET",
            &path,
            &[],
            now,
            Duration::from_secs(900),
        );
        let served = fetch_at(&emulator, host, "GET", &url, b"").await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: a presigned GET must serve the object, got {} with body {:?}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
        assert_eq!(
            served.body.as_ref(),
            body.as_slice(),
            "case {case:?}: expected the stored bytes, got {} bytes",
            served.body.len()
        );
    }
}

/// SigV4's `UriEncode` for a path: only the unreserved characters and the `/` separator stay.
const PATH_ENCODE_SET: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'/')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// RULE 2: a presigned request fetched after its window has closed is refused with the window
/// it was refused on.
#[tokio::test]
async fn an_expired_presigned_request_names_the_window_it_was_refused_on() {
    let mut emulator = Emulator::new();
    emulator.enforce_signatures(false);
    let created = staged(&emulator, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {}",
        created.status
    );
    emulator.enforce_signatures(true);

    // The instant the URLs are signed at, on a whole second: a SigV4 stamp spells seconds,
    // so the window every row below is judged against is the one the stamp really names.
    let now = whole_second(SystemTime::now());
    // (case, method, path, the window asked for, how long before now it was signed)
    let rows: [(&str, &str, &str, u64, u64); 5] = [
        ("a read one second past its window", "GET", "/object.txt", 60, 61),
        ("a read an hour past a quarter-hour window", "GET", "/object.txt", 900, 3_600),
        ("a write past its window", "PUT", "/written.bin", 300, 1_200),
        ("a read of a key with / separators", "GET", "/deep/dir/nested.bin", 30, 90),
        ("a read whose window was a single second", "GET", "/object.txt", 1, 5),
    ];

    for (case, method, key, expires, signed_ago) in rows {
        let signed_at = now - Duration::from_secs(signed_ago);
        let url = presigned(&emulator, 
            method,
            &format!("/{BUCKET}{key}"),
            &[],
            signed_at,
            Duration::from_secs(expires),
        );
        let refused = fetch(&emulator, method, &url, b"").await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: expected HTTP 403, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            EXPIRED_CODE,
            "case {case:?}: expected the {EXPIRED_CODE} code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            EXPIRED_MESSAGE,
            "case {case:?}: expected AWS's expiry sentence, got {:?}",
            refused.error_message()
        );
        // The window it was refused on: the seconds the REQUEST asked for, and the instant
        // they ran to — the signing instant plus those seconds, in the form S3's element
        // carries it.
        assert_eq!(
            refused.element("X-Amz-Expires").as_deref(),
            Some(expires.to_string().as_str()),
            "case {case:?}: expected the window the request asked for, got {:?}",
            refused.element("X-Amz-Expires")
        );
        let expires_at = signed_at + Duration::from_secs(expires);
        assert_eq!(
            refused.element("Expires").as_deref(),
            Some(iso8601(expires_at).as_str()),
            "case {case:?}: expected the instant the window ran to, got {:?}",
            refused.element("Expires")
        );
        // And the clock the refusal was taken at, which is at or after that instant: the
        // refusal is the window having closed.
        let server_time = refused
            .element("ServerTime")
            .unwrap_or_else(|| panic!("case {case:?}: the refusal must state the server's time"));
        assert!(
            server_time.as_str() >= iso8601(expires_at).as_str(),
            "case {case:?}: expected a server time at or after {:?}, got {server_time:?}",
            iso8601(expires_at)
        );
        // A write refused for its window writes nothing: the key it addressed is still absent.
        if method == "PUT" {
            emulator.enforce_signatures(false);
            let read = staged(&emulator, "GET", &format!("/{BUCKET}{key}"), b"").await;
            emulator.enforce_signatures(true);
            assert_eq!(
                read.error_code(),
                "NoSuchKey",
                "case {case:?}: a refused write must store nothing, got {:?} with status {}",
                read.error_code(),
                read.status
            );
        }
    }

    // The boundary of the rule: a window that has NOT closed is not an expiry refusal at
    // all — the same URL, signed now, is served.
    let inside = presigned(&emulator, 
        "GET",
        &format!("/{BUCKET}/object.txt"),
        &[],
        now,
        Duration::from_secs(900),
    );
    let answered = fetch(&emulator, "GET", &inside, b"").await;
    assert_ne!(
        answered.error_code(),
        EXPIRED_CODE,
        "case \"a window still open\": a URL inside its window must not be refused for its \
         expiry, got status {} and body {:?}",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
}

/// `instant` with its sub-second part dropped — the precision a SigV4 stamp carries, and so
/// the precision the instants an expiry refusal states are computed at.
fn whole_second(instant: SystemTime) -> SystemTime {
    let since_epoch = instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock must be after the epoch");
    SystemTime::UNIX_EPOCH + Duration::from_secs(since_epoch.as_secs())
}

/// `instant` as `%Y-%m-%dT%H:%M:%SZ`, the form S3's `Expires` and `ServerTime` elements carry
/// — rendered by the library's own calendar rather than by date arithmetic written here.
fn iso8601(instant: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(instant)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant from the clock must render")
}
