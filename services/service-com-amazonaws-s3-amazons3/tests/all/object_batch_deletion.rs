// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteObjects`, S3's multi-object delete.
//!
//! Every assertion drives a real HTTP request through the router — the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_and_tagging_roundtrip`],
//! [`super::object_storage`] and [`super::object_versioning`] already own — so what is
//! pinned is the wire behaviour (status, the result document, what the store holds
//! afterwards) rather than any handler's internals.
//!
//! The rule under test, asserted over a SET of payloads rather than one request:
//!
//! > One `DeleteObjects` performs the delete of each `Object` its `Delete` payload names —
//! > the version a `VersionId` names, or the simple delete that lays a delete marker on a
//! > bucket carrying a versioning state — and reports the result of each in the response:
//! > a key that held nothing is reported deleted too, a `VersionId` the bucket holds no
//! > version of is that key's own `Errors` entry rather than a failure of the request, and
//! > `Quiet` leaves the successes out.
//!
//! Its sources (this operation's own documentation): "For each key, Amazon S3 performs a
//! delete operation and returns the result of that delete, success or failure, in the
//! response. If the object specified in the request isn't found, Amazon S3 confirms the
//! deletion by returning the result as deleted"; "The request can contain a list of up to
//! 1,000 keys that you want to delete"; "In quiet mode the response includes only keys
//! where the delete operation encountered an error"; and, for the marker a simple delete
//! lays, "If bucket versioning is enabled, the operation inserts a delete marker, which
//! becomes the current version of the object" (`DeleteObject`).

use std::collections::BTreeMap;

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::state::VERSION_ID_CHARS;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::bucket_tagging::unescaped;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_storage::{list_objects_v2, listing_of};
use super::object_versioning::{list_object_versions, set_versioning, version_listing_of};

/// One identifier of a `Delete` payload: the key, and the version id when the case names
/// one.
type Identifier<'a> = (&'a str, Option<&'a str>);

/// The `Delete` payload a `DeleteObjects` request carries, from the identifiers and the
/// quiet flag a case names.
fn delete_payload(identifiers: &[Identifier], quiet: bool) -> String {
    let mut payload =
        String::from("<Delete xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    for (key, version_id) in identifiers {
        payload.push_str("<Object><Key>");
        payload.push_str(key);
        payload.push_str("</Key>");
        if let Some(version_id) = version_id {
            payload.push_str("<VersionId>");
            payload.push_str(version_id);
            payload.push_str("</VersionId>");
        }
        payload.push_str("</Object>");
    }
    if quiet {
        payload.push_str("<Quiet>true</Quiet>");
    }
    payload.push_str("</Delete>");
    payload
}

/// `POST /<bucket>?delete` — `DeleteObjects`, with the payload the case carries.
async fn delete_objects(
    router: &MultiServiceRouter,
    name: &str,
    identifiers: &[Identifier<'_>],
    quiet: bool,
) -> Answer {
    send(
        router,
        "POST",
        &format!("/{name}?delete"),
        &[],
        &delete_payload(identifiers, quiet),
    )
    .await
}

/// One `DeleteObjects` response, as a client's XML reader sees it: one map per `Deleted`
/// and per `Error` entry, each in document order.
#[derive(Debug, Default)]
struct DeleteResult {
    /// One map per `Deleted` entry, keyed by element name.
    deleted: Vec<BTreeMap<String, String>>,
    /// One map per `Error` entry, keyed by element name.
    errors: Vec<BTreeMap<String, String>>,
}

impl DeleteResult {
    /// Every `Deleted` entry's key, in response order.
    fn deleted_keys(&self) -> Vec<String> {
        self.deleted
            .iter()
            .map(|entry| entry.get("Key").cloned().unwrap_or_default())
            .collect()
    }

    /// Every `Error` entry as `(key, code)`, in response order.
    fn error_codes(&self) -> Vec<(String, String)> {
        self.errors
            .iter()
            .map(|entry| {
                (
                    entry.get("Key").cloned().unwrap_or_default(),
                    entry.get("Code").cloned().unwrap_or_default(),
                )
            })
            .collect()
    }

    /// The `Deleted` entry for `key`, or `None` when the response reports none.
    fn deleted_entry(&self, key: &str) -> Option<&BTreeMap<String, String>> {
        self.deleted
            .iter()
            .find(|entry| entry.get("Key").map(String::as_str) == Some(key))
    }
}

/// Read a delete result with an XML reader, the same accumulate-per-element walk
/// [`super::object_versioning::version_listing_of`] does for a version listing.
fn delete_result_of(answer: &Answer) -> DeleteResult {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut result = DeleteResult::default();
    let mut element = String::new();
    let mut current = String::new();
    let mut entry: Option<(bool, BTreeMap<String, String>)> = None;
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the result body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                current.clear();
                match element.as_str() {
                    "Deleted" => entry = Some((true, BTreeMap::new())),
                    "Error" => entry = Some((false, BTreeMap::new())),
                    _ => {}
                }
            }
            Event::Text(text) => {
                let decoded = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the result body must decode: {error}"));
                current.push_str(&unescaped(&decoded));
            }
            Event::End(end) => {
                let closing = String::from_utf8_lossy(end.name().as_ref()).into_owned();
                match closing.as_str() {
                    "Deleted" | "Error" => {
                        if let Some((is_deleted, done)) = entry.take() {
                            if is_deleted {
                                result.deleted.push(done);
                            } else {
                                result.errors.push(done);
                            }
                        }
                    }
                    name if name == element => {
                        let value = std::mem::take(&mut current);
                        if let Some((_, entry)) = entry.as_mut() {
                            entry.insert(name.to_string(), value);
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
    result
}

/// Write `keys` into the bucket, handing back the version id each write minted (the empty
/// string when the bucket minted none).
async fn write_keys(
    router: &MultiServiceRouter,
    name: &str,
    keys: &[&str],
) -> BTreeMap<String, String> {
    let mut minted = BTreeMap::new();
    for key in keys {
        let written = put_object(router, name, key, format!("body of {key}").as_bytes(), &[]).await;
        assert_eq!(
            written.status,
            200,
            "the write of {key:?} must be accepted, got {} with body {:?}",
            written.status,
            text(&written)
        );
        minted.insert(
            (*key).to_string(),
            written.header("x-amz-version-id").unwrap_or_default(),
        );
    }
    minted
}

#[tokio::test]
async fn a_batch_delete_reports_the_result_of_every_identifier_it_names() {
    let keys: &[&str] = &["one", "agent/two.txt", "agent/nested/three.txt"];

    // (case, the keys the payload names, quiet). The rows differ in how many identifiers
    // one payload carries, in the shape of the keys, in whether a named key is even
    // stored, and in the response mode.
    let rows: &[(&str, &[&str], bool)] = &[
        ("one stored key", &["one"], false),
        ("every stored key at once", keys, false),
        (
            "a key that is not stored is still reported deleted",
            &["one", "never-written"],
            false,
        ),
        ("only keys that are not stored", &["absent/a", "absent/b"], false),
        ("quiet mode reports no success", keys, true),
        ("the same key twice", &["one", "one"], false),
    ];

    for (case, named, quiet) in rows {
        // A fresh store per row, so one row's deletes cannot decide another's.
        let router = router();
        let name = "batch-delete";
        bucket(&router, name).await;
        write_keys(&router, name, keys).await;

        let answer = delete_objects(&router, name, &identifiers(named), *quiet).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a multi-object delete answers 200, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let result = delete_result_of(&answer);
        assert!(
            result.errors.is_empty(),
            "case {case:?}: no identifier can fail here, got {:?}",
            result.error_codes()
        );
        let expected: Vec<String> = if *quiet {
            Vec::new()
        } else {
            named.iter().map(|key| (*key).to_string()).collect()
        };
        assert_eq!(
            result.deleted_keys(),
            expected,
            "case {case:?}: expected one Deleted entry per identifier in request order, got \
             {:?}",
            result.deleted_keys()
        );

        // Whatever the response mode, every key the payload named is gone from the
        // listing, and every key it did not name is still there.
        let left = listing_of(&list_objects_v2(&router, name, "").await).keys();
        for key in keys {
            let still_there = !named.contains(key);
            assert_eq!(
                left.contains(&(*key).to_string()),
                still_there,
                "case {case:?}: key {key:?} must {} be listed, got {left:?}",
                if still_there { "still" } else { "no longer" }
            );
        }
    }
}

#[tokio::test]
async fn a_batch_delete_drains_a_versioned_bucket_and_reports_each_marker() {
    let router = router();
    let name = "batch-delete-versions";
    bucket(&router, name).await;
    set_versioning(&router, name, "Enabled").await;
    let keys: &[&str] = &["one", "agent/two.txt"];
    let minted = write_keys(&router, name, keys).await;

    // Without a version id each identifier is the simple delete, so each one lays a delete
    // marker and reports the marker it created.
    let laid = delete_objects(&router, name, &identifiers(keys), false).await;
    assert_eq!(
        laid.status,
        200,
        "the simple batch delete answers 200, got {} with body {:?}",
        laid.status,
        text(&laid)
    );
    let result = delete_result_of(&laid);
    assert_eq!(
        result.deleted_keys(),
        keys.iter().map(|key| (*key).to_string()).collect::<Vec<_>>(),
        "every key is reported deleted, got {:?}",
        result.deleted_keys()
    );
    let mut marker_ids: Vec<(String, String)> = Vec::new();
    for key in keys {
        let entry = result
            .deleted_entry(key)
            .unwrap_or_else(|| panic!("case {key:?}: the key must be reported deleted"));
        assert_eq!(
            entry.get("DeleteMarker").map(String::as_str),
            Some("true"),
            "case {key:?}: the delete reports that it created a delete marker, got {entry:?}"
        );
        let marker_id = entry
            .get("DeleteMarkerVersionId")
            .cloned()
            .unwrap_or_default();
        assert!(
            !marker_id.is_empty() && marker_id != minted[*key],
            "case {key:?}: the marker carries a minted id of its own, got {marker_id:?}"
        );
        marker_ids.push(((*key).to_string(), marker_id));
    }

    // Nothing was taken away: each key's version is still stored behind its marker.
    let listing = version_listing_of(&list_object_versions(&router, name, "").await);
    assert_eq!(
        listing.entries().len(),
        4,
        "two versions and two markers are stored, got {:?}",
        listing.entries()
    );
    for key in keys {
        let read = get_object(
            &router,
            name,
            key,
            &format!("?versionId={}", minted[*key]),
            &[],
        )
        .await;
        assert_eq!(
            read.status,
            200,
            "case {key:?}: the version behind the marker is still readable, got {} with body \
             {:?}",
            read.status,
            text(&read)
        );
    }

    // Naming every version AND every marker empties the bucket in one request, and a
    // `VersionId` the bucket holds no version of is that identifier's own error rather
    // than a failure of the request.
    let mut named: Vec<(String, Option<String>)> = Vec::new();
    for key in keys {
        named.push(((*key).to_string(), Some(minted[*key].clone())));
    }
    for (key, marker_id) in &marker_ids {
        named.push((key.clone(), Some(marker_id.clone())));
    }
    // A token this store never minted, in two shapes: one of the minted shape and one of
    // no version id's shape at all. The BATCH path reports what it could not do with the
    // pair rather than classifying the token, so each is that identifier's own
    // absent-version error (the single-object path is where the argument itself is judged,
    // see `version_id_classification`).
    named.push((
        "one".to_string(),
        Some("a".repeat(VERSION_ID_CHARS)),
    ));
    named.push(("one".to_string(), Some("not-a-version".to_string())));
    let borrowed: Vec<Identifier> = named
        .iter()
        .map(|(key, version_id)| (key.as_str(), version_id.as_deref()))
        .collect();

    let drained = delete_objects(&router, name, &borrowed, false).await;
    assert_eq!(
        drained.status,
        200,
        "the versioned batch delete answers 200, got {} with body {:?}",
        drained.status,
        text(&drained)
    );
    let result = delete_result_of(&drained);
    assert_eq!(
        result.deleted_keys().len(),
        4,
        "every version and marker named is reported deleted, got {:?}",
        result.deleted_keys()
    );
    assert_eq!(
        result.error_codes(),
        vec![
            ("one".to_string(), "NoSuchVersion".to_string()),
            ("one".to_string(), "NoSuchVersion".to_string())
        ],
        "each identifier naming a version the bucket holds none of is that identifier's own \
         error, got {:?}",
        result.error_codes()
    );
    let emptied = version_listing_of(&list_object_versions(&router, name, "").await);
    assert!(
        emptied.entries().is_empty(),
        "the bucket holds no version afterwards, got {:?}",
        emptied.entries()
    );
}

#[tokio::test]
async fn a_batch_delete_outside_what_it_can_serve_is_refused_not_guessed() {
    let router = router();
    let name = "batch-delete-refusals";
    bucket(&router, name).await;
    write_keys(&router, name, &["one"]).await;

    // A payload that names no Object at all, and one past the published ceiling of 1,000,
    // are both outside the schema S3 publishes for `Delete`: each is REFUSED with 400, not
    // answered as a success that deleted nothing, and each carries `MalformedXML` — the
    // code S3 answers a document that does not validate against its published schema.
    // The empty one is refused by the boundary validator before the handler runs
    // (`Delete$Objects` is a required member) and the over-long one by the handler, so one
    // code for one logical failure is what says the two cannot drift apart.
    let too_many: Vec<String> = (0..1_001).map(|index| format!("key-{index}")).collect();
    let too_many: Vec<Identifier> = too_many.iter().map(|key| (key.as_str(), None)).collect();
    for (case, identifiers, code) in [
        ("no Object element at all", Vec::new(), "MalformedXML"),
        (
            "one more than the 1,000-key ceiling",
            too_many,
            "MalformedXML",
        ),
    ] {
        let answer = delete_objects(&router, name, &identifiers, false).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            answer.error_code()
        );
    }

    // A ceiling-sized payload IS served, so the refusal above is the bound and not a
    // smaller one of this emulator's own.
    let at_the_ceiling: Vec<String> = (0..1_000).map(|index| format!("key-{index}")).collect();
    let at_the_ceiling: Vec<Identifier> = at_the_ceiling
        .iter()
        .map(|key| (key.as_str(), None))
        .collect();
    let answer = delete_objects(&router, name, &at_the_ceiling, true).await;
    assert_eq!(
        answer.status,
        200,
        "case \"exactly the 1,000-key ceiling\": expected HTTP 200, got {} with body {:?}",
        answer.status,
        text(&answer)
    );

    // The whole request is addressed to one bucket, so a bucket this emulator does not
    // hold refuses it entirely rather than reporting a per-key error.
    let absent = delete_objects(
        &router,
        "bucket-that-was-never-created",
        &[("one", None)],
        false,
    )
    .await;
    assert_eq!(
        absent.status,
        404,
        "case \"a bucket that does not exist\": expected HTTP 404, got {} with body {:?}",
        absent.status,
        text(&absent)
    );
    assert_eq!(
        absent.error_code(),
        "NoSuchBucket",
        "case \"a bucket that does not exist\": expected NoSuchBucket, got {:?}",
        absent.error_code()
    );

    // An MFA token is something this emulator cannot judge — it holds no authentication
    // device — so a request that carries one keeps the scaffold's 501 instead of being
    // answered as a success that ignored it.
    let with_mfa = send(
        &router,
        "POST",
        &format!("/{name}?delete"),
        &[("x-amz-mfa", "arn:aws:iam::123456789012:mfa/device 123456")],
        &delete_payload(&[("one", None)], false),
    )
    .await;
    assert_eq!(
        with_mfa.status,
        501,
        "case \"an MFA token\": expected HTTP 501, got {} with body {:?}",
        with_mfa.status,
        text(&with_mfa)
    );

    // Neither refusal changed anything.
    assert_eq!(
        listing_of(&list_objects_v2(&router, name, "").await).keys(),
        vec!["one".to_string()],
        "a refused batch delete keeps every object"
    );
}

/// The identifiers of a payload that names keys and no version ids.
fn identifiers<'a>(keys: &'a [&'a str]) -> Vec<Identifier<'a>> {
    keys.iter().map(|key| (*key, None)).collect()
}
