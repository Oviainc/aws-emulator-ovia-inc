// SPDX-License-Identifier: CC0-1.0

//! What `encoding-type` does to a listing, end to end.
//!
//! RULE: `EncodingType` is a modelled input member of a listing, bound to the
//! `encoding-type` query parameter, and when a request carries it the response ECHOES it
//! and writes its key-name elements percent-encoded — "If you specify the encoding-type
//! request parameter, Amazon S3 includes this element in the response, and returns encoded
//! key name values in the following response elements: `Delimiter, Prefix, Key,` and
//! `StartAfter`" (`ListObjectsV2Output$EncodingType`), with the rolled-up
//! `CommonPrefixes` encoded with them. The encoding is per UTF-8 code value — "the object
//! `test_file(3).png` will appear as `test_file%283%29.png`" (the `EncodingType` enum's own
//! documentation) — so every character outside the unreserved set `A-Z a-z 0-9 - _ . ~` is
//! written `%XX`, with the two exceptions the form S3 answers in carries: a SPACE is written
//! `+`, and the `/` that separates the elements of a key prefix is left as it is. A listing
//! that asks for no encoding type reports every one of those elements verbatim and echoes no
//! `EncodingType`.
//!
//! `ListObjectVersions` carries the same member and is held to the same rule, over the
//! same keys.
//!
//! The objects are seeded through the SAME store the service reads (the pattern the
//! object-storage suites use for a case a bound's worth of wire calls cannot reach),
//! because the keys this rule is about carry characters an HTTP request line cannot: a
//! space, and non-ASCII UTF-8. Every listing below is driven over the wire, which is what
//! the rule is about. Every expected value is written out rather than computed, so nothing
//! here can agree with a wrong encoder by construction.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use service_com_amazonaws_s3_amazons3::state::{
    ComAmazonawsS3Amazons3State, Object, ObjectEncryption, ObjectWrite, DEFAULT_STORAGE_CLASS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router_with_engine, send};
use super::bucket_tagging::unescaped;
use super::object_and_tagging_roundtrip::text;
use super::object_storage::{list_objects_v2, listing_of};

/// The keys every case below holds, with the form each takes under `encoding-type=url`.
///
/// The rows differ along WHICH characters a key name carries: only unreserved ones (which
/// the encoding leaves alone), the `/` of a prefix (left as it is), the parentheses of the
/// documented example, a space (written `+`), `+` and `=` and `&` (characters a query string
/// would otherwise swallow), and non-ASCII UTF-8, whose two bytes become two escapes.
const KEYS: &[(&str, &str, &str)] = &[
    (
        "only unreserved characters",
        "train.csv-v1_0.final~2",
        "train.csv-v1_0.final~2",
    ),
    ("a prefixed key", "train/train.csv", "train/train.csv"),
    (
        "the documented parentheses example",
        "test_file(3).png",
        "test_file%283%29.png",
    ),
    ("a space", "model data.tar.gz", "model+data.tar.gz"),
    ("query punctuation", "a+b=c&d", "a%2Bb%3Dc%26d"),
    (
        "non-ASCII UTF-8",
        "données/café.txt",
        "donn%C3%A9es/caf%C3%A9.txt",
    ),
];

/// A stored object of `body`, with every member a write that named nothing leaves at its
/// empty value.
fn object(body: &[u8]) -> Object {
    Object {
        etag: format!("\"{}\"", "0".repeat(32)),
        body: body.to_vec(),
        last_modified: SystemTime::now(),
        storage_class: DEFAULT_STORAGE_CLASS.to_string(),
        content_type: None,
        cache_control: None,
        content_disposition: None,
        content_encoding: None,
        content_language: None,
        expires: None,
        website_redirect_location: None,
        metadata: BTreeMap::new(),
        tags: Vec::new(),
        encryption: ObjectEncryption::default(),
        // The store mints an object's version id; one handed to `put_object` carries none.
        version_id: None,
        // A write places no Object Lock retention; `PutObjectRetention` is what does.
        retention: None,
        // Nor a legal hold; `PutObjectLegalHold` is what applies one.
        legal_hold: None,
        // A write initiates no restore either; `RestoreObject` is what does.
        restore: None,
        // And it attaches no annotation; `PutObjectAnnotation` is what does.
        annotations: BTreeMap::new(),
        // Nor an ACL; `PutObjectAcl` is what sets one.
        acl: None,
        // And no checksum: a write that names no checksum algorithm stores none.
        checksum: None,
        // Nor is it assembled from parts: one write stored it whole.
        parts: Vec::new(),
        // And nothing replicated it: the store settles a replication record from the
        // bucket's own configuration.
        replication: None,
    }
}

/// Create `bucket` over the wire and seed it with `keys` through the store.
async fn given_bucket_holding(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    bucket: &str,
    keys: &[&str],
) {
    let created = create_bucket(router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket {bucket:?} must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let state = engine.state::<ComAmazonawsS3Amazons3State>();
    let mut store = state.lock();
    for key in keys {
        assert_eq!(
            store.put_object(bucket, key, object(b"body")),
            Some(ObjectWrite::Created),
            "the key {key:?} must be stored first"
        );
    }
}

/// The stored keys of [`KEYS`] and their encoded forms, in the ascending byte order a
/// listing reports them in.
fn expected_keys() -> (Vec<String>, Vec<String>) {
    let mut rows: Vec<(String, String)> = KEYS
        .iter()
        .map(|(_, key, encoded)| ((*key).to_string(), (*encoded).to_string()))
        .collect();
    rows.sort();
    (
        rows.iter().map(|(key, _)| key.clone()).collect(),
        rows.iter().map(|(_, encoded)| encoded.clone()).collect(),
    )
}

#[tokio::test]
async fn the_encoding_type_a_listing_asks_for_is_what_its_key_names_carry() {
    let (router, engine) = router_with_engine();
    let keys: Vec<&str> = KEYS.iter().map(|(_, key, _)| *key).collect();
    given_bucket_holding(&router, &engine, "encoded-listing", &keys).await;
    let (verbatim, encoded_keys) = expected_keys();

    // Without an encoding type: every key verbatim, and no `EncodingType` echoed.
    let plain = list_objects_v2(&router, "encoded-listing", "").await;
    assert_eq!(
        plain.status, 200,
        "case \"no encoding type\": a listing of a bucket that exists is 200, got {} with \
         body {:?}",
        plain.status,
        text(&plain)
    );
    let plain = listing_of(&plain);
    assert_eq!(
        plain.keys(),
        verbatim,
        "case \"no encoding type\": every key must be reported verbatim, got {:?}",
        plain.keys()
    );
    assert_eq!(
        plain.field("EncodingType"),
        "",
        "case \"no encoding type\": nothing may be echoed, got {:?}",
        plain.field("EncodingType")
    );

    // With `encoding-type=url`: the SAME entries, each key percent-encoded, and the
    // parameter echoed.
    let encoded = list_objects_v2(&router, "encoded-listing", "&encoding-type=url").await;
    assert_eq!(
        encoded.status, 200,
        "case \"encoding-type=url\": a listing that asks for an encoding type is 200, not \
         a refusal, got {} with body {:?}",
        encoded.status,
        text(&encoded)
    );
    let encoded = listing_of(&encoded);
    assert_eq!(
        encoded.keys(),
        encoded_keys,
        "case \"encoding-type=url\": every Key must be percent-encoded per UTF-8 code \
         value, got {:?}",
        encoded.keys()
    );
    assert_eq!(
        encoded.field("EncodingType"),
        "url",
        "case \"encoding-type=url\": the request's encoding type must be echoed, got {:?}",
        encoded.field("EncodingType")
    );
    assert_eq!(
        encoded.field("KeyCount"),
        KEYS.len().to_string(),
        "case \"encoding-type=url\": the encoding changes the rendering, not the page, got \
         {:?}",
        encoded.field("KeyCount")
    );
}

#[tokio::test]
async fn the_other_key_name_elements_of_a_listing_are_encoded_too() {
    let (router, engine) = router_with_engine();
    given_bucket_holding(
        &router,
        &engine,
        "encoded-elements",
        &[
            "données/café.txt",
            "données/model data.tar.gz",
            "test_file(3).png",
        ],
    )
    .await;

    // `Prefix`, `Delimiter` and `StartAfter` are key names, so each comes back encoded —
    // and the rolled-up `CommonPrefixes` with them. The filtering itself still happens on
    // the key as stored, which is what makes the prefix match at all.
    let answer = list_objects_v2(
        &router,
        "encoded-elements",
        "&encoding-type=url&prefix=donn%C3%A9es/&delimiter=%20",
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "a listing with a prefix and a delimiter is 200, got {} with body {:?}",
        answer.status,
        text(&answer)
    );
    let listing = listing_of(&answer);
    assert_eq!(
        listing.field("Prefix"),
        "donn%C3%A9es/",
        "case \"prefix\": the echoed Prefix must be percent-encoded, got {:?}",
        listing.field("Prefix")
    );
    assert_eq!(
        listing.field("Delimiter"),
        "+",
        "case \"delimiter\": the echoed Delimiter must be encoded, got {:?}",
        listing.field("Delimiter")
    );
    assert_eq!(
        listing.keys(),
        vec!["donn%C3%A9es/caf%C3%A9.txt".to_string()],
        "case \"prefix\": the key that does not roll up is reported encoded, got {:?}",
        listing.keys()
    );
    assert_eq!(
        listing.common_prefixes,
        vec!["donn%C3%A9es/model+".to_string()],
        "case \"delimiter\": a rolled-up common prefix is a key name, so it is encoded \
         too, got {:?}",
        listing.common_prefixes
    );

    // `StartAfter` is echoed encoded, and still excludes the key it names.
    let after = list_objects_v2(
        &router,
        "encoded-elements",
        "&encoding-type=url&start-after=donn%C3%A9es/caf%C3%A9.txt",
    )
    .await;
    let after = listing_of(&after);
    assert_eq!(
        after.field("StartAfter"),
        "donn%C3%A9es/caf%C3%A9.txt",
        "case \"start-after\": the echoed StartAfter must be percent-encoded, got {:?}",
        after.field("StartAfter")
    );
    assert_eq!(
        after.keys(),
        vec![
            "donn%C3%A9es/model+data.tar.gz".to_string(),
            "test_file%283%29.png".to_string(),
        ],
        "case \"start-after\": the listing still starts after the key it names, got {:?}",
        after.keys()
    );

    // And the same request WITHOUT the encoding type answers the same entries verbatim,
    // so what changed is the rendering and not the listing.
    let plain = list_objects_v2(
        &router,
        "encoded-elements",
        "&prefix=donn%C3%A9es/&delimiter=%20",
    )
    .await;
    let plain = listing_of(&plain);
    assert_eq!(
        plain.field("Prefix"),
        "données/",
        "case \"no encoding type\": the echoed Prefix must be verbatim, got {:?}",
        plain.field("Prefix")
    );
    assert_eq!(
        plain.keys(),
        vec!["données/café.txt".to_string()],
        "case \"no encoding type\": the same entries, verbatim, got {:?}",
        plain.keys()
    );
    assert_eq!(
        plain.common_prefixes,
        vec!["données/model ".to_string()],
        "case \"no encoding type\": the same common prefix, verbatim, got {:?}",
        plain.common_prefixes
    );
}

#[tokio::test]
async fn a_version_listing_honours_the_encoding_type_the_same_way() {
    let (router, engine) = router_with_engine();
    let keys: Vec<&str> = KEYS.iter().map(|(_, key, _)| *key).collect();
    given_bucket_holding(&router, &engine, "encoded-versions", &keys).await;

    let encoded = send(
        &router,
        "GET",
        "/encoded-versions?versions&encoding-type=url",
        &[],
        "",
    )
    .await;
    assert_eq!(
        encoded.status, 200,
        "case \"encoding-type=url\": a version listing that asks for an encoding type is \
         200, not a refusal, got {} with body {:?}",
        encoded.status,
        text(&encoded)
    );
    // Unescaped, because the serializer escapes an `&` in a key name: what a client's XML
    // reader sees is the text, not the escape.
    let body = unescaped(&String::from_utf8_lossy(&encoded.body));
    assert!(
        body.contains("<EncodingType>url</EncodingType>"),
        "case \"encoding-type=url\": the version listing must echo the encoding type, got \
         {body}"
    );
    for (case, key, encoded_key) in KEYS {
        assert!(
            body.contains(&format!("<Key>{encoded_key}</Key>")),
            "case {case:?}: the version listing must report {key:?} as {encoded_key:?}, \
             got {body}"
        );
    }

    // Without the encoding type, the same listing reports the same keys verbatim.
    let verbatim = send(&router, "GET", "/encoded-versions?versions", &[], "").await;
    let body = unescaped(&String::from_utf8_lossy(&verbatim.body));
    assert!(
        !body.contains("<EncodingType>"),
        "case \"no encoding type\": nothing may be echoed, got {body}"
    );
    for (case, key, _) in KEYS {
        assert!(
            body.contains(&format!("<Key>{key}</Key>")),
            "case {case:?}: the version listing must report {key:?} verbatim, got {body}"
        );
    }
}
