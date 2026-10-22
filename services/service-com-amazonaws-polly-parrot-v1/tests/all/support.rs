// SPDX-License-Identifier: CC0-1.0

//! Fixtures the speech-synthesis-task suites share.
//!
//! One definition of what a started task looks like in the store, and one way to seed
//! it, so `error_semantics` (the read path) and `synthesis_tasks` (the list path)
//! cannot drift on either. The request harness and the response readers live with the
//! lexicon suite (`crate::lexicons_and_speech`), which every suite drives the service
//! through.

use std::time::SystemTime;

use service_com_amazonaws_polly_parrot_v1::state::{service_state, StoredSynthesisTask};

use crate::lexicons_and_speech::Polly;

/// A synthesis task as a start call would have recorded it. A caller overrides the
/// members its case is about.
pub(crate) fn stored_task(task_id: &str, created: SystemTime) -> StoredSynthesisTask {
    StoredSynthesisTask {
        task_id: task_id.to_string(),
        engine: "standard".to_string(),
        output_format: "mp3".to_string(),
        output_uri: format!("s3://polly-output/{task_id}.mp3"),
        creation_time: created,
        request_characters: 11,
        sns_topic_arn: None,
        lexicon_names: Vec::new(),
        sample_rate: Some("22050".to_string()),
        speech_mark_types: Vec::new(),
        text_type: "text".to_string(),
        voice_id: "Joanna".to_string(),
        language_code: None,
        task_status: "inProgress".to_string(),
        task_status_reason: None,
        pending_status: None,
    }
}

/// Put a task in the store the way a start call would.
///
/// `StartSpeechSynthesisTask` is still scaffolded (it has its own stub issue), so the
/// read paths are seeded through the shared engine rather than through that call.
pub(crate) fn seed_task(polly: &Polly, task: &StoredSynthesisTask) {
    let state = service_state(&**polly.engine()).expect("the in-memory engine carries the state");
    state
        .lock()
        .synthesis_tasks
        .insert(task.task_id.clone(), task.clone());
}
