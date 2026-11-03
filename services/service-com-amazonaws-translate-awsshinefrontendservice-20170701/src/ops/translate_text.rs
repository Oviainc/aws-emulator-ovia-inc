// SPDX-License-Identifier: CC0-1.0

//! @op TranslateText
//!
//! Translates the request's text from the source language into the target
//! language, applying the terms of the named custom terminology from the stored
//! record and reporting exactly the terms that matched.
//!
//! The translation itself is a deterministic synthetic transform marked as such
//! — this emulator hosts no translation model — but everything around it is
//! real: which terminology applies, which of its terms matched, the text they
//! were substituted into, and the resolved source language all come from the
//! request and from persisted state.
//!
//! Shared types live in `super::types`; the stored shapes and everything the
//! five terminology operations report by live in [`crate::state`].
//!
//! ---
//!
//! # Modeled contract for `TranslateText`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json")
//! ```
//!
//! Translates input text from the source language to the target language. For a list of
//! available languages and language codes, see Supported languages.
//!
//! ## Input
//!
//! * `Settings` (TranslationSettings) — Settings to configure your translation output. You
//!   can configure the following options: Brevity: reduces the length of the translated
//!   output for most translations. Formality: sets the formality level of the output text.
//!   Profanity: masks profane words and phrases in your translation output.
//! * `SourceLanguageCode` (LanguageCodeString, len 2..5) **required** — The language code for
//!   the language of the source text. For a list of language codes, see Supported languages.
//!   To have Amazon Translate determine the source language of your text, you can specify
//!   auto in the SourceLanguageCode field. If you specify auto, Amazon Translate will call
//!   Amazon Comprehend to determine the source language. If you specify auto, you must send
//!   the TranslateText request in a region that supports Amazon Comprehend. Otherwise, the
//!   request returns an error indicating that autodetect is not supported.
//! * `TargetLanguageCode` (LanguageCodeString, len 2..5) **required** — The language code
//!   requested for the language of the target text. For a list of language codes, see
//!   Supported languages.
//! * `TerminologyNames` (ResourceNameList) — The name of a terminology list file to add to
//!   the translation job. This file provides source terms and the desired translation for
//!   each term. A terminology list can contain a maximum of 256 terms. You can use one custom
//!   terminology resource in your translation request. Use the ListTerminologies operation to
//!   get the available terminology lists. For more information about custom terminology
//!   lists, see Custom terminology.
//! * `Text` (BoundedLengthString, len 1..10000, pattern ^[\P{M}\p{M}]{1,10000}$) **required**
//!   — The text to translate. The text string can be a maximum of 10,000 bytes long.
//!   Depending on your character set, this may be fewer than 10,000 characters.
//!
//! ## Output
//!
//! * `AppliedSettings` (TranslationSettings) — Optional settings that modify the translation
//!   output.
//! * `AppliedTerminologies` (AppliedTerminologyList) — The names of the custom terminologies
//!   applied to the input text by Amazon Translate for the translated text response.
//! * `SourceLanguageCode` (LanguageCodeString, len 2..5) **required** — The language code for
//!   the language of the source text.
//! * `TargetLanguageCode` (LanguageCodeString, len 2..5) **required** — The language code for
//!   the language of the target text.
//! * `TranslatedText` (TranslatedTextString, len 0..20000, pattern ^[\P{M}\p{M}]{0,20000}$)
//!   **required** — The translated text.
//!
//! ## Errors
//!
//! * `DetectedLanguageLowConfidenceException` (HTTP 400) — The confidence that Amazon
//!   Comprehend accurately detected the source language is low. If a low confidence level is
//!   acceptable for your application, you can use the language in the exception to call
//!   Amazon Translate again. For more information, see the DetectDominantLanguage operation
//!   in the Amazon Comprehend Developer Guide.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `InvalidRequestException` (HTTP 400) — The request that you made is not valid. Check
//!   your request to determine why it's not valid and then retry the request.
//! * `ResourceNotFoundException` (HTTP 404) — The resource you are looking for has not been
//!   found. Review the resource you're looking for and see if a different resource will
//!   accomplish your needs before retrying the revised request.
//! * `ServiceUnavailableException` (HTTP 503) — The Amazon Translate service is temporarily
//!   unavailable. Wait a bit and then retry your request.
//! * `TextSizeLimitExceededException` (HTTP 400) — The size of the text you submitted exceeds
//!   the size limit. Reduce the size of the text or use a smaller document and then retry
//!   your request.
//! * `TooManyRequestsException` (HTTP 429) — You have made too many requests within a short
//!   period of time. Wait for a short time and then try your request again.
//! * `UnsupportedLanguagePairException` (HTTP 400) — Amazon Translate does not support
//!   translation from the language of the source text into the requested target language. For
//!   more information, see Supported languages.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{
    applied_term, invalid_request, is_supported_language, language_for_detector_code,
    resolve_terminologies, service_state, substitute, terminology_not_found,
    text_size_limit_exceeded, translation_marker, unsupported_language_pair, AUTO_LANGUAGE_CODE,
    MAX_TERMINOLOGIES_PER_TRANSLATION, MAX_TEXT_BYTES, MAX_TRANSLATED_TEXT_CHARS,
    TERMINOLOGY_RESOURCE,
};

/// The source language of `text`, for a request that asked for `auto`.
///
/// The detection is the `whatlang` crate's — a statistical identifier over the
/// text's script and trigrams — so the resolved code is computed from the text,
/// and the same text always resolves the same way. Text the detector cannot
/// place at all is the service's documented low-confidence failure; a language
/// it places but Amazon Translate does not translate is an unsupported pair.
fn detect_source_language(
    text: &str,
    target_language_code: &str,
) -> Result<String, TranslateTextError> {
    let detected = ::whatlang::detect_lang(text).ok_or_else(|| {
        TranslateTextErrorModeled::DetectedLanguageLowConfidenceException(
            DetectedLanguageLowConfidenceException {
                message: Some(
                    "The confidence that the source language was accurately detected is low."
                        .to_string(),
                ),
                detected_language_code: None,
            },
        )
    })?;
    language_for_detector_code(detected.code())
        .map(ToString::to_string)
        .ok_or_else(|| {
            // The text is in a language this service does not translate, so
            // there is no supported code to report the source as: the pair the
            // request asked for is what it is refused by.
            TranslateTextErrorModeled::UnsupportedLanguagePairException(
                unsupported_language_pair(AUTO_LANGUAGE_CODE, target_language_code),
            )
            .into()
        })
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TranslateTextError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TranslateTextError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.translate#TranslateText")]
#[derive(Debug)]
pub enum TranslateTextErrorModeled {
    DetectedLanguageLowConfidenceException(DetectedLanguageLowConfidenceException),
    InternalServerException(InternalServerException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
    TextSizeLimitExceededException(TextSizeLimitExceededException),
    TooManyRequestsException(TooManyRequestsException),
    UnsupportedLanguagePairException(UnsupportedLanguagePairException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::translate_text::TranslateTextError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TranslateTextError = crate::scaffolded::Scaffolded<TranslateTextErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TranslateTextErrorModeled> for TranslateTextError {
    fn from(modeled: TranslateTextErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `TranslateText` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TranslateText<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TranslateText<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TranslateText<Eng>
{
    type Input = TranslateTextInput;
    type Output = TranslateTextOutput;
    type Error = TranslateTextError;

    const NAME: &'static str = "TranslateText";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TRANSLATE_TEXT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TRANSLATE_TEXT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = service_state(&*self.engine).ok_or_else(|| {
            TranslateTextErrorModeled::InternalServerException(InternalServerException {
                message: Some("the terminology store is unavailable".to_string()),
            })
        })?;

        // The documented size limit counts the request text's BYTES, so it is
        // checked here rather than by the member's character-length constraint,
        // and it is checked first: the size of the request is decided before
        // anything in it is resolved.
        if input.text.len() > MAX_TEXT_BYTES {
            return Err(TranslateTextErrorModeled::TextSizeLimitExceededException(
                text_size_limit_exceeded(input.text.len()),
            )
            .into());
        }

        let requested_terminologies = input.terminology_names.unwrap_or_default();
        // "You can use one custom terminology resource in your translation
        // request."
        if requested_terminologies.len() > MAX_TERMINOLOGIES_PER_TRANSLATION {
            return Err(
                TranslateTextErrorModeled::InvalidRequestException(invalid_request()).into(),
            );
        }

        // `auto` asks the service to determine the source language from the
        // text itself, so the resolved code is computed from the text rather
        // than echoed back from the request.
        let source_language_code = if input.source_language_code == AUTO_LANGUAGE_CODE {
            detect_source_language(&input.text, &input.target_language_code)?
        } else {
            input.source_language_code.clone()
        };
        let target_language_code = input.target_language_code.clone();

        // Amazon Translate translates between the languages it supports, and
        // never from a language into itself.
        if source_language_code == target_language_code
            || !is_supported_language(&source_language_code)
            || !is_supported_language(&target_language_code)
        {
            return Err(TranslateTextErrorModeled::UnsupportedLanguagePairException(
                unsupported_language_pair(&source_language_code, &target_language_code),
            )
            .into());
        }

        let resolved = {
            let store = state.lock();
            resolve_terminologies(
                &store,
                &requested_terminologies,
                &source_language_code,
                &target_language_code,
            )
            .map_err(|name| {
                TranslateTextErrorModeled::ResourceNotFoundException(terminology_not_found(
                    TERMINOLOGY_RESOURCE,
                    &name,
                ))
            })?
        };

        // The marker is part of the answer, so the substitution's own ceiling
        // is what is left of the modelled output length once it is accounted
        // for.
        let marker = translation_marker(
            &source_language_code,
            &target_language_code,
            input.settings.as_ref(),
        );
        let budget = MAX_TRANSLATED_TEXT_CHARS.saturating_sub(marker.chars().count());

        // The scan below is bounded by that budget, but it is the only
        // caller-driven work this handler does: yield first, so a timeout or a
        // cancellation has a point to land on.
        ::tokio::task::yield_now().await;

        let mut substituted = input.text.clone();
        let mut applied_terminologies = Vec::new();
        for terminology in &resolved {
            let (text, applied, overflowed) =
                substitute(&substituted, &terminology.pairs, budget);
            if overflowed {
                // The substituted text no longer fits what the response can
                // carry, which is the service's own size refusal rather than a
                // truncated answer.
                return Err(TranslateTextErrorModeled::TextSizeLimitExceededException(
                    TextSizeLimitExceededException {
                        message: Some(
                            "The size of the text you submitted exceeds the size limit. \
                             Reduce the size of the text or use a smaller document and then \
                             retry your request."
                                .to_string(),
                        ),
                    },
                )
                .into());
            }
            substituted = text;
            if !applied.is_empty() {
                applied_terminologies.push(AppliedTerminology {
                    name: Some(terminology.name.clone()),
                    terms: Some(
                        applied
                            .into_iter()
                            .map(|(source, target)| applied_term(source, target))
                            .collect(),
                    ),
                });
            }
        }

        let translated_text = format!("{marker}{substituted}");

        Ok(TranslateTextOutput {
            translated_text,
            source_language_code,
            target_language_code,
            // Reported only when the request named a terminology that matched:
            // "This is optional in the response and will only be present if you
            // specified terminology input in the request."
            applied_terminologies: (!applied_terminologies.is_empty())
                .then_some(applied_terminologies),
            applied_settings: input.settings,
        })
    }
}
