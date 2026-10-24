// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EMPTINESS test `DeleteBucket` is refused by.
//!
//! Every assertion drives a real HTTP request through the router — the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_and_tagging_roundtrip`],
//! [`super::object_storage`] and [`super::object_versioning`] already own — so what is
//! pinned is the wire behaviour (status, error code, what the bucket still holds) rather
//! than any handler's internals.
//!
//! The rules under test, each asserted over a SET of bucket histories rather than one
//! request:
//!
//! > 1. `DeleteBucket` succeeds only when the bucket holds NO object version at all. While
//! >    it holds any — a current object, a version that is only reachable by its own
//! >    version id because a delete displaced it, or a delete marker — the call is refused
//! >    with `BucketNotEmpty` (HTTP 409) and nothing about the bucket changes; once every
//! >    version is gone the delete succeeds and the bucket stops existing.
//! > 2. On a bucket that carries a versioning state, a delete that names no version takes
//! >    nothing away: it lays a DELETE MARKER that becomes the key's current version, so
//! >    the key leaves the current view while every version behind the marker stays stored
//! >    and addressable — and removing the marker makes the version it hid current again.
//!
//! Their sources: "All objects (including all object versions and delete markers) in the
//! bucket must be deleted before the bucket itself can be deleted" (`DeleteBucket`'s own
//! documentation); "If bucket versioning is enabled, the operation inserts a delete marker,
//! which becomes the current version of the object. To permanently delete an object in a
//! versioned bucket, you must include the object's versionId in the request" and "If the
//! specified version is a delete marker, the response returns a 405 Method Not Allowed
//! error" (`DeleteObject` / `GetObject`); "Enabled—Enables versioning for the objects in
//! the bucket. All objects added to the bucket receive a unique version ID. Suspended—…
//! All objects added to the bucket receive the version ID null" (`PutBucketVersioning`).
//!
//! `BucketNotEmpty` and the 409 are the requirement. The refusal MESSAGE is an example, so
//! what is asserted of it is only that a versioned bucket's refusal says what is left to
//! delete are versions — the distinction S3's recorded wording draws — and never a literal
//! sentence.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{head_bucket, router};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_storage::{delete_bucket, list_objects_v2, listing_of};
use super::object_versioning::{
    delete_object, head_object, list_object_versions, set_versioning, version_listing_of,
    VersionListing,
};

/// One step of a bucket's history, so a case is a sequence of real requests rather than a
/// hand-built state.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// `PutBucketVersioning` with this `BucketVersioningStatus`.
    Versioning(&'static str),
    /// `PutObject` at this key, with bytes nothing else in the case writes.
    Write(&'static str),
    /// `DeleteObject` at this key naming NO `versionId` — the delete that, on a
    /// versioning-enabled bucket, hides the version it displaces behind a delete marker.
    Delete(&'static str),
    /// `DeleteObject` once per version the bucket still holds for this key, each naming
    /// that version's id — the permanent delete.
    PurgeVersions(&'static str),
}

/// One row: the case name, the history, and how many versions the bucket must still hold
/// once that history has run.
type Row<'a> = (&'a str, &'a [Step], usize);

/// Every `(key, version id)` the bucket still holds, as `ListObjectVersions` reports them.
async fn stored_versions(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
) -> Vec<(String, String)> {
    let listed = list_object_versions(router, name, "").await;
    assert_eq!(
        listed.status,
        200,
        "case {case:?}: a version listing of a bucket that exists is 200, got {} with body {:?}",
        listed.status,
        text(&listed)
    );
    version_listing_of(&listed).entries()
}

/// Run `steps` against the bucket `name`, asserting every request in the history was
/// accepted — so a failing case fails on the delete under test, not on its setup.
async fn apply(router: &MultiServiceRouter, name: &str, case: &str, steps: &[Step]) {
    let mut written = 0usize;
    for step in steps {
        match *step {
            Step::Versioning(status) => set_versioning(router, name, status).await,
            Step::Write(key) => {
                written += 1;
                let body = format!("{key} revision {written}");
                let answer = put_object(router, name, key, body.as_bytes(), &[]).await;
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?}: the write of {key:?} must be accepted, got {} with body {:?}",
                    answer.status,
                    text(&answer)
                );
            }
            Step::Delete(key) => {
                let answer = delete_object(router, name, key, "").await;
                assert_eq!(
                    answer.status, 204,
                    "case {case:?}: the delete of {key:?} must be accepted, got {} with body {:?}",
                    answer.status,
                    text(&answer)
                );
            }
            Step::PurgeVersions(key) => {
                for (held_key, version_id) in stored_versions(router, name, case).await {
                    if held_key != key {
                        continue;
                    }
                    let answer =
                        delete_object(router, name, key, &format!("?versionId={version_id}")).await;
                    assert_eq!(
                        answer.status, 204,
                        "case {case:?}: the delete of {key:?} version {version_id:?} must be \
                         accepted, got {} with body {:?}",
                        answer.status,
                        text(&answer)
                    );
                }
            }
        }
    }
}

/// Whether the history ever gave the bucket a versioning state, which is what decides
/// which of S3's two refusal wordings applies.
fn versioned(steps: &[Step]) -> bool {
    steps
        .iter()
        .any(|step| matches!(step, Step::Versioning(_)))
}

#[tokio::test]
async fn a_bucket_is_deletable_only_once_it_holds_no_version_at_all() {
    // The rows differ along what the rule generalises over: the bucket's versioning state
    // (never set, `Enabled`, `Suspended` after `Enabled`), how many keys it holds, how
    // many versions each key accumulates, the shape of the key, and whether the deletes
    // that emptied the current view named a version id or not. The recorded case is ONE of
    // them: "versioning enabled, every key deleted without a version id".
    let rows: &[Row] = &[
        ("a bucket that holds nothing at all", &[], 0),
        (
            "no versioning state, one key",
            &[Step::Write("only.txt")],
            1,
        ),
        (
            "no versioning state, two keys, one of them deleted",
            &[
                Step::Write("one"),
                Step::Write("a/deep/two.txt"),
                Step::Delete("one"),
            ],
            1,
        ),
        (
            "no versioning state, every key written then deleted",
            &[
                Step::Write("one"),
                Step::Write("a/deep/two.txt"),
                Step::Delete("one"),
                Step::Delete("a/deep/two.txt"),
            ],
            0,
        ),
        (
            "no versioning state, a key overwritten then deleted",
            &[
                Step::Write("only.txt"),
                Step::Write("only.txt"),
                Step::Delete("only.txt"),
            ],
            0,
        ),
        (
            "versioning enabled, three keys still current",
            &[
                Step::Versioning("Enabled"),
                Step::Write("script-one.sh"),
                Step::Write("agent/script-two.sh"),
                Step::Write("agent/nested/script-three.sh"),
            ],
            3,
        ),
        (
            // The recorded case: every key deleted once, so the CURRENT view is empty
            // (ListObjectsV2 reports no key) while each delete kept the version it
            // displaced and laid a delete marker over it — two stored versions per key.
            "versioning enabled, every key deleted without a version id",
            &[
                Step::Versioning("Enabled"),
                Step::Write("script-one.sh"),
                Step::Write("agent/script-two.sh"),
                Step::Write("agent/nested/script-three.sh"),
                Step::Delete("script-one.sh"),
                Step::Delete("agent/script-two.sh"),
                Step::Delete("agent/nested/script-three.sh"),
            ],
            6,
        ),
        (
            "versioning enabled, one key with three revisions then deleted",
            &[
                Step::Versioning("Enabled"),
                Step::Write("only.txt"),
                Step::Write("only.txt"),
                Step::Write("only.txt"),
                Step::Delete("only.txt"),
            ],
            4,
        ),
        (
            "versioning enabled, every version purged while current",
            &[
                Step::Versioning("Enabled"),
                Step::Write("only.txt"),
                Step::Write("only.txt"),
                Step::PurgeVersions("only.txt"),
            ],
            0,
        ),
        (
            "versioning enabled, keys deleted and then every version purged",
            &[
                Step::Versioning("Enabled"),
                Step::Write("one"),
                Step::Write("agent/two.txt"),
                Step::Delete("one"),
                Step::Delete("agent/two.txt"),
                Step::PurgeVersions("one"),
                Step::PurgeVersions("agent/two.txt"),
            ],
            0,
        ),
        (
            // Suspension is a state of the BUCKET, not of the versions it holds: the
            // delete removes the `null` version and lays a `null` delete marker in its
            // place, and the version minted while versioning was enabled stays.
            "versioning suspended, the null version deleted and a minted one left",
            &[
                Step::Versioning("Enabled"),
                Step::Write("only.txt"),
                Step::Versioning("Suspended"),
                Step::Write("only.txt"),
                Step::Delete("only.txt"),
            ],
            2,
        ),
        (
            "versioning suspended, every version purged",
            &[
                Step::Versioning("Enabled"),
                Step::Write("only.txt"),
                Step::Versioning("Suspended"),
                Step::Write("only.txt"),
                Step::PurgeVersions("only.txt"),
            ],
            0,
        ),
    ];

    for (case, steps, held) in rows {
        // A fresh store per row, so one row's leftovers cannot decide another's.
        let router = router();
        let name = "deletion-emptiness";
        bucket(&router, name).await;
        apply(&router, name, case, steps).await;

        let before = stored_versions(&router, name, case).await;
        assert_eq!(
            before.len(),
            *held,
            "case {case:?}: this history must leave {held} stored version(s), got {before:?}"
        );

        let answer = delete_bucket(&router, name).await;
        if *held == 0 {
            assert_eq!(
                answer.status,
                204,
                "case {case:?}: a bucket holding no version is deleted, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            // Once it is deleted the bucket stops existing, version history and all:
            // a version listing of it reports no such bucket rather than an empty one.
            assert_eq!(
                head_bucket(&router, name).await.status,
                404,
                "case {case:?}: the deleted bucket must stop existing"
            );
            let listed = list_object_versions(&router, name, "").await;
            assert_eq!(
                listed.status,
                404,
                "case {case:?}: a version listing of the deleted bucket is not found, got {} \
                 with body {:?}",
                listed.status,
                text(&listed)
            );
            assert_eq!(
                listed.error_code(),
                "NoSuchBucket",
                "case {case:?}: it answers NoSuchBucket, got {:?}",
                listed.error_code()
            );
            continue;
        }

        assert_eq!(
            answer.status,
            409,
            "case {case:?}: {held} stored version(s) must refuse the delete with a conflict, \
             got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "BucketNotEmpty",
            "case {case:?}: the refusal is BucketNotEmpty, got {:?}",
            answer.error_code()
        );
        // The wording is an example; what it must carry is which thing is left to delete.
        let message = answer.error_message();
        if versioned(steps) {
            assert!(
                message.to_lowercase().contains("version"),
                "case {case:?}: a versioned bucket's refusal must say its versions are what \
                 is left to delete, got {message:?}"
            );
        } else {
            assert!(
                !message.is_empty(),
                "case {case:?}: the refusal must carry a message, got {message:?}"
            );
        }

        // A refusal changes NOTHING: the bucket is still there, it still holds exactly the
        // versions it did, and every one of them is still addressable by its id.
        assert_eq!(
            head_bucket(&router, name).await.status,
            200,
            "case {case:?}: a refused delete keeps the bucket"
        );
        let after = stored_versions(&router, name, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused delete keeps every stored version, got {after:?}"
        );
        for (key, version_id) in &after {
            let read = get_object(
                &router,
                name,
                key,
                &format!("?versionId={version_id}"),
                &[],
            )
            .await;
            // An object version answers with its bytes; a delete marker answers 405 — "if
            // the specified version is a delete marker, the response returns a 405 Method
            // Not Allowed error" (`GetObject`). Either way the version is STILL THERE: a
            // refused bucket delete took nothing away, so no read of it answers 404.
            assert!(
                read.status == 200 || read.status == 405,
                "case {case:?}: version {version_id:?} of {key:?} must still be there \
                 after a refused delete (200 for an object version, 405 for a delete \
                 marker), got {} with body {:?}",
                read.status,
                text(&read)
            );
        }
    }
}

#[tokio::test]
async fn emptying_the_version_history_is_what_lets_the_delete_through() {
    // The teardown path a client written against AWS takes: the delete is refused, the
    // caller drains the version history the refusal told it about, and the retry succeeds.
    // The rows differ in what the leftovers are — versions hidden behind a delete that
    // named no version id, and several revisions of one key still current.
    let rows: &[(&str, &[Step])] = &[
        (
            "every key deleted without a version id",
            &[
                Step::Versioning("Enabled"),
                Step::Write("one"),
                Step::Write("agent/two.txt"),
                Step::Write("agent/nested/three.txt"),
                Step::Delete("one"),
                Step::Delete("agent/two.txt"),
                Step::Delete("agent/nested/three.txt"),
            ],
        ),
        (
            "one key holding three revisions",
            &[
                Step::Versioning("Enabled"),
                Step::Write("only.txt"),
                Step::Write("only.txt"),
                Step::Write("only.txt"),
            ],
        ),
    ];

    for (case, steps) in rows {
        let router = router();
        let name = "deletion-drain";
        bucket(&router, name).await;
        apply(&router, name, case, steps).await;

        let refused = delete_bucket(&router, name).await;
        assert_eq!(
            refused.status,
            409,
            "case {case:?}: the first delete is refused, got {} with body {:?}",
            refused.status,
            text(&refused)
        );

        // The current view says nothing about what is left: these buckets' keys are
        // reported by a version listing only.
        let listed = listing_of(&list_objects_v2(&router, name, "").await);
        let current = listed.keys();
        let held = stored_versions(&router, name, case).await;
        assert!(
            held.len() > current.len(),
            "case {case:?}: the version history must hold more than the current view, got \
             {held:?} against {current:?}"
        );

        // Draining it one version at a time: still refused while one is left.
        for (index, (key, version_id)) in held.iter().enumerate() {
            let purged = delete_object(&router, name, key, &format!("?versionId={version_id}"))
                .await;
            assert_eq!(
                purged.status, 204,
                "case {case:?}: the delete of version {version_id:?} must be accepted, got {}",
                purged.status
            );
            let expected = if index + 1 == held.len() { 204 } else { 409 };
            let retried = delete_bucket(&router, name).await;
            assert_eq!(
                retried.status,
                expected,
                "case {case:?}: with {} version(s) left the delete must answer {expected}, got \
                 {} with body {:?}",
                held.len() - index - 1,
                retried.status,
                text(&retried)
            );
            if expected == 409 {
                assert_eq!(
                    retried.error_code(),
                    "BucketNotEmpty",
                    "case {case:?}: the refusal is BucketNotEmpty, got {:?}",
                    retried.error_code()
                );
            }
        }

        // And the bucket is gone for good: nothing re-creates it, and a second delete
        // reports that it does not exist.
        let again = delete_bucket(&router, name).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: a deleted bucket is not found, got {} with body {:?}",
            again.status,
            text(&again)
        );
        assert_eq!(
            again.error_code(),
            "NoSuchBucket",
            "case {case:?}: it answers NoSuchBucket, got {:?}",
            again.error_code()
        );
    }
}

#[tokio::test]
async fn a_delete_that_names_no_version_lays_a_marker_and_takes_nothing_away() {
    // (case, the versioning statuses applied in order, the id the marker must carry). The
    // rows span both states a versioning-ENABLED-or-suspended bucket can be in when the
    // delete arrives: "Enabled—… All objects added to the bucket receive a unique version
    // ID" mints the marker an id of its own, while "Suspended—… All objects added to the
    // bucket receive the version ID null" lays the `null` marker. A bucket with NO
    // versioning state lays no marker at all, which the table in
    // `a_bucket_is_deletable_only_once_it_holds_no_version_at_all` covers through the rows
    // whose delete leaves the bucket empty.
    let rows: &[(&str, &[&str], Option<&str>)] = &[
        ("versioning enabled", &["Enabled"], None),
        (
            "versioning suspended after being enabled",
            &["Enabled", "Suspended"],
            Some("null"),
        ),
    ];

    for (case, statuses, marker_id_is) in rows {
        let router = router();
        let name = "delete-marker";
        bucket(&router, name).await;
        let key = "agent/script.sh";
        // The version the marker will hide is written while versioning is ENABLED, so it
        // carries an id of its own whatever the bucket's status is when the delete arrives
        // ("to permanently delete an object in a versioned bucket, you must include the
        // object's versionId in the request").
        set_versioning(&router, name, "Enabled").await;
        let body = b"#!/bin/sh\necho hello\n";
        let written = put_object(&router, name, key, body, &[]).await;
        assert_eq!(
            written.status,
            200,
            "case {case:?}: the write must be accepted, got {} with body {:?}",
            written.status,
            text(&written)
        );
        let hidden_id = written
            .header("x-amz-version-id")
            .unwrap_or_else(|| panic!("case {case:?}: a versioned write must mint an id"));
        for status in &statuses[1..] {
            set_versioning(&router, name, status).await;
        }

        // The delete reports the marker it created: "indicates whether the specified
        // object version that was permanently deleted was (true) or was not (false) a
        // delete marker" and "returns the version ID of the delete marker created as a
        // result of the DELETE operation".
        let deleted = delete_object(&router, name, key, "").await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: the delete is a 204, got {} with body {:?}",
            deleted.status,
            text(&deleted)
        );
        assert!(
            deleted.body.is_empty(),
            "case {case:?}: a 204 carries no body, got {:?}",
            text(&deleted)
        );
        assert_eq!(
            deleted.header("x-amz-delete-marker").as_deref(),
            Some("true"),
            "case {case:?}: the delete reports that it created a delete marker, got {:?}",
            deleted.header("x-amz-delete-marker")
        );
        let marker_id = deleted.header("x-amz-version-id").unwrap_or_default();
        match marker_id_is {
            Some(expected) => assert_eq!(
                marker_id, *expected,
                "case {case:?}: expected the marker id {expected:?}, got {marker_id:?}"
            ),
            None => {
                assert!(
                    !marker_id.is_empty() && marker_id != "null" && marker_id != hidden_id,
                    "case {case:?}: an enabled bucket's marker carries a minted id of its \
                     own, got {marker_id:?}"
                );
            }
        }

        // The key is gone from every read that names no version — "Amazon S3 behaves as if
        // the object was deleted" — while the version behind the marker answers with its
        // bytes.
        let absent = get_object(&router, name, key, "", &[]).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: a read of the key reads as absent, got {} with body {:?}",
            absent.status,
            text(&absent)
        );
        assert_eq!(
            absent.error_code(),
            "NoSuchKey",
            "case {case:?}: expected NoSuchKey, got {:?}",
            absent.error_code()
        );
        assert!(
            !listing_of(&list_objects_v2(&router, name, "").await)
                .keys()
                .contains(&key.to_string()),
            "case {case:?}: the key leaves the current listing, got {:?}",
            listing_of(&list_objects_v2(&router, name, "").await).keys()
        );
        let hidden = get_object(&router, name, key, &format!("?versionId={hidden_id}"), &[]).await;
        assert_eq!(
            hidden.status,
            200,
            "case {case:?}: the version behind the marker is still readable, got {} with \
             body {:?}",
            hidden.status,
            text(&hidden)
        );
        assert_eq!(
            hidden.body.as_ref(),
            body,
            "case {case:?}: it answers the bytes the write stored"
        );

        // A read that names the MARKER is 405: it holds no bytes to answer with.
        for (reader, answer) in [
            (
                "GetObject",
                get_object(&router, name, key, &format!("?versionId={marker_id}"), &[]).await,
            ),
            (
                "HeadObject",
                head_object(&router, name, key, &format!("?versionId={marker_id}")).await,
            ),
        ] {
            assert_eq!(
                answer.status, 405,
                "case {case:?}, {reader}: naming a delete marker is 405, got {} with body \
                 {:?}",
                answer.status,
                text(&answer)
            );
        }

        // The version listing reports the marker as a delete marker — in its own
        // container, with no size and no entity tag — and as the key's latest version.
        let listing = version_listing_of(&list_object_versions(&router, name, "").await);
        assert_eq!(
            listing.entries().len(),
            2,
            "case {case:?}: the marker is listed beside the version it hides, got {:?}",
            listing.entries()
        );
        let marker = listing
            .entry_for(&marker_id)
            .unwrap_or_else(|| panic!("case {case:?}: the marker must be listed"));
        assert_eq!(
            VersionListing::entry(&marker, "Key"),
            key,
            "case {case:?}: the marker carries the key it was laid on, got {:?}",
            VersionListing::entry(&marker, "Key")
        );
        assert_eq!(
            VersionListing::entry(&marker, "IsLatest"),
            "true",
            "case {case:?}: the marker is the key's current version, got {:?}",
            VersionListing::entry(&marker, "IsLatest")
        );
        assert_eq!(
            VersionListing::entry(&marker, "Size"),
            "",
            "case {case:?}: a delete marker reports no size, got {:?}",
            VersionListing::entry(&marker, "Size")
        );
        assert_eq!(
            VersionListing::entry(&marker, "ETag"),
            "",
            "case {case:?}: a delete marker reports no entity tag, got {:?}",
            VersionListing::entry(&marker, "ETag")
        );
        let hidden_entry = listing
            .entry_for(&hidden_id)
            .unwrap_or_else(|| panic!("case {case:?}: the hidden version must be listed"));
        assert_eq!(
            VersionListing::entry(&hidden_entry, "IsLatest"),
            "false",
            "case {case:?}: the version behind the marker is not the latest one, got {:?}",
            VersionListing::entry(&hidden_entry, "IsLatest")
        );

        // And removing the marker makes the version it hid current again: the delete a
        // marker performs is reversible, which is the whole point of laying one.
        let purged = delete_object(&router, name, key, &format!("?versionId={marker_id}")).await;
        assert_eq!(
            purged.status, 204,
            "case {case:?}: the marker is removable by its id, got {} with body {:?}",
            purged.status,
            text(&purged)
        );
        assert_eq!(
            purged.header("x-amz-delete-marker").as_deref(),
            Some("true"),
            "case {case:?}: the delete reports that what it removed was a delete marker, \
             got {:?}",
            purged.header("x-amz-delete-marker")
        );
        let restored = get_object(&router, name, key, "", &[]).await;
        assert_eq!(
            restored.status,
            200,
            "case {case:?}: the version the marker hid is current again, got {} with body \
             {:?}",
            restored.status,
            text(&restored)
        );
        assert_eq!(
            restored.body.as_ref(),
            body,
            "case {case:?}: it answers the bytes the write stored"
        );
    }
}
