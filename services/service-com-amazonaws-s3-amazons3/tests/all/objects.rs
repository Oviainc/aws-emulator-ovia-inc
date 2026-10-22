// SPDX-License-Identifier: CC0-1.0

//! Object keys whose characters a client must percent-encode in the request URI.
//!
//! The object plane's behaviour — what a write stores, what a read answers, what either
//! refuses, and the store's ceilings — is pinned by `all/object_write_and_read_back.rs`,
//! `all/object_and_tagging_roundtrip.rs` and `all/head_object_read_back.rs` beside this
//! suite, over keys that need no encoding. This suite covers the one axis those do not:
//! `PutObject` and the two readers bind their key as a GREEDY `@httpLabel`
//! (`/{Bucket}/{Key+}`), so a key carrying a space, a `+`, a `%`, a `#`, a `?` or
//! non-ASCII text reaches the emulator percent-encoded, and the object must be
//! addressable afterwards by exactly the key the write named.
//!
//! That is the form a cross-service caller uses: an Amazon Transcribe
//! `Media.MediaFileUri` (`s3://<bucket>/<key>`) routinely names media whose key has a
//! space or a `+` in it, and the eight Transcribe checks that stage media through
//! `PutObject` are the reason this crate's object plane exists.

use super::bucket_lifecycle::{create_bucket, router, send_bytes};

/// One key to round-trip: the case, the key as the request URI spells it (already
/// percent-encoded, as a client sends it), and the bytes stored under it.
///
/// The rows differ along the characters that force the encoding: a space, a `+` (which a
/// query parser would read as a space), a literal `%`, a `#` and a `?` (which would
/// otherwise start a fragment or a query), and non-ASCII text.
const KEYS: &[(&str, &str, &[u8])] = &[
    ("a space in the key", "media/my%20sample.wav", b"RIFF----WAVE"),
    ("a plus in the key", "media/my%2Bsample.wav", b"RIFF++++WAVE"),
    ("a literal percent in the key", "media/100%25done.mp3", b"ID3 100%"),
    ("a hash in the key", "media/take%231.flac", b"fLaC take 1"),
    ("a question mark in the key", "media/what%3F.mp3", b"ID3 what?"),
    (
        "non-ascii text in the key",
        "media/%E6%97%A5%E6%9C%AC%E8%AA%9E.mp3",
        b"ID3 nihongo",
    ),
    (
        "several encoded characters under nested prefixes",
        "a%20b/c%2Bd/e%25f/sample%20file.wav",
        b"RIFF nested WAVE",
    ),
];

#[tokio::test]
async fn an_object_under_a_percent_encoded_key_round_trips() {
    let router = router();
    create_bucket(&router, "encoded-keys", None, None).await;

    for (case, key, body) in KEYS {
        let uri = format!("/encoded-keys/{key}");
        let written = send_bytes(
            &router,
            "PUT",
            &uri,
            &[("content-type", "application/octet-stream")],
            body,
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: writing {key} must answer 200, got {} with body {:?}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        let e_tag = written.header("etag").unwrap_or_else(|| {
            panic!(
                "case {case:?}: writing {key} must report an ETag, got {:?}",
                written.headers
            )
        });

        let read = send_bytes(&router, "GET", &uri, &[], b"").await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the object written at {key} must be readable, got {} with \
             body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.body.as_ref(),
            *body,
            "case {case:?}: the read must answer exactly the stored bytes",
        );
        assert_eq!(
            read.header("etag").as_deref(),
            Some(e_tag.as_str()),
            "case {case:?}: the read must report the ETag the write did, got {:?}",
            read.headers
        );

        let headed = send_bytes(&router, "HEAD", &uri, &[], b"").await;
        assert_eq!(
            headed.status, 200,
            "case {case:?}: the object written at {key} must be reportable, got {}",
            headed.status
        );
        assert_eq!(
            headed.header("etag").as_deref(),
            Some(e_tag.as_str()),
            "case {case:?}: the head must report the same ETag, got {:?}",
            headed.headers
        );
        assert_eq!(
            headed.header("content-length").as_deref(),
            Some(body.len().to_string().as_str()),
            "case {case:?}: the head must report the stored size, got {:?}",
            headed.headers
        );
    }

    // Each row is a DISTINCT key: encoding is not collapsing two of them onto one
    // object, which a decode that dropped characters would do.
    let mut e_tags: Vec<String> = Vec::new();
    for (case, key, _) in KEYS {
        let read = send_bytes(&router, "GET", &format!("/encoded-keys/{key}"), &[], b"").await;
        let body = String::from_utf8_lossy(&read.body).to_string();
        assert!(
            !e_tags.contains(&body),
            "case {case:?}: {key} answered a body another key already answered, so the \
             two keys were stored as one: {body:?}"
        );
        e_tags.push(body);
    }

    // And a key the suite never wrote, in the same encoded shape, is still absent.
    let missing = send_bytes(
        &router,
        "GET",
        "/encoded-keys/media/never%20written.wav",
        &[],
        b"",
    )
    .await;
    assert_eq!(
        missing.status, 404,
        "a key nothing wrote must not be readable, got {} with body {:?}",
        missing.status,
        String::from_utf8_lossy(&missing.body)
    );
}
