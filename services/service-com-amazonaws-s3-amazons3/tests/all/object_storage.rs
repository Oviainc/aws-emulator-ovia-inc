// SPDX-License-Identifier: CC0-1.0

//! The harness this crate's object-storage suites share: the requests that read and
//! remove objects, and an XML reader for a `ListObjectsV2` body.
//!
//! One module rather than a copy per suite, beside the request harness
//! [`super::bucket_lifecycle`] owns (`router`, `send`, `send_bytes`, `Answer`) and the
//! object helpers [`super::object_and_tagging_roundtrip`] owns (`put_object`,
//! `get_object`, `bucket`, `expected_etag`, `text`). The tests that use what is here live
//! in [`super::object_listing_and_deletion`]; nothing in this file asserts anything, so a
//! suite is free to assert its own rule over these.

use std::collections::BTreeMap;

use quick_xml::events::Event;
use quick_xml::Reader;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{send, send_bytes, Answer};
use super::bucket_tagging::unescaped;
use super::object_and_tagging_roundtrip::{put_object, text};

/// `GET /<bucket>?list-type=2…` — `ListObjectsV2`, with whatever extra query parameters
/// the case under test carries.
pub(crate) async fn list_objects_v2(
    router: &MultiServiceRouter,
    bucket: &str,
    query: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{bucket}?list-type=2{query}"),
        &[],
        "",
    )
    .await
}

/// `DELETE /<bucket>/<key>` — `DeleteObject`.
pub(crate) async fn delete_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
) -> Answer {
    send_bytes(router, "DELETE", &format!("/{bucket}/{key}"), &[], b"").await
}

/// `DELETE /<bucket>` — `DeleteBucket`.
pub(crate) async fn delete_bucket(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "DELETE", &format!("/{bucket}"), &[], "").await
}

/// One `ListObjectsV2` response, as a client's XML reader sees it: the top-level
/// elements, one map per `Contents` entry and the `CommonPrefixes` prefixes, each in
/// document order.
#[derive(Debug, Default)]
pub(crate) struct Listing {
    /// The elements directly under the result: `Name`, `KeyCount`, `IsTruncated`,
    /// `MaxKeys`, `NextContinuationToken` and the echoed request parameters.
    top: BTreeMap<String, String>,
    /// One map per `Contents` entry, keyed by element name.
    pub(crate) contents: Vec<BTreeMap<String, String>>,
    /// Every `CommonPrefixes/Prefix`, in response order.
    pub(crate) common_prefixes: Vec<String>,
}

impl Listing {
    /// A top-level element's text, or the empty string when the response carries none —
    /// which is what "absent" looks like to a client reading a bound member.
    pub(crate) fn field(&self, name: &str) -> String {
        self.top.get(name).cloned().unwrap_or_default()
    }

    /// Every listed `Key`, in response order.
    pub(crate) fn keys(&self) -> Vec<String> {
        self.contents
            .iter()
            .map(|entry| entry.get("Key").cloned().unwrap_or_default())
            .collect()
    }

    /// One `Contents` entry's element, or the empty string when it carries none.
    pub(crate) fn entry_field(entry: &BTreeMap<String, String>, name: &str) -> String {
        entry.get(name).cloned().unwrap_or_default()
    }
}

/// Read a listing body with an XML reader, rather than by slicing the body.
///
/// The reader breaks text at every entity reference and emits the reference on its own,
/// so each element's fragments are accumulated: an ETag travels as `&quot;…&quot;`, and a
/// loop that handled only `Event::Text` would drop its quotes.
pub(crate) fn listing_of(answer: &Answer) -> Listing {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut listing = Listing::default();
    let mut element = String::new();
    let mut current = String::new();
    let mut entry: Option<BTreeMap<String, String>> = None;
    let mut common_prefix: Option<String> = None;
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the listing body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                current.clear();
                match element.as_str() {
                    "Contents" => entry = Some(BTreeMap::new()),
                    "CommonPrefixes" => common_prefix = Some(String::new()),
                    _ => {}
                }
            }
            Event::Text(text) => {
                let decoded = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the listing body must decode: {error}"));
                current.push_str(&unescaped(&decoded));
            }
            Event::GeneralRef(reference) => {
                let name = reference
                    .decode()
                    .unwrap_or_else(|error| panic!("the listing body must decode: {error}"));
                current.push_str(&unescaped(&format!("&{name};")));
            }
            Event::End(end) => {
                let closing = String::from_utf8_lossy(end.name().as_ref()).into_owned();
                match closing.as_str() {
                    "Contents" => {
                        if let Some(done) = entry.take() {
                            listing.contents.push(done);
                        }
                    }
                    "CommonPrefixes" => {
                        if let Some(done) = common_prefix.take() {
                            listing.common_prefixes.push(done);
                        }
                    }
                    // Every other element's text belongs to whichever container is open.
                    name if name == element => {
                        let value = std::mem::take(&mut current);
                        match (entry.as_mut(), common_prefix.as_mut()) {
                            (Some(entry), _) => {
                                entry.insert(name.to_string(), value);
                            }
                            (None, Some(prefix)) if name == "Prefix" => *prefix = value,
                            _ => {
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

/// Write every key in `keys` into `bucket_name`, each with a body of its own length so a
/// listing's `Size` and `ETag` differ per key, and answer what was written, keyed by key.
///
/// Written in REVERSE sorted order, so a listing that reported insertion order rather
/// than key order would fail.
pub(crate) async fn store_keys(
    router: &MultiServiceRouter,
    bucket_name: &str,
    keys: &[&str],
) -> BTreeMap<String, Vec<u8>> {
    let mut written: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut reversed: Vec<&&str> = keys.iter().collect();
    reversed.sort_by(|left, right| right.cmp(left));
    for (index, key) in reversed.iter().enumerate() {
        let body = vec![b'a' + (index as u8 % 26); index + 1];
        let stored = put_object(router, bucket_name, key, &body, &[]).await;
        assert_eq!(
            stored.status, 200,
            "case {key:?}: the write must succeed, got {} with body {:?}",
            stored.status,
            text(&stored)
        );
        written.insert((*key).to_string(), body);
    }
    written
}

/// The keys of `written`, in the ascending UTF-8 byte order S3 lists in.
pub(crate) fn sorted_keys(written: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    written.keys().cloned().collect()
}
