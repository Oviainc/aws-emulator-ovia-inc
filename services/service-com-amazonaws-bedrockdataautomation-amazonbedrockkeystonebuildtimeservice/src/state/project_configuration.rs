// SPDX-License-Identifier: CC0-1.0

//! The EFFECTIVE configuration of a DataAutomationProject: what a project read
//! reports, as opposed to what the create or the update spelled out.
//!
//! RULE. Reading a project reports the configuration the project will actually be
//! processed under: every configuration block the modelled shape carries is PRESENT,
//! each member taking the value the create or the update specified and the service's
//! own default wherever that specification was silent. A project created with a
//! `standardOutputConfiguration` naming only one modality is still processed with a
//! setting for each of the other three, with an override configuration, and with a
//! (possibly empty) custom-output and library binding — so a read that omitted them
//! would let a client that diffs the read against its desired state see every default
//! as drift.
//!
//! WHERE THE SHAPES COME FROM. `smithy_query(query="shape",
//! shape="StandardOutputConfiguration,OverrideConfiguration,CustomOutputConfiguration,
//! DataAutomationLibraryConfiguration",
//! model_path="services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/smithy-model/com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.json")`:
//! `StandardOutputConfiguration` carries `document`, `image`, `video` and `audio`;
//! `OverrideConfiguration` carries `document`, `image`, `video`, `audio` and
//! `modalityRouting`; the other two are the blueprint and library bindings. Every
//! enum value a default below names is a value of its own modelled enum — the
//! catalogues are in `crate::ops::types` (`STATE_VALUES`,
//! `DOCUMENT_EXTRACTION_GRANULARITY_TYPE_VALUES`,
//! `DOCUMENT_OUTPUT_TEXT_FORMAT_TYPE_VALUES`, `IMAGE_EXTRACTION_CATEGORY_TYPE_VALUES`,
//! `IMAGE_STANDARD_GENERATIVE_FIELD_TYPE_VALUES`,
//! `VIDEO_EXTRACTION_CATEGORY_TYPE_VALUES`,
//! `VIDEO_STANDARD_GENERATIVE_FIELD_TYPE_VALUES`,
//! `AUDIO_EXTRACTION_CATEGORY_TYPE_VALUES`,
//! `AUDIO_GENERATIVE_OUTPUT_LANGUAGE_VALUES`), and each constant below says which.
//!
//! WHICH value is the default is NOT in the model — no member of these shapes carries
//! `@default` — it is Bedrock Data Automation's own standard-output default for a
//! modality a project leaves unspecified. Each constant records the setting it stands
//! for so a reader can see what a defaulted project emits: text extraction per page
//! and per element in Markdown with no bounding boxes and no summaries for a document,
//! text detection with bounding boxes and a summary for an image or a video, a
//! transcript without channel or speaker labels for audio, and splitting off.
//!
//! The merge is a pure projection: it reads the stored record and writes nothing, so
//! two readers at the same instant agree and a read leaves the store exactly as it
//! found it. The STORED configuration stays the one the caller sent — the defaults are
//! materialised on the way out, never folded into the record — so a later update still
//! replaces exactly what the caller specified.

use crate::ops::types::{
    AudioExtractionCategory, AudioExtractionCategoryTypeConfiguration, AudioLanguageConfiguration,
    AudioOverrideConfiguration, AudioStandardExtraction, AudioStandardGenerativeField,
    AudioStandardOutputConfiguration, ChannelLabelingConfiguration, CustomOutputConfiguration,
    DataAutomationLibraryConfiguration, DocumentBoundingBox, DocumentExtractionGranularity,
    DocumentOutputAdditionalFileFormat, DocumentOutputFormat, DocumentOutputTextFormat,
    DocumentOverrideConfiguration, DocumentStandardExtraction, DocumentStandardGenerativeField,
    DocumentStandardOutputConfiguration, ImageBoundingBox, ImageExtractionCategory,
    ImageStandardExtraction, ImageStandardGenerativeField,
    ImageStandardOutputConfiguration, OverrideConfiguration, SpeakerLabelingConfiguration,
    SplitterConfiguration, StandardOutputConfiguration, TranscriptConfiguration,
    VideoBoundingBox, VideoExtractionCategory, VideoStandardExtraction,
    VideoStandardGenerativeField, VideoStandardOutputConfiguration,
};

/// The `State` value meaning a setting is on. One of
/// `smithy_query(shape="State")` -> `["DISABLED", "ENABLED"]`
/// (`crate::ops::types::STATE_VALUES`).
pub const STATE_ENABLED: &str = "ENABLED";

/// The `State` value meaning a setting is off. See [`STATE_ENABLED`].
pub const STATE_DISABLED: &str = "DISABLED";

/// The granularities a defaulted document extraction reports, from
/// `smithy_query(shape="DocumentExtractionGranularityType")` ->
/// `["DOCUMENT", "ELEMENT", "LINE", "PAGE", "WORD"]`: a defaulted project extracts a
/// document per PAGE and per ELEMENT.
pub const DEFAULT_DOCUMENT_GRANULARITY_TYPES: [&str; 2] = ["PAGE", "ELEMENT"];

/// The text format a defaulted document output reports, from
/// `smithy_query(shape="DocumentOutputTextFormatType")` ->
/// `["CSV", "HTML", "MARKDOWN", "PLAIN_TEXT"]`.
pub const DEFAULT_DOCUMENT_TEXT_FORMAT_TYPES: [&str; 1] = ["MARKDOWN"];

/// The extraction category a defaulted image or video reports, from
/// `smithy_query(shape="ImageExtractionCategoryType")` ->
/// `["CONTENT_MODERATION", "LOGOS", "TEXT_DETECTION"]` and
/// `smithy_query(shape="VideoExtractionCategoryType")` -> the same three plus
/// `TRANSCRIPT`: a defaulted project detects text in both.
pub const DEFAULT_VISUAL_CATEGORY_TYPES: [&str; 1] = ["TEXT_DETECTION"];

/// The generative fields a defaulted image reports, from
/// `smithy_query(shape="ImageStandardGenerativeFieldType")` ->
/// `["IAB", "IMAGE_SUMMARY"]`.
pub const DEFAULT_IMAGE_GENERATIVE_FIELD_TYPES: [&str; 1] = ["IMAGE_SUMMARY"];

/// The generative fields a defaulted video reports, from
/// `smithy_query(shape="VideoStandardGenerativeFieldType")` ->
/// `["CHAPTER_SUMMARY", "IAB", "VIDEO_SUMMARY"]`.
pub const DEFAULT_VIDEO_GENERATIVE_FIELD_TYPES: [&str; 2] = ["VIDEO_SUMMARY", "CHAPTER_SUMMARY"];

/// The extraction categories a defaulted audio modality reports, from
/// `smithy_query(shape="AudioExtractionCategoryType")` ->
/// `["AUDIO_CONTENT_MODERATION", "TOPIC_CONTENT_MODERATION", "TRANSCRIPT"]`: a
/// defaulted project transcribes audio.
pub const DEFAULT_AUDIO_CATEGORY_TYPES: [&str; 1] = ["TRANSCRIPT"];

/// The generative output language a defaulted audio override reports, from
/// `smithy_query(shape="AudioGenerativeOutputLanguage")` -> `["DEFAULT", "EN"]`.
pub const DEFAULT_AUDIO_GENERATIVE_OUTPUT_LANGUAGE: &str = "DEFAULT";

/// Whether a defaulted audio override identifies several spoken languages at once.
pub const DEFAULT_IDENTIFY_MULTIPLE_LANGUAGES: bool = false;

/// Owned copies of a static value list, which is how every `types` member of these
/// shapes is modelled (`Option<Vec<String>>`).
fn types(values: &[&str]) -> Option<Vec<String>> {
    Some(values.iter().map(|value| (*value).to_string()).collect())
}

/// A modelled `@required` enum member as the record holds it, or `fallback` when it
/// holds no value at all. Every such member reaches the store through the validating
/// request walk, so the fallback is reached only by a record built in-process.
fn state_or(configured: &str, fallback: &str) -> String {
    if configured.is_empty() {
        fallback.to_string()
    } else {
        configured.to_string()
    }
}

/// The standard output configuration a project reports: every modality present, each
/// one the project configured taking its configured values and each one it left
/// unspecified taking the service's default.
pub fn effective_standard_output_configuration(
    configured: Option<&StandardOutputConfiguration>,
) -> StandardOutputConfiguration {
    StandardOutputConfiguration {
        document: Some(effective_document(
            configured.and_then(|c| c.document.as_ref()),
        )),
        image: Some(effective_image(configured.and_then(|c| c.image.as_ref()))),
        video: Some(effective_video(configured.and_then(|c| c.video.as_ref()))),
        audio: Some(effective_audio(configured.and_then(|c| c.audio.as_ref()))),
    }
}

/// The document modality: extraction per page and element without bounding boxes, no
/// generated summary, Markdown text and no additional file format.
fn effective_document(
    configured: Option<&DocumentStandardOutputConfiguration>,
) -> DocumentStandardOutputConfiguration {
    DocumentStandardOutputConfiguration {
        extraction: Some(match configured.and_then(|c| c.extraction.as_ref()) {
            Some(extraction) => DocumentStandardExtraction {
                granularity: DocumentExtractionGranularity {
                    types: extraction
                        .granularity
                        .types
                        .clone()
                        .or_else(|| types(&DEFAULT_DOCUMENT_GRANULARITY_TYPES)),
                },
                bounding_box: DocumentBoundingBox {
                    state: state_or(&extraction.bounding_box.state, STATE_DISABLED),
                },
            },
            None => DocumentStandardExtraction {
                granularity: DocumentExtractionGranularity {
                    types: types(&DEFAULT_DOCUMENT_GRANULARITY_TYPES),
                },
                bounding_box: DocumentBoundingBox {
                    state: STATE_DISABLED.to_string(),
                },
            },
        }),
        generative_field: Some(DocumentStandardGenerativeField {
            state: match configured.and_then(|c| c.generative_field.as_ref()) {
                Some(field) => state_or(&field.state, STATE_DISABLED),
                None => STATE_DISABLED.to_string(),
            },
        }),
        output_format: Some(match configured.and_then(|c| c.output_format.as_ref()) {
            Some(format) => DocumentOutputFormat {
                text_format: DocumentOutputTextFormat {
                    types: format
                        .text_format
                        .types
                        .clone()
                        .or_else(|| types(&DEFAULT_DOCUMENT_TEXT_FORMAT_TYPES)),
                },
                additional_file_format: DocumentOutputAdditionalFileFormat {
                    state: state_or(&format.additional_file_format.state, STATE_DISABLED),
                },
            },
            None => DocumentOutputFormat {
                text_format: DocumentOutputTextFormat {
                    types: types(&DEFAULT_DOCUMENT_TEXT_FORMAT_TYPES),
                },
                additional_file_format: DocumentOutputAdditionalFileFormat {
                    state: STATE_DISABLED.to_string(),
                },
            },
        }),
    }
}

/// The image modality: text detection with bounding boxes, and an image summary.
fn effective_image(
    configured: Option<&ImageStandardOutputConfiguration>,
) -> ImageStandardOutputConfiguration {
    ImageStandardOutputConfiguration {
        extraction: Some(match configured.and_then(|c| c.extraction.as_ref()) {
            Some(extraction) => ImageStandardExtraction {
                category: ImageExtractionCategory {
                    state: state_or(&extraction.category.state, STATE_ENABLED),
                    types: extraction
                        .category
                        .types
                        .clone()
                        .or_else(|| types(&DEFAULT_VISUAL_CATEGORY_TYPES)),
                },
                bounding_box: ImageBoundingBox {
                    state: state_or(&extraction.bounding_box.state, STATE_ENABLED),
                },
            },
            None => ImageStandardExtraction {
                category: ImageExtractionCategory {
                    state: STATE_ENABLED.to_string(),
                    types: types(&DEFAULT_VISUAL_CATEGORY_TYPES),
                },
                bounding_box: ImageBoundingBox {
                    state: STATE_ENABLED.to_string(),
                },
            },
        }),
        generative_field: Some(match configured.and_then(|c| c.generative_field.as_ref()) {
            Some(field) => ImageStandardGenerativeField {
                state: state_or(&field.state, STATE_ENABLED),
                types: field
                    .types
                    .clone()
                    .or_else(|| types(&DEFAULT_IMAGE_GENERATIVE_FIELD_TYPES)),
            },
            None => ImageStandardGenerativeField {
                state: STATE_ENABLED.to_string(),
                types: types(&DEFAULT_IMAGE_GENERATIVE_FIELD_TYPES),
            },
        }),
    }
}

/// The video modality: text detection with bounding boxes, and a video plus chapter
/// summary.
fn effective_video(
    configured: Option<&VideoStandardOutputConfiguration>,
) -> VideoStandardOutputConfiguration {
    VideoStandardOutputConfiguration {
        extraction: Some(match configured.and_then(|c| c.extraction.as_ref()) {
            Some(extraction) => VideoStandardExtraction {
                category: VideoExtractionCategory {
                    state: state_or(&extraction.category.state, STATE_ENABLED),
                    types: extraction
                        .category
                        .types
                        .clone()
                        .or_else(|| types(&DEFAULT_VISUAL_CATEGORY_TYPES)),
                },
                bounding_box: VideoBoundingBox {
                    state: state_or(&extraction.bounding_box.state, STATE_ENABLED),
                },
            },
            None => VideoStandardExtraction {
                category: VideoExtractionCategory {
                    state: STATE_ENABLED.to_string(),
                    types: types(&DEFAULT_VISUAL_CATEGORY_TYPES),
                },
                bounding_box: VideoBoundingBox {
                    state: STATE_ENABLED.to_string(),
                },
            },
        }),
        generative_field: Some(match configured.and_then(|c| c.generative_field.as_ref()) {
            Some(field) => VideoStandardGenerativeField {
                state: state_or(&field.state, STATE_ENABLED),
                types: field
                    .types
                    .clone()
                    .or_else(|| types(&DEFAULT_VIDEO_GENERATIVE_FIELD_TYPES)),
            },
            None => VideoStandardGenerativeField {
                state: STATE_ENABLED.to_string(),
                types: types(&DEFAULT_VIDEO_GENERATIVE_FIELD_TYPES),
            },
        }),
    }
}

/// The audio modality: a transcript with neither channel nor speaker labels, and no
/// generated summary.
fn effective_audio(
    configured: Option<&AudioStandardOutputConfiguration>,
) -> AudioStandardOutputConfiguration {
    AudioStandardOutputConfiguration {
        extraction: Some(match configured.and_then(|c| c.extraction.as_ref()) {
            Some(extraction) => AudioStandardExtraction {
                category: AudioExtractionCategory {
                    state: state_or(&extraction.category.state, STATE_ENABLED),
                    types: extraction
                        .category
                        .types
                        .clone()
                        .or_else(|| types(&DEFAULT_AUDIO_CATEGORY_TYPES)),
                    type_configuration: Some(effective_audio_type_configuration(
                        extraction.category.type_configuration.as_ref(),
                    )),
                },
            },
            None => AudioStandardExtraction {
                category: AudioExtractionCategory {
                    state: STATE_ENABLED.to_string(),
                    types: types(&DEFAULT_AUDIO_CATEGORY_TYPES),
                    type_configuration: Some(effective_audio_type_configuration(None)),
                },
            },
        }),
        generative_field: Some(AudioStandardGenerativeField {
            state: match configured.and_then(|c| c.generative_field.as_ref()) {
                Some(field) => state_or(&field.state, STATE_DISABLED),
                None => STATE_DISABLED.to_string(),
            },
            types: configured
                .and_then(|c| c.generative_field.as_ref())
                .and_then(|field| field.types.clone()),
        }),
    }
}

/// The transcript settings under an audio extraction category: speaker and channel
/// labelling both off unless the project turned one on.
fn effective_audio_type_configuration(
    configured: Option<&AudioExtractionCategoryTypeConfiguration>,
) -> AudioExtractionCategoryTypeConfiguration {
    let transcript = configured.and_then(|c| c.transcript.as_ref());
    AudioExtractionCategoryTypeConfiguration {
        transcript: Some(TranscriptConfiguration {
            speaker_labeling: Some(SpeakerLabelingConfiguration {
                state: match transcript.and_then(|t| t.speaker_labeling.as_ref()) {
                    Some(labeling) => state_or(&labeling.state, STATE_DISABLED),
                    None => STATE_DISABLED.to_string(),
                },
            }),
            channel_labeling: Some(ChannelLabelingConfiguration {
                state: match transcript.and_then(|t| t.channel_labeling.as_ref()) {
                    Some(labeling) => state_or(&labeling.state, STATE_DISABLED),
                    None => STATE_DISABLED.to_string(),
                },
            }),
        }),
    }
}

/// The override configuration a project reports: every modality present, document
/// splitting off and audio answering in the default language unless the project
/// overrode it. `modalityRouting` has no default — it names the modality a given file
/// extension is routed to, so there is nothing to report until a project sets one.
pub fn effective_override_configuration(
    configured: Option<&OverrideConfiguration>,
) -> OverrideConfiguration {
    OverrideConfiguration {
        document: Some(effective_document_override(
            configured.and_then(|c| c.document.as_ref()),
        )),
        image: Some(configured.and_then(|c| c.image.clone()).unwrap_or_default()),
        video: Some(configured.and_then(|c| c.video.clone()).unwrap_or_default()),
        audio: Some(effective_audio_override(
            configured.and_then(|c| c.audio.as_ref()),
        )),
        modality_routing: configured.and_then(|c| c.modality_routing.clone()),
    }
}

/// The document override: splitting off unless the project turned it on, and nothing
/// else reported until the project sets it.
fn effective_document_override(
    configured: Option<&DocumentOverrideConfiguration>,
) -> DocumentOverrideConfiguration {
    DocumentOverrideConfiguration {
        splitter: Some(SplitterConfiguration {
            state: Some(
                configured
                    .and_then(|c| c.splitter.as_ref())
                    .and_then(|splitter| splitter.state.clone())
                    .unwrap_or_else(|| STATE_DISABLED.to_string()),
            ),
        }),
        modality_processing: configured.and_then(|c| c.modality_processing.clone()),
        sensitive_data_configuration: configured
            .and_then(|c| c.sensitive_data_configuration.clone()),
    }
}

/// The audio override: the default generative output language and no multi-language
/// identification unless the project asked for them.
fn effective_audio_override(
    configured: Option<&AudioOverrideConfiguration>,
) -> AudioOverrideConfiguration {
    let language = configured.and_then(|c| c.language_configuration.as_ref());
    AudioOverrideConfiguration {
        modality_processing: configured.and_then(|c| c.modality_processing.clone()),
        language_configuration: Some(AudioLanguageConfiguration {
            input_languages: language.and_then(|l| l.input_languages.clone()),
            generative_output_language: Some(
                language
                    .and_then(|l| l.generative_output_language.clone())
                    .unwrap_or_else(|| DEFAULT_AUDIO_GENERATIVE_OUTPUT_LANGUAGE.to_string()),
            ),
            identify_multiple_languages: Some(
                language
                    .and_then(|l| l.identify_multiple_languages)
                    .unwrap_or(DEFAULT_IDENTIFY_MULTIPLE_LANGUAGES),
            ),
        }),
        sensitive_data_configuration: configured
            .and_then(|c| c.sensitive_data_configuration.clone()),
    }
}

/// The custom output configuration a project reports: the blueprint binding it was
/// created with, or the empty binding — a project binds no blueprint until one is
/// named, and the block is reported either way.
pub fn effective_custom_output_configuration(
    configured: Option<&CustomOutputConfiguration>,
) -> CustomOutputConfiguration {
    configured.cloned().unwrap_or_default()
}

/// The library configuration a project reports: the libraries it was created with, or
/// the empty binding. Same reasoning as [`effective_custom_output_configuration`].
pub fn effective_data_automation_library_configuration(
    configured: Option<&DataAutomationLibraryConfiguration>,
) -> DataAutomationLibraryConfiguration {
    configured.cloned().unwrap_or_default()
}
