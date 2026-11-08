// SPDX-License-Identifier: CC0-1.0

//! The server-side defaults Amazon Lex V2 materializes onto a `PromptSpecification`
//! when it stores one.
//!
//! A prompt specification is accepted with only its two required members
//! (`smithy_query(query="shape",shape="PromptSpecification")`: `messageGroups` and
//! `maxRetries`), and the service fills the rest in BEFORE it persists the prompt: the
//! message selection strategy, and the advanced per-attempt input settings for every
//! attempt the prompt has. Every operation that reports the resource then reads those
//! back off the stored record, so a create, a read and a listing agree.
//!
//! Normalizing at WRITE time (rather than per response) is what keeps the stored record
//! and every reader consistent: the defaults are part of the resource, not of one
//! serialization.

use crate::ops::types::{
    AllowedInputTypes, AudioAndDTMFInputSpecification, AudioSpecification, DTMFSpecification,
    IntentConfirmationSetting, PromptAttemptSpecification, PromptSpecification,
    SlotValueElicitationSetting, SubSlotSetting, TextInputSpecification,
    PROMPT_ATTEMPT_VALUES,
};

/// The strategy Amazon Lex selects messages with when a stored prompt names none:
/// "Indicates how a message is selected from a message group among retries"
/// (`smithy_query(query="shape",shape="MessageSelectionStrategy")`: `Random | Ordered`).
pub const MESSAGE_SELECTION_STRATEGY_DEFAULT: &str = "Random";

/// Whether a synthesized attempt lets the user interrupt the prompt. Amazon Lex's
/// default for an un-customized prompt attempt is to allow it.
const DEFAULT_ATTEMPT_ALLOW_INTERRUPT: bool = true;

/// How long Amazon Lex waits for the user to start speaking or pressing a key, in
/// milliseconds — the default start timeout shared by audio and DTMF input.
const DEFAULT_START_TIMEOUT_MS: i32 = 4_000;

/// How long a spoken utterance may run before it is truncated, in milliseconds.
const DEFAULT_AUDIO_MAX_LENGTH_MS: i32 = 15_000;

/// How long Amazon Lex waits after the user stops speaking before taking the utterance
/// to be finished, in milliseconds.
const DEFAULT_AUDIO_END_TIMEOUT_MS: i32 = 640;

/// The most DTMF digits one utterance may carry.
const DEFAULT_DTMF_MAX_LENGTH: i32 = 513;

/// How long Amazon Lex waits after the last DTMF character before taking the input to be
/// concluded, in milliseconds.
const DEFAULT_DTMF_END_TIMEOUT_MS: i32 = 5_000;

/// The DTMF character that clears the digits entered so far and ends the input.
const DEFAULT_DTMF_DELETION_CHARACTER: &str = "*";

/// The DTMF character that ends the input immediately.
const DEFAULT_DTMF_END_CHARACTER: &str = "#";

/// How long Amazon Lex waits for text input before re-prompting, in milliseconds.
const DEFAULT_TEXT_START_TIMEOUT_MS: i32 = 30_000;

/// The attempt names a prompt with that many retries has: the initial attempt plus one
/// per retry, which is how many entries the service's `promptAttemptsSpecification`
/// carries. `maxRetries` is modelled 0..5, and anything outside that is held to the
/// attempts that exist.
pub fn prompt_attempt_names(max_retries: i32) -> impl Iterator<Item = &'static str> {
    // The names and their order are the modelled `PromptAttempt` enum members, which are
    // the keys of `PromptAttemptsSpecificationMap`: the initial attempt first, then one
    // name per retry.
    let attempts = max_retries.clamp(0, PROMPT_ATTEMPT_VALUES.len() as i32 - 1) as usize + 1;
    PROMPT_ATTEMPT_VALUES.iter().copied().take(attempts)
}

/// The advanced settings Amazon Lex materializes for one attempt of a prompt nobody
/// customized: both input types allowed, the user free to interrupt, and the default
/// audio, DTMF and text timeouts.
pub fn default_prompt_attempt_specification() -> PromptAttemptSpecification {
    PromptAttemptSpecification {
        allow_interrupt: Some(DEFAULT_ATTEMPT_ALLOW_INTERRUPT),
        allowed_input_types: AllowedInputTypes {
            allow_audio_input: true,
            allow_dtmf_input: true,
        },
        audio_and_dtmf_input_specification: Some(AudioAndDTMFInputSpecification {
            start_timeout_ms: DEFAULT_START_TIMEOUT_MS,
            audio_specification: Some(AudioSpecification {
                max_length_ms: DEFAULT_AUDIO_MAX_LENGTH_MS,
                end_timeout_ms: DEFAULT_AUDIO_END_TIMEOUT_MS,
            }),
            dtmf_specification: Some(DTMFSpecification {
                max_length: DEFAULT_DTMF_MAX_LENGTH,
                end_timeout_ms: DEFAULT_DTMF_END_TIMEOUT_MS,
                deletion_character: DEFAULT_DTMF_DELETION_CHARACTER.to_string(),
                end_character: DEFAULT_DTMF_END_CHARACTER.to_string(),
            }),
        }),
        text_input_specification: Some(TextInputSpecification {
            start_timeout_ms: DEFAULT_TEXT_START_TIMEOUT_MS,
        }),
    }
}

/// Fill one prompt specification's server-side defaults in, leaving every member the
/// caller supplied exactly as it supplied it: a prompt that names no message selection
/// strategy gets the service's, and a prompt that carries no per-attempt settings gets
/// one default entry per attempt it has.
pub fn normalize_prompt_specification(prompt: &mut PromptSpecification) {
    if prompt.message_selection_strategy.is_none() {
        prompt.message_selection_strategy = Some(MESSAGE_SELECTION_STRATEGY_DEFAULT.to_string());
    }
    if prompt.prompt_attempts_specification.is_none() {
        prompt.prompt_attempts_specification = Some(
            prompt_attempt_names(prompt.max_retries)
                .map(|attempt| {
                    (
                        attempt.to_string(),
                        default_prompt_attempt_specification(),
                    )
                })
                .collect(),
        );
    }
}

/// Normalize the prompt a slot elicits its value with.
pub fn normalize_slot_value_elicitation_setting(setting: &mut SlotValueElicitationSetting) {
    if let Some(prompt) = setting.prompt_specification.as_mut() {
        normalize_prompt_specification(prompt);
    }
}

/// Normalize the prompt each constituent sub slot of a composite slot elicits its value
/// with: a sub slot's elicitation setting carries a prompt specification of its own
/// (`smithy_query(query="shape",shape="SubSlotValueElicitationSetting")`), stored by the
/// same call that stores the slot.
pub fn normalize_sub_slot_setting(setting: &mut SubSlotSetting) {
    for specification in setting
        .slot_specifications
        .iter_mut()
        .flat_map(|specifications| specifications.values_mut())
    {
        normalize_prompt_specification(
            &mut specification.value_elicitation_setting.prompt_specification,
        );
    }
}

/// Normalize the prompt an intent asks the user to confirm it with.
pub fn normalize_intent_confirmation_setting(setting: &mut IntentConfirmationSetting) {
    if let Some(prompt) = setting.prompt_specification.as_mut() {
        normalize_prompt_specification(prompt);
    }
}
