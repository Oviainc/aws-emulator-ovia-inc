// SPDX-License-Identifier: CC0-1.0

//! The shadow version counter across a delete, end to end over HTTP through the router
//! this service mounts in production.
//!
//! The rule these cases assert: a shadow's version counter belongs to the SHADOW NAME,
//! not to the document, and it is monotone. An accepted update consumes one version,
//! `DeleteThingShadow` reports the version the document had and consumes one of its own,
//! and the next update of that shadow name resumes above the delete instead of starting
//! again at 1. The counter is the optimistic-concurrency token devices hold, so a write
//! conditioned on a version from before the delete is still refused with
//! `ConflictException` (HTTP 409) afterwards, and only the version the counter now stands
//! at is accepted.
//!
//! The table spans what the rule generalises over — how many updates a document took
//! before the delete, whether the shadow is the unnamed (classic) one or a named one, and
//! how many delete/recreate cycles the name has been through. The recorded case (a
//! document at version 2, deleted, recreated at 4) is one row among them.

use crate::shadow_and_retained_message::{Answer, Harness};
use serde_json::{json, Value};

/// An unconditional update of a shadow.
fn update_document(step: usize) -> Vec<u8> {
    serde_json::to_vec(&json!({"state": {"reported": {"step": step}}})).expect("serializable")
}

/// An update CONDITIONED on `version`: accepted only when that is the version the shadow
/// name's counter stands at.
fn conditional_document(version: i64) -> Vec<u8> {
    serde_json::to_vec(&json!({"version": version, "state": {"reported": {"at": version}}}))
        .expect("serializable")
}

/// The `version` a reply reports.
fn version_of(case: &str, answer: &Answer) -> i64 {
    answer
        .json()
        .get("version")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a version in the body, got {}",
                String::from_utf8_lossy(&answer.body)
            )
        })
}

/// Rows differing in the shadow addressed and in how many updates it took before being
/// deleted — including the one-update case, where a reset counter and a surviving one
/// differ by the least.
fn version_cases() -> Vec<(&'static str, Option<&'static str>, i64)> {
    vec![
        ("the classic shadow after one update", None, 1),
        ("the classic shadow after two updates", None, 2),
        ("a named shadow after one update", Some("cfg"), 1),
        ("a named shadow after three updates", Some("telemetry-1"), 3),
        ("a named shadow after five updates", Some("sensor_a"), 5),
    ]
}

#[tokio::test]
async fn the_version_counter_survives_a_delete_and_the_delete_consumes_one() {
    let harness = Harness::new();

    for (case, shadow, updates) in version_cases() {
        let thing = format!("versionThing{updates}{}", shadow.unwrap_or("classic"));

        // Each accepted update consumes exactly one version.
        for step in 1..=updates {
            let answer = harness
                .update(&thing, shadow, update_document(step as usize))
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected update {step} to be accepted, got {} with body {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                version_of(case, &answer),
                step,
                "case {case:?}: expected one version per accepted update, got {}",
                String::from_utf8_lossy(&answer.body)
            );
        }

        // The delete reports the version the document HAD…
        let deleted = harness.delete_shadow(&thing, shadow).await;
        assert_eq!(
            deleted.status,
            200,
            "case {case:?}: expected the delete to be accepted, got {} with body {}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
        assert_eq!(
            version_of(case, &deleted),
            updates,
            "case {case:?}: expected the deleted version, got {}",
            String::from_utf8_lossy(&deleted.body)
        );

        // …and the document is gone for every reader.
        let read = harness.get_shadow(&thing, shadow).await;
        assert_eq!(
            read.status,
            404,
            "case {case:?}: expected the deleted document to read back 404, got {} with body {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );

        // A write conditioned on a version from BEFORE the delete is still refused: the
        // delete consumed one of its own, so the counter never falls back onto it.
        for (conflict_case, stated) in [
            ("the version the document was deleted at", updates),
            ("the version a fresh shadow would be at", 0),
            ("a version the counter has long passed", 1),
        ] {
            let refused = harness
                .update(&thing, shadow, conditional_document(stated))
                .await;
            assert_eq!(
                refused.status,
                409,
                "case {case:?} / {conflict_case:?}: expected 409 for a stated version of \
                 {stated}, got {} with body {}",
                refused.status,
                String::from_utf8_lossy(&refused.body)
            );
            assert_eq!(
                refused.error_code(),
                "ConflictException",
                "case {case:?} / {conflict_case:?}: expected ConflictException, got {}",
                String::from_utf8_lossy(&refused.body)
            );
        }

        // Only the version the counter now stands at — the one the delete consumed — is
        // accepted, and accepting it resumes the counter above the delete.
        let recreated = harness
            .update(&thing, shadow, conditional_document(updates + 1))
            .await;
        assert_eq!(
            recreated.status,
            200,
            "case {case:?}: expected the update at the post-delete version to be accepted, \
             got {} with body {}",
            recreated.status,
            String::from_utf8_lossy(&recreated.body)
        );
        assert_eq!(
            version_of(case, &recreated),
            updates + 2,
            "case {case:?}: expected the recreated document to resume two above the deleted \
             one, got {}",
            String::from_utf8_lossy(&recreated.body)
        );

        // Every later reader answers from that document, so the surviving counter is
        // stored and not just reported once.
        let reread = harness.get_shadow(&thing, shadow).await;
        assert_eq!(
            version_of(case, &reread),
            updates + 2,
            "case {case:?}: expected the stored document to carry the resumed version, got {}",
            String::from_utf8_lossy(&reread.body)
        );
    }
}

#[tokio::test]
async fn the_counter_keeps_climbing_over_repeated_delete_and_recreate_cycles() {
    let harness = Harness::new();

    for (case, shadow) in [
        ("the classic shadow", None),
        ("a named shadow", Some("cycles")),
    ] {
        let mut expected = 0_i64;
        // Four cycles of "one update, one delete": each consumes two versions, so the
        // counter is at 2 * cycles when the last delete is done.
        for cycle in 1..=4 {
            let created = harness
                .update("cycleThing", shadow, update_document(cycle))
                .await;
            expected += 1;
            assert_eq!(
                version_of(case, &created),
                expected,
                "case {case:?} cycle {cycle}: expected version {expected}, got {}",
                String::from_utf8_lossy(&created.body)
            );

            let deleted = harness.delete_shadow("cycleThing", shadow).await;
            assert_eq!(
                version_of(case, &deleted),
                expected,
                "case {case:?} cycle {cycle}: expected the delete to report the version the \
                 document had, got {}",
                String::from_utf8_lossy(&deleted.body)
            );
            // The delete consumes one of its own.
            expected += 1;
        }

        // Listing a thing whose only shadow was deleted finds no named shadow, which is
        // an empty listing, even though the counter is still held.
        let listed = harness.list_named_shadows("cycleThing", "").await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: expected a thing with no remaining document to list empty, got {} \
             with body {}",
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );
        assert_eq!(
            listed
                .json()
                .get("results")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0),
            "case {case:?}: expected no named shadows left, got body {}",
            String::from_utf8_lossy(&listed.body)
        );

        // And the next update resumes above every version the cycles consumed.
        let resumed = harness
            .update("cycleThing", shadow, update_document(99))
            .await;
        assert_eq!(
            version_of(case, &resumed),
            expected + 1,
            "case {case:?}: expected the counter to resume at {}, got {}",
            expected + 1,
            String::from_utf8_lossy(&resumed.body)
        );
        // Leave the shadow deleted for the next row, which uses the same thing name and
        // its own shadow name.
        harness.delete_shadow("cycleThing", shadow).await;
    }
}
