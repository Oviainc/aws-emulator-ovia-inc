// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/error_document_message.rs"]
mod error_document_message;
#[path = "all/error_message_wording.rs"]
mod error_message_wording;
#[path = "all/error_semantics.rs"]
mod error_semantics;
#[path = "all/lexicon_listing.rs"]
mod lexicon_listing;
#[path = "all/lexicon_regions.rs"]
mod lexicon_regions;
#[path = "all/lexicons_and_speech.rs"]
mod lexicons_and_speech;
#[path = "all/pls_alphabet_support.rs"]
mod pls_alphabet_support;
#[path = "all/pls_document_validity.rs"]
mod pls_document_validity;
#[path = "all/pls_replacement_measurement.rs"]
mod pls_replacement_measurement;
#[path = "all/pls_replacement_whitespace.rs"]
mod pls_replacement_whitespace;
// The shared PLS document fixture, driven through the path the suites that share it
// drive.
#[path = "all/shared_pls_fixture.rs"]
mod shared_pls_fixture;
#[path = "all/ssml_billed_characters.rs"]
mod ssml_billed_characters;
#[path = "all/ssml_vocabulary.rs"]
mod ssml_vocabulary;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// The synthesis-task fixtures the task suites share.
#[path = "all/support.rs"]
mod support;
#[path = "all/synthesis_tasks.rs"]
mod synthesis_tasks;
#[path = "all/task_report_bounds.rs"]
mod task_report_bounds;
#[path = "all/task_id_validity.rs"]
mod task_id_validity;
#[path = "all/task_retention.rs"]
mod task_retention;
#[path = "all/voice_catalog.rs"]
mod voice_catalog;
#[path = "all/voice_catalog_is_not_paged.rs"]
mod voice_catalog_is_not_paged;
#[path = "all/voice_engine_compatibility.rs"]
mod voice_engine_compatibility;
#[path = "all/xml_well_formedness.rs"]
mod xml_well_formedness;
