// SPDX-License-Identifier: CC0-1.0

//! Cross-account readiness authorizations under CONCURRENCY: simultaneous
//! `CreateCrossAccountAuthorization` calls must still leave the store a SET.
//!
//! The single-threaded tables for this surface — value forms, pagination, the delete
//! and the refusals — live in [`crate::cross_account_authorizations`], whose `Emulator`
//! harness this suite drives rather than spelling the router setup out again. What it
//! adds is the one axis those tables cannot cover: several creates IN FLIGHT AT ONCE
//! against the one shared store, which is what the recorded
//! `route53-recovery-readiness.concurrent-create-cross-account-authorization-idempotent-dedup`
//! unit exercises. A handler that read the store and wrote it back without holding its
//! lock across both, or that keyed authorizations by anything other than the value
//! itself, reports a duplicate here and nowhere else.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the
//! production emulator mounts and inspects only the HTTP status and the JSON body, so an
//! answer that is not computed from persisted state cannot pass.
//!
//! The tests are tables, whose rows differ along what the rule generalises over: how
//! many calls run at once, whether they carry the same value or distinct ones, and which
//! form the value takes.

use std::sync::Arc;

use crate::cross_account_authorizations::Emulator;

/// Forms a `CrossAccountAuthorization` value takes. The shape is a bare string with
/// neither `@length` nor `@pattern`
/// (`smithy_query(query="shape", shape="CrossAccountAuthorization")`), so the rows here
/// are account-root ARNs, the form a cross-account readiness authorization is granted
/// to — chosen so this suite asserts the CONCURRENCY rule alone and stays correct
/// whatever the open issue on the value's format settles on
/// (`20261022T204335Z-route53-recovery-readiness-createcrossaccountauthorization-accepts-a-malformed-authorization.md`).
const AUTHORIZATION_FORMS: &[&str] = &[
    "arn:aws:iam::111122223333:root",
    "arn:aws:iam::444455556666:root",
];

/// How many creates this suite runs at once. One row per concurrency level, the last
/// above the runtime's worker count so the calls really do overlap.
const CONCURRENCY_LEVELS: &[usize] = &[2, 8, 16];

/// Creating ONE value from several calls at once leaves exactly one membership, and
/// every one of those calls reports that value back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_creates_of_one_value_leave_exactly_one_authorization() {
    for concurrency in CONCURRENCY_LEVELS.iter().copied() {
        for form in AUTHORIZATION_FORMS {
            let case = format!("{form} x{concurrency} at once");
            let emulator = Arc::new(Emulator::new());

            let mut calls = Vec::new();
            for _ in 0..concurrency {
                let emulator = Arc::clone(&emulator);
                let value = (*form).to_string();
                calls.push(tokio::spawn(async move {
                    let reply = emulator.create(&value).await;
                    (reply.status, reply.string("crossAccountAuthorization"))
                }));
            }
            for call in calls {
                let (status, reported) = call.await.expect("create task");
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200 from every simultaneous create, got {status}"
                );
                assert_eq!(
                    reported.as_deref(),
                    Some(*form),
                    "case {case:?}: expected the stored authorization back, got {reported:?}"
                );
            }

            assert_eq!(
                emulator.authorizations(&case).await,
                vec![(*form).to_string()],
                "case {case:?}: expected one membership however many creates ran at once"
            );
        }
    }
}

/// Creating DISTINCT values from several calls at once stores every one of them exactly
/// once, in the one stable order the list reports.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_creates_of_distinct_values_store_each_one_exactly_once() {
    for concurrency in CONCURRENCY_LEVELS.iter().copied() {
        let case = format!("{concurrency} distinct at once");
        let emulator = Arc::new(Emulator::new());
        // Account-root ARNs, so the axis under test is the number in flight.
        let values: Vec<String> = (0..concurrency)
            .map(|index| format!("arn:aws:iam::{:012}:root", 100_000_000_000_u64 + index as u64))
            .collect();

        let mut calls = Vec::new();
        for value in values.clone() {
            let emulator = Arc::clone(&emulator);
            calls.push(tokio::spawn(async move {
                let reply = emulator.create(&value).await;
                // The value THIS call sent travels back with its answer, so a handler
                // that answered one in-flight caller with another's value — the
                // cross-talk this suite exists to catch — cannot pass the assertion
                // below by reporting some other row's value.
                (value, reply.status, reply.string("crossAccountAuthorization"))
            }));
        }
        for call in calls {
            let (sent, status, reported) = call.await.expect("create task");
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from every simultaneous create of {sent:?}, got {status}"
            );
            assert_eq!(
                reported.as_deref(),
                Some(sent.as_str()),
                "case {case:?}: expected the create of {sent:?} to report back the value IT sent, got {reported:?}"
            );
        }

        let mut expected = values.clone();
        expected.sort();
        assert_eq!(
            emulator.authorizations(&case).await,
            expected,
            "case {case:?}: expected every value stored exactly once, in value order"
        );
    }
}
