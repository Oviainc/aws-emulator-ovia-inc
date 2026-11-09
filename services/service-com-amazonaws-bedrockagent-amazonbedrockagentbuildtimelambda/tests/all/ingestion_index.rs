// SPDX-License-Identifier: CC0-1.0

//! What an ingestion RECORDS in the bound vector index beside each vector, end to end
//! over HTTP.
//!
//! A knowledge base's vector store holds, for every chunk it ingested, the chunk's vector
//! and the fields Amazon Bedrock records with it: the chunk's text
//! (`AMAZON_BEDROCK_TEXT_CHUNK`), the chunk's own id, the data source it came from and
//! the modality of its source file. Those fields are what makes a retrieval possible —
//! the Bedrock Agent RUNTIME plane serves `Retrieve` by reading them back (see that
//! crate's `tests/all/knowledge_base_retrieval.rs`) — so this suite pins what this plane
//! writes:
//!
//! THE RULE UNDER TEST: the vector an ingestion writes for a source document carries the
//! document's own text as its chunk, a chunk id that is stable across runs and distinct
//! per document, the id of the data source the document came from, and the text modality;
//! and what it recorded does not move when the SOURCE moves — a rewritten document is
//! re-recorded by the next job, and a deleted one keeps what the last job recorded until
//! a job reconciles it away.
//!
//! The rows differ along what the rule generalises over: the document's text and its
//! whitespace, how many documents a data source holds, how many data sources a knowledge
//! base has, and whether a document is new, rewritten or gone.
//!
//! The harness is `ingestion_crawl`'s, so both suites drive the emulator the same way.

use aws_smithy_types::Document;
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::ingestion_index::{
    chunk_id, leading_chunk, CHUNK_ID_METADATA_KEY, CHUNK_MAX_BYTES, CHUNK_MAX_TOKENS,
    CHUNK_TEXT_METADATA_KEY, DATA_SOURCE_ID_METADATA_KEY, SOURCE_FILE_MODALITY_METADATA_KEY,
    SOURCE_FILE_MODALITY_TEXT,
};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::SOURCE_URI_METADATA_KEY;
use service_com_amazonaws_s3vectors_s3vectors::state::{bucket_from_arn, service_state};

use crate::ingestion_crawl::{uri, Emulator, INDEX, VECTOR_BUCKET};

/// A document long enough to be worth a chunk, and with irregular whitespace, so the
/// collapsing a chunk's text goes through is visible.
const HANDBOOK: &str = "The handbook   explains how a request is routed,\n\n  how it is \
     signed\tand where each answer comes from.";

/// A second document on another topic, so one query distinguishes the two.
const MENU: &str = "Lentil soup on Tuesday, roast squash on Thursday.";

/// The metadata the bound index holds with the vector of the document at `key`, as a
/// field -> string map; every field an ingestion records is a string.
fn recorded_fields(emulator: &Emulator, key: &str) -> std::collections::BTreeMap<String, String> {
    let bucket = bucket_from_arn("vectorBucketArn", VECTOR_BUCKET)
        .expect("the suite's vectorBucketArn is a vector bucket ARN");
    let state = service_state(&*emulator.engine).expect("the S3 Vectors store on this engine");
    let store = state.lock();
    let metadata = store
        .index(&bucket, INDEX)
        .ok()
        .and_then(|index| index.vectors.get(&uri(key)))
        .map(|vector| vector.metadata.clone())
        .unwrap_or_else(|| panic!("the bound index holds a vector for {key:?}"));
    let Document::Object(fields) = metadata else {
        panic!("the vector of {key:?} carries a metadata object, got {metadata:?}");
    };
    fields
        .into_iter()
        .filter_map(|(field, value)| match value {
            Document::String(value) => Some((field, value)),
            _ => None,
        })
        .collect()
}

/// RULE: the vector of every ingested source document carries the published fields a
/// retrieval of that chunk is answered from.
#[tokio::test]
async fn an_ingestion_records_the_published_fields_with_every_vector() {
    // (case, the objects the data source holds)
    let rows: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("one document", vec![("handbook.txt", HANDBOOK)]),
        (
            "two documents",
            vec![("handbook.txt", HANDBOOK), ("menu.txt", MENU)],
        ),
        (
            "a document under a prefix, beside its metadata file",
            vec![
                ("public/handbook.txt", HANDBOOK),
                ("public/handbook.txt.metadata.json", "{\"a\":1}"),
            ],
        ),
    ];

    for (case, objects) in rows {
        let emulator = Emulator::new();
        for (key, body) in &objects {
            emulator.hold_object(key, body);
        }
        let knowledge_base = emulator.knowledge_base("recorded-fields-kb").await;
        let data_source = emulator
            .data_source(&knowledge_base, "recorded-fields", None, None)
            .await;
        emulator
            .run_ingestion_job(&knowledge_base, &data_source)
            .await;

        let mut ids: Vec<String> = Vec::new();
        for (key, body) in &objects {
            if key.ends_with(".metadata.json") {
                // A metadata file is the metadata of its sibling, not a document of its
                // own, so no vector is recorded for it.
                continue;
            }
            let fields = recorded_fields(&emulator, key);
            assert_eq!(
                fields.get(CHUNK_TEXT_METADATA_KEY).map(String::as_str),
                Some(leading_chunk(body).as_str()),
                "case {case:?}: expected the chunk of {key:?} to be the document's own \
                 text, got {fields:?}"
            );
            assert_eq!(
                fields.get(DATA_SOURCE_ID_METADATA_KEY).map(String::as_str),
                Some(data_source.as_str()),
                "case {case:?}: expected the data source the document came from, got \
                 {fields:?}"
            );
            assert_eq!(
                fields
                    .get(SOURCE_FILE_MODALITY_METADATA_KEY)
                    .map(String::as_str),
                Some(SOURCE_FILE_MODALITY_TEXT),
                "case {case:?}: expected the modality of a text source file, got {fields:?}"
            );
            assert_eq!(
                fields.get(SOURCE_URI_METADATA_KEY).map(String::as_str),
                Some(uri(key).as_str()),
                "case {case:?}: expected the source URI of {key:?}, got {fields:?}"
            );
            let id = fields
                .get(CHUNK_ID_METADATA_KEY)
                .unwrap_or_else(|| panic!("case {case:?}: no chunk id in {fields:?}"))
                .clone();
            assert!(
                !id.is_empty(),
                "case {case:?}: expected a non-empty chunk id for {key:?}"
            );
            assert_eq!(
                id,
                chunk_id(&data_source, &uri(key)),
                "case {case:?}: expected the chunk id of {key:?} to be the one its \
                 provenance derives"
            );
            assert!(
                !ids.contains(&id),
                "case {case:?}: expected every chunk id to be distinct, {id:?} repeated"
            );
            ids.push(id);
        }
        assert!(
            !ids.is_empty(),
            "case {case:?}: expected at least one document to be recorded"
        );
    }
}

/// RULE: the chunk recorded for a document is the LEADING chunk of its text, bounded both
/// by the chunk size the default chunking documents and by the index's own byte ceiling —
/// so a document longer than either is recorded up to that bound and no further.
#[tokio::test]
async fn the_recorded_chunk_is_the_leading_chunk_of_the_document() {
    let emulator = Emulator::new();
    let long: String = "alpha beta gamma delta ".repeat(CHUNK_MAX_TOKENS);
    emulator.hold_object("long.txt", &long);
    // A document with no whitespace at all is ONE token, so the token bound alone would
    // retain the whole of it: the byte ceiling is what bounds this one.
    let unbroken: String = "x".repeat(CHUNK_MAX_BYTES * 3);
    emulator.hold_object("unbroken.txt", &unbroken);
    let knowledge_base = emulator.knowledge_base("leading-chunk-kb").await;
    let data_source = emulator
        .data_source(&knowledge_base, "leading-chunk", None, None)
        .await;
    emulator
        .run_ingestion_job(&knowledge_base, &data_source)
        .await;

    let fields = recorded_fields(&emulator, "long.txt");
    let chunk = fields
        .get(CHUNK_TEXT_METADATA_KEY)
        .expect("the recorded chunk")
        .clone();
    let words: Vec<&str> = chunk.split_whitespace().collect();
    assert_eq!(
        words.len(),
        CHUNK_MAX_TOKENS,
        "case \"a document longer than one chunk\": expected the chunk to hold the \
         documented maximum of {CHUNK_MAX_TOKENS} words, got {}",
        words.len()
    );
    assert!(
        long.split_whitespace()
            .collect::<Vec<&str>>()
            .join(" ")
            .starts_with(&chunk),
        "case \"a document longer than one chunk\": expected the chunk to start the \
         document, got {chunk:?}"
    );

    let unbroken_fields = recorded_fields(&emulator, "unbroken.txt");
    let unbroken_chunk = unbroken_fields
        .get(CHUNK_TEXT_METADATA_KEY)
        .expect("the recorded chunk")
        .clone();
    assert_eq!(
        unbroken_chunk.len(),
        CHUNK_MAX_BYTES,
        "case \"a document with no whitespace\": expected the chunk to stop at the \
         {CHUNK_MAX_BYTES}-byte ceiling, got {} bytes",
        unbroken_chunk.len()
    );
    assert!(
        unbroken.starts_with(&unbroken_chunk),
        "case \"a document with no whitespace\": expected the chunk to start the document"
    );
}

/// RULE: what an ingestion recorded does not move when the source does — a rewritten
/// document is re-recorded by the NEXT job, and a deleted one keeps what the last job
/// recorded until a job reconciles it away.
#[tokio::test]
async fn what_was_recorded_moves_only_when_a_job_moves_it() {
    let emulator = Emulator::new();
    emulator.hold_object("handbook.txt", HANDBOOK);
    emulator.hold_object("menu.txt", MENU);
    let knowledge_base = emulator.knowledge_base("recorded-window-kb").await;
    let data_source = emulator
        .data_source(&knowledge_base, "recorded-window", None, None)
        .await;
    emulator
        .run_ingestion_job(&knowledge_base, &data_source)
        .await;

    let recorded = recorded_fields(&emulator, "handbook.txt");
    assert_eq!(
        recorded.get(CHUNK_TEXT_METADATA_KEY).map(String::as_str),
        Some(leading_chunk(HANDBOOK).as_str()),
        "case \"the first job\": expected the document's own text to be recorded"
    );

    // The source is rewritten and the other document deleted, with no job in between:
    // what the index holds is what the last job recorded.
    emulator.hold_object("handbook.txt", "The handbook was rewritten today.");
    emulator.drop_object("menu.txt");
    let unmoved = recorded_fields(&emulator, "handbook.txt");
    assert_eq!(
        unmoved, recorded,
        "case \"the source moved, no job yet\": expected the recorded chunk to stand"
    );
    let deleted = recorded_fields(&emulator, "menu.txt");
    assert_eq!(
        deleted.get(CHUNK_TEXT_METADATA_KEY).map(String::as_str),
        Some(leading_chunk(MENU).as_str()),
        "case \"the source moved, no job yet\": expected the deleted document's chunk to \
         stand, got {deleted:?}"
    );

    // The next job is what moves it: the rewritten document is re-recorded, and the
    // deleted one is reconciled out of the index.
    emulator
        .run_ingestion_job(&knowledge_base, &data_source)
        .await;
    let moved = recorded_fields(&emulator, "handbook.txt");
    assert_eq!(
        moved.get(CHUNK_TEXT_METADATA_KEY).map(String::as_str),
        Some(leading_chunk("The handbook was rewritten today.").as_str()),
        "case \"after the next job\": expected the rewritten text to be recorded, got \
         {moved:?}"
    );
    assert_eq!(
        moved.get(CHUNK_ID_METADATA_KEY),
        recorded.get(CHUNK_ID_METADATA_KEY),
        "case \"after the next job\": expected the chunk id of one document to be stable"
    );
}
