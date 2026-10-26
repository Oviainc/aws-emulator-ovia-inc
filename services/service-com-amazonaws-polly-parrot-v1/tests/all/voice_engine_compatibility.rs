// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for which refusal a `SynthesizeSpeech` voice/engine mismatch
//! carries.
//!
//! RULE: which code a refusal carries is decided by what Amazon Polly's own responses
//! carry, not by the model merely declaring a code for the condition — so a request
//! naming an `Engine` the designated voice is not available on is refused as a
//! REQUEST-SHAPE failure: HTTP 400 with the error code `ValidationException` and the
//! sentence Amazon Polly words that refusal with, whose one variable part is the engine
//! the request asked for. A pair the catalog `DescribeVoices` serves does list is
//! synthesised. `EngineNotSupportedException` is declared for this operation by the
//! model but is not on this service's wire for any voice/engine pair, the same way
//! `UnsupportedPlsAlphabetException` is declared for `PutLexicon` and never sent.
//!
//! The rule is checked over a TABLE that spans the class rather than the one pair an
//! issue happens to quote: every DISTINCT supported-engine set the catalog carries
//! (single-engine voices, every pair, and the three-engine sets) crossed with every
//! modelled `Engine` value, so each row is both a refusal case and a success case for
//! the voices that do support it, plus the request that names no engine at all (the
//! default engine, which is `standard`).

use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::ops::types::ENGINE_VALUES;

use crate::lexicons_and_speech::{error_code, json, voices, Polly};
use crate::support::message_slot;

/// The sentence Amazon Polly refuses a voice/engine mismatch with, for `engine`.
///
/// OBSERVED: this is the text Amazon Polly's own response carries for this refusal
/// ("This voice does not support the selected engine: neural" for `Engine=neural`), so
/// the fixed half is pinned here and the variable half is taken from the request.
fn mismatch_message(engine: &str) -> String {
    format!("This voice does not support the selected engine: {engine}")
}

/// The engines the catalog lists for each voice, keyed by voice id — read through
/// `DescribeVoices`, which is the state the synthesis check is made against.
async fn supported_engines(polly: &Polly) -> BTreeMap<String, BTreeSet<String>> {
    let mut catalog = BTreeMap::new();
    for voice in voices(polly, "").await {
        let id = voice["Id"]
            .as_str()
            .expect("every voice reports its Id")
            .to_string();
        let engines: BTreeSet<String> = voice["SupportedEngines"]
            .as_array()
            .expect("every voice reports its SupportedEngines")
            .iter()
            .map(|engine| {
                engine
                    .as_str()
                    .expect("a supported engine is a string")
                    .to_string()
            })
            .collect();
        catalog.insert(id, engines);
    }
    catalog
}

/// One voice per DISTINCT supported-engine set in the catalog, so the rows differ along
/// what the rule generalises over — which engines a voice is available on — rather than
/// along the voice's name.
async fn one_voice_per_engine_set(polly: &Polly) -> Vec<(String, BTreeSet<String>)> {
    let mut seen: BTreeSet<BTreeSet<String>> = BTreeSet::new();
    let mut rows = Vec::new();
    for (id, engines) in supported_engines(polly).await {
        if seen.insert(engines.clone()) {
            rows.push((id, engines));
        }
    }
    rows
}

/// Assert the refusal one request carried: status, code, and the message member.
fn assert_mismatch_refusal(case: &str, response: &http::Response<Bytes>, engine: &str) {
    assert_eq!(
        (response.status().as_u16(), error_code(response)),
        (400, "ValidationException".to_string()),
        "case {case:?}: expected the request-shape refusal, body {:?}",
        json(response)
    );
    let slot = message_slot(response).unwrap_or_else(|| {
        panic!(
            "case {case:?}: the error document carries no message member, body {:?}",
            json(response)
        )
    });
    assert_eq!(
        slot.as_str(),
        Some(mismatch_message(engine).as_str()),
        "case {case:?}: expected the refusal's sentence for engine {engine:?}, got {slot:?}"
    );
}

/// RULE: an `Engine` the designated voice is not available on is HTTP 400
/// `ValidationException` naming that engine, and an engine the catalog lists for the
/// voice is synthesised — over every distinct supported-engine set in the catalog
/// crossed with every modelled engine value.
#[tokio::test]
async fn an_engine_the_voice_does_not_support_is_the_request_shape_refusal() {
    let polly = Polly::new();
    let rows = one_voice_per_engine_set(&polly).await;
    assert!(
        rows.len() >= 6,
        "the catalog must span several supported-engine sets, got {:?}",
        rows.iter().map(|(id, _)| id).collect::<Vec<_>>()
    );

    let mut refused = 0usize;
    let mut served = 0usize;
    for (voice, engines) in &rows {
        for engine in ENGINE_VALUES {
            let case = format!("voice {voice} (engines {engines:?}) on engine {engine}");
            let response = polly
                .synthesize(&serde_json::json!({
                    "Text": "hello world",
                    "VoiceId": voice,
                    "Engine": engine,
                    "OutputFormat": "mp3",
                }))
                .await;
            if engines.contains(*engine) {
                assert_eq!(
                    response.status().as_u16(),
                    200,
                    "case {case:?}: the catalog lists this engine, so synthesis must \
                     succeed, body {:?}",
                    String::from_utf8_lossy(response.body())
                );
                assert!(
                    !response.body().is_empty(),
                    "case {case:?}: a served request must carry an audio stream"
                );
                served += 1;
            } else {
                assert_mismatch_refusal(&case, &response, engine);
                refused += 1;
            }
        }
    }
    assert!(
        refused >= 6 && served >= 6,
        "the table must cover both outcomes, got {refused} refusals and {served} \
         served requests"
    );
}

/// RULE, for the request that names NO engine: the default engine is `standard`, so a
/// voice the catalog does not list under `standard` is refused the same way, with
/// `standard` as the named engine.
#[tokio::test]
async fn the_default_engine_is_checked_against_the_voice_too() {
    /// How many catalog voices each leg of the default-engine check is driven over.
    const VOICES_PER_SIDE: usize = 3;

    let polly = Polly::new();
    let catalog = supported_engines(&polly).await;
    let mut refused = 0usize;
    let mut served = 0usize;
    // The rows are read off the catalog: the first few voices that DO support the
    // default engine and the first few that do not, so both legs are exercised without
    // naming a single voice by hand.
    for (voice, engines) in &catalog {
        let supports_default = engines.contains("standard");
        if supports_default && served >= VOICES_PER_SIDE {
            continue;
        }
        if !supports_default && refused >= VOICES_PER_SIDE {
            continue;
        }
        let case = format!("voice {voice} (engines {engines:?}) with no Engine member");
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": "hello world",
                "VoiceId": voice,
                "OutputFormat": "mp3",
            }))
            .await;
        if supports_default {
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case:?}: the voice supports the default engine, body {:?}",
                String::from_utf8_lossy(response.body())
            );
            served += 1;
        } else {
            assert_mismatch_refusal(&case, &response, "standard");
            refused += 1;
        }
    }
    assert!(
        refused >= VOICES_PER_SIDE && served >= VOICES_PER_SIDE,
        "the catalog must offer {VOICES_PER_SIDE} voices on each side of the default \
         engine, got {refused} refusals and {served} served requests"
    );
}

/// RULE's boundary: `EngineNotSupportedException` is declared by the model for this
/// operation but is not on this service's wire — no voice/engine pair produces it.
#[tokio::test]
async fn the_modeled_engine_error_is_not_on_this_services_wire() {
    let polly = Polly::new();
    for (voice, engines) in one_voice_per_engine_set(&polly).await {
        for engine in ENGINE_VALUES {
            if engines.contains(*engine) {
                continue;
            }
            let response = polly
                .synthesize(&serde_json::json!({
                    "Text": "hello world",
                    "VoiceId": voice,
                    "Engine": engine,
                    "OutputFormat": "mp3",
                }))
                .await;
            let code = error_code(&response);
            assert_ne!(
                code, "EngineNotSupportedException",
                "case {voice:?} on engine {engine:?}: this code is not on the wire"
            );
        }
    }
}
