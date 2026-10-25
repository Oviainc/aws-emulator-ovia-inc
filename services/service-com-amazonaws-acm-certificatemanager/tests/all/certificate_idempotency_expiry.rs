// SPDX-License-Identifier: CC0-1.0

//! The hour a `RequestCertificate` idempotency entry stands for its certificate, and
//! what the store does with the entry once it has timed out.
//!
//! RULE: an idempotency token identifies one certificate request for exactly as long as
//! it is live — `RequestCertificateRequest$IdempotencyToken` publishes the window ("if
//! you call RequestCertificate multiple times with the same idempotency token within one
//! hour, ACM recognizes that you are requesting only one certificate"), which
//! `state::IDEMPOTENCY_TOKEN_TTL` holds. Inside the window the same request answers the
//! ARN it first created; from the window's end the token stands for nothing, so the same
//! request creates a SECOND certificate and the entry that stood for the first is gone
//! from the index rather than kept for the life of the process.
//!
//! Every case drives the wire through the harness in `certificate_lifecycle`. The age of
//! a token is set by writing the instant the store itself records — the same field the
//! service wrote — so a row can stand on either side of the published hour without
//! waiting it out.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_acm_certificatemanager::state;

use crate::certificate_lifecycle::{call, request_certificate, router_with_engine};

/// Age every stored idempotency entry by `age`, by moving the instant the index
/// recorded it that far into the past.
fn age_idempotency_entries(engine: &Arc<engine::InMemoryEngine>, age: Duration) {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    for recorded in store.idempotency.values_mut() {
        recorded.recorded_at -= age;
    }
}

/// How many entries the idempotency index holds.
fn stored_tokens(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let held = store.lock().idempotency.len();
    held
}

/// RULE: the entry stands for its certificate while the token is live and for nothing
/// once the published window has passed, and the index keeps only the live entries.
#[tokio::test]
async fn an_idempotency_entry_lives_for_exactly_the_published_hour() {
    // Both sides of the published bound, and one age well past it: the window runs from
    // the instant the request was recorded, so a token older than it identifies nothing.
    let hour = state::IDEMPOTENCY_TOKEN_TTL;
    let minute = Duration::from_secs(60);
    let cases: &[(&str, Duration, bool)] = &[
        ("a token recorded just now", Duration::ZERO, true),
        ("a token a minute old", minute, true),
        ("a token a minute short of the window's end", hour - minute, true),
        ("a token exactly at the window's end", hour, false),
        ("a token a minute past the window's end", hour + minute, false),
        ("a token twice the window old", hour * 2, false),
    ];

    for (case, age, reused) in cases {
        let (router, engine) = router_with_engine();
        let request = json!({
            "DomainName": "idempotency-window.example.com",
            "ValidationMethod": "DNS",
            "IdempotencyToken": "windowtoken"
        });

        let first = request_certificate(&router, &request).await;
        assert_eq!(
            stored_tokens(&engine),
            1,
            "case {case:?}: the request must have recorded its token, got {} entries",
            stored_tokens(&engine)
        );

        age_idempotency_entries(&engine, *age);
        let second = request_certificate(&router, &request).await;

        if *reused {
            assert_eq!(
                second, first,
                "case {case:?}: a live token must answer the certificate it created, got {second:?} after {first:?}"
            );
        } else {
            assert_ne!(
                second, first,
                "case {case:?}: a token past the published window identifies nothing, so the \
                 request must create a second certificate, got {second:?}"
            );
        }

        // Either way the index holds exactly the entries that still stand for
        // something: the one this request recorded, and never the dead one beside it.
        assert_eq!(
            stored_tokens(&engine),
            1,
            "case {case:?}: expected one live idempotency entry, got {}",
            stored_tokens(&engine)
        );

        // And both certificates are still readable, so expiring an entry retires the
        // TOKEN rather than the certificate it named.
        for arn in [&first, &second] {
            let (status, body) = call(
                &router,
                "DescribeCertificate",
                &json!({ "CertificateArn": arn }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: {arn} must still be readable, got {status} {body}"
            );
        }
    }
}

/// RULE: a deleted certificate's entry stands for nothing either — the index drops it,
/// so the same request creates a new certificate rather than answering an ARN the store
/// no longer holds.
#[tokio::test]
async fn a_deleted_certificates_idempotency_entry_is_dropped() {
    let (router, engine) = router_with_engine();
    let request = json!({
        "DomainName": "idempotency-deleted.example.com",
        "ValidationMethod": "DNS",
        "IdempotencyToken": "deletedtoken"
    });

    let first = request_certificate(&router, &request).await;
    let (status, body) = call(
        &router,
        "DeleteCertificate",
        &json!({ "CertificateArn": &first }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteCertificate must succeed, got {status} {body}"
    );
    assert_eq!(
        stored_tokens(&engine),
        0,
        "deleting the certificate must take its idempotency entry with it, got {} entries",
        stored_tokens(&engine)
    );

    let second = request_certificate(&router, &request).await;
    assert_ne!(
        second, first,
        "the token of a deleted certificate must not resurrect its ARN, got {second:?}"
    );
    assert_eq!(
        stored_tokens(&engine),
        1,
        "the second request records one entry, got {}",
        stored_tokens(&engine)
    );
}
