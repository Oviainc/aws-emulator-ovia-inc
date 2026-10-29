// SPDX-License-Identifier: CC0-1.0

//! The data-plane write into an index of a domain — `crate::state::ingest`, the one path
//! a producer outside this service (a Firehose stream delivering to a domain, say) writes
//! a document through.
//!
//! The domain and the index are created over the real wire path, so what the write is
//! judged against is what a client created; the write itself is the state API, because it
//! is a producer's call rather than a modelled operation of this service.
//!
//! The rules asserted here are the ones the write OWNS, each over a set of inputs:
//!
//! * a document lands under the id the write names and reads back as it was sent, and a
//!   second write to the SAME id replaces it and numbers the next version rather than
//!   adding a copy;
//! * every caller-driven growth the write can cause is bounded — documents per index,
//!   bytes per document, and indexes auto-created on a domain — and a write past a bound
//!   is refused rather than stored;
//! * a domain this store does not hold, and one whose provisioning has not landed, answer
//!   nothing: the write is reported as unavailable rather than as a document anybody
//!   refused.

use std::time::SystemTime;

use serde_json::json;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::ingest::{
    self, DocumentWrite, WriteRefusal, INDEX_DOCUMENT_QUOTA, MAX_DOCUMENT_BYTES,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::INDEX_QUOTA;

use super::data_source_registry::created_domain;
use super::domain_lifecycle::Emulator;

/// The role a write is authorized as. No IAM store is mounted here, so this emulator holds
/// no identity policy for it — the position the module documents as "no evidence", which a
/// domain carrying no access policy admits.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/producer";

/// The ARN of a domain this suite created, read out of the store the create wrote it to.
fn domain_arn(emulator: &Emulator, domain: &str) -> String {
    let store = emulator.store();
    let guard = store.lock();
    guard
        .domains
        .get(domain)
        .unwrap_or_else(|| panic!("expected the store to hold domain {domain}"))
        .arn
        .clone()
}

/// One write of `document` into `index` of the domain at `address`, under `document_id`.
fn write(
    emulator: &Emulator,
    address: &str,
    index: &str,
    document_id: Option<&str>,
    document: &[u8],
) -> Result<ingest::Indexed, WriteRefusal> {
    ingest::index_document(
        &**emulator.engine(),
        &DocumentWrite {
            domain: address,
            index_name: index,
            document_id,
            document,
            principal_arn: ROLE_ARN,
            now: SystemTime::now(),
        },
    )
}

/// The documents one index holds, as `(id, source, version)`, read out of the store the
/// write put them in.
fn stored(emulator: &Emulator, domain: &str, index: &str) -> Vec<(String, serde_json::Value, u64)> {
    let store = emulator.store();
    let guard = store.lock();
    guard
        .domains
        .get(domain)
        .and_then(|domain| domain.indexes.get(index))
        .map(|index| {
            index
                .documents
                .values()
                .map(|document| {
                    (
                        document.id.clone(),
                        document.source.clone(),
                        document.version,
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: a written document is held under the id the write named and reads back as it was
/// sent; a second write to the same id REPLACES it and carries the next version, so a
/// delivery attempted twice leaves one document rather than two.
#[tokio::test]
async fn a_second_write_to_one_id_replaces_the_document_and_numbers_the_version() {
    let emulator = Emulator::new();
    created_domain(&emulator, "documents").await;
    let arn = domain_arn(&emulator, "documents");

    let first = write(
        &emulator,
        &arn,
        "events",
        Some("record-1"),
        br#"{"event":"one"}"#,
    )
    .expect("the first write is taken");
    assert_eq!(
        first.document_id, "record-1",
        "case \"the first write\": expected the id the write named, got {first:?}"
    );
    assert_eq!(
        stored(&emulator, "documents", "events"),
        vec![("record-1".to_string(), json!({ "event": "one" }), 1)],
        "case \"the first write\": expected the document as sent, at version 1"
    );

    write(
        &emulator,
        &arn,
        "events",
        Some("record-1"),
        br#"{"event":"again"}"#,
    )
    .expect("the second write is taken");
    assert_eq!(
        stored(&emulator, "documents", "events"),
        vec![("record-1".to_string(), json!({ "event": "again" }), 2)],
        "case \"a second write to the same id\": expected one document at version 2"
    );

    // A write naming NO id is named by the engine, so it is a document of its own rather
    // than a replacement of the one above.
    let minted = write(&emulator, &arn, "events", None, br#"{"event":"unnamed"}"#)
        .expect("a write naming no id is taken");
    assert_ne!(
        minted.document_id, "record-1",
        "case \"a write naming no id\": expected an id of its own, got {minted:?}"
    );
    assert_eq!(
        stored(&emulator, "documents", "events").len(),
        2,
        "case \"a write naming no id\": expected a second document"
    );
}

/// RULE: every growth a producer can drive is bounded, and a write past a bound is REFUSED
/// rather than stored.
///
/// The rows are the three bounds the write can reach: the bytes of one document, the
/// documents of one index, and the indexes a write may create on one domain.
#[tokio::test]
async fn every_bound_the_write_can_reach_refuses_rather_than_growing_the_store() {
    let emulator = Emulator::new();
    created_domain(&emulator, "bounded").await;
    let arn = domain_arn(&emulator, "bounded");

    // One document past the byte bound, with a document just inside it taken.
    let filler = |bytes: usize| format!(r#"{{"t":"{}"}}"#, "x".repeat(bytes)).into_bytes();
    let inside = filler(MAX_DOCUMENT_BYTES / 2);
    assert!(
        write(&emulator, &arn, "sizes", Some("inside"), &inside).is_ok(),
        "case \"a document inside the byte bound\": expected it to be taken"
    );
    let past = filler(MAX_DOCUMENT_BYTES + 1);
    let refused = write(&emulator, &arn, "sizes", Some("past"), &past);
    assert!(
        matches!(
            &refused,
            Err(WriteRefusal::Rejected { error_type, .. })
                if *error_type == ingest::ILLEGAL_ARGUMENT_EXCEPTION
        ),
        "case \"a document past the byte bound\": expected a refusal, got {refused:?}"
    );
    assert_eq!(
        stored(&emulator, "bounded", "sizes").len(),
        1,
        "case \"a document past the byte bound\": expected it not to be stored"
    );

    // One document past the per-index document bound.
    for ordinal in 0..INDEX_DOCUMENT_QUOTA {
        let document = format!(r#"{{"n":{ordinal}}}"#);
        write(
            &emulator,
            &arn,
            "many",
            Some(&format!("id-{ordinal}")),
            document.as_bytes(),
        )
        .unwrap_or_else(|refusal| {
            panic!("case \"filling the index\": write {ordinal} was refused: {refusal:?}")
        });
    }
    let over = write(&emulator, &arn, "many", Some("one-too-many"), br#"{"n":-1}"#);
    assert!(
        matches!(
            &over,
            Err(WriteRefusal::Rejected { error_type, .. })
                if *error_type == ingest::ILLEGAL_ARGUMENT_EXCEPTION
        ),
        "case \"a document past the index's document bound\": expected a refusal, got {over:?}"
    );
    assert_eq!(
        stored(&emulator, "bounded", "many").len(),
        INDEX_DOCUMENT_QUOTA,
        "case \"a document past the index's document bound\": expected the index not to grow"
    );
    // A write to an id the index already holds is a REPLACEMENT, so the bound does not
    // refuse it: the index does not grow either way.
    assert!(
        write(&emulator, &arn, "many", Some("id-0"), br#"{"n":0}"#).is_ok(),
        "case \"replacing a document at the bound\": expected it to be taken"
    );

    // One index past the per-domain index bound. The domain already holds the two indexes
    // the writes above created.
    let held = {
        let store = emulator.store();
        let guard = store.lock();
        guard
            .domains
            .get("bounded")
            .map(|domain| domain.indexes.len())
            .unwrap_or_default()
    };
    for ordinal in held..INDEX_QUOTA {
        write(
            &emulator,
            &arn,
            &format!("index-{ordinal}"),
            Some("only"),
            br#"{"n":1}"#,
        )
        .unwrap_or_else(|refusal| {
            panic!("case \"filling the domain\": index {ordinal} was refused: {refusal:?}")
        });
    }
    let crowded = write(&emulator, &arn, "one-index-too-many", Some("only"), br#"{}"#);
    assert!(
        matches!(
            &crowded,
            Err(WriteRefusal::Rejected { error_type, .. })
                if *error_type == ingest::ILLEGAL_ARGUMENT_EXCEPTION
        ),
        "case \"an index past the domain's index bound\": expected a refusal, got {crowded:?}"
    );
    let store = emulator.store();
    let guard = store.lock();
    assert_eq!(
        guard
            .domains
            .get("bounded")
            .map(|domain| domain.indexes.len()),
        Some(INDEX_QUOTA),
        "case \"an index past the domain's index bound\": expected the domain not to grow"
    );
}

/// RULE: a domain this store does not hold answers nothing, so the write is reported as
/// UNAVAILABLE — a delivery that has not happened — rather than as a document something
/// refused.
///
/// The rows are the ways an address can name no reachable domain: an ARN of the documented
/// form that no domain carries, a host that is no domain's endpoint, and the empty address.
#[tokio::test]
async fn an_address_no_domain_answers_is_reported_as_unavailable() {
    let emulator = Emulator::new();
    created_domain(&emulator, "reachable").await;
    let arn = domain_arn(&emulator, "reachable");

    // (the case, the address)
    let rows: &[(&str, &str)] = &[
        (
            "an ARN no domain carries",
            "arn:aws:es:us-east-1:000000000000:domain/never-created",
        ),
        ("a host no domain publishes", "search-nowhere.example.com"),
        ("no address at all", ""),
    ];
    for (case, address) in rows {
        let answer = write(&emulator, address, "events", Some("one"), br#"{"a":1}"#);
        assert!(
            matches!(&answer, Err(WriteRefusal::Unavailable { .. })),
            "case {case:?}: expected the write to be unavailable, got {answer:?}"
        );
    }

    // And the domain this store DOES hold is reachable by both of the addresses that name
    // it: its ARN and the host of its published endpoint.
    let endpoint = {
        let store = emulator.store();
        let guard = store.lock();
        guard
            .domains
            .get("reachable")
            .map(|domain| domain.endpoint.clone())
            .unwrap_or_default()
    };
    for (case, address) in [("its ARN", arn.as_str()), ("its endpoint", endpoint.as_str())] {
        let answer = write(&emulator, address, "events", Some(case), br#"{"a":1}"#);
        assert!(
            answer.is_ok(),
            "case {case:?}: expected the write to reach the domain, got {answer:?}"
        );
    }
}
