// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-polly-parrot-v1 service.
//!
//! Two stores live here, and every handler reaches them through [`service_state`]:
//!
//! * [`ComAmazonawsPollyParrotV1State::lexicons`] — the pronunciation lexicons
//!   `PutLexicon` writes and `GetLexicon`, `ListLexicons`, `DeleteLexicon` and
//!   `SynthesizeSpeech` read. Keyed by lexicon name in a `BTreeMap`, so
//!   `ListLexicons` is ordered and paginable without sorting on every call.
//! * [`ComAmazonawsPollyParrotV1State::voices`] — the voice catalog `DescribeVoices`
//!   serves. Seeded once from [`VOICE_CATALOG`] when the state is first touched and
//!   never written at runtime; Amazon Polly models no operation that mutates it.

use std::collections::BTreeMap;
use std::time::SystemTime;

/// A pronunciation lexicon exactly as `PutLexicon` stored it, plus the attributes
/// derived from its PLS document at store time.
///
/// `content` is kept verbatim so `GetLexicon` can return the bytes the caller put,
/// character for character — the derived attributes are a *summary* of it, never a
/// replacement for it.
#[derive(Debug, Clone)]
pub struct StoredLexicon {
    /// The lexicon's name (the `@httpLabel` of every lexicon operation).
    pub name: String,
    /// The PLS document, byte-for-byte as `PutLexicon` received it.
    pub content: String,
    /// The PLS `alphabet` attribute, when the document carries one.
    pub alphabet: Option<String>,
    /// The PLS `xml:lang` attribute, when the document carries one.
    pub language_code: Option<String>,
    /// When this lexicon was last written by `PutLexicon`.
    pub last_modified: SystemTime,
    /// `arn:aws:polly:<region>:<account>:lexicon/<name>`.
    pub lexicon_arn: String,
    /// Number of `<lexeme>` elements in `content`.
    pub lexemes_count: i32,
    /// Size of the lexicon in characters, which is what the model's
    /// `LexiconAttributes$Size` documents ("Total size of the lexicon, in
    /// characters").
    pub size: i32,
}

/// One entry of the seeded voice catalog `DescribeVoices` serves.
///
/// Every field is `&'static` because the catalog is a compile-time constant: the
/// service has no operation that creates or edits a voice.
#[derive(Debug, Clone, Copy)]
pub struct StoredVoice {
    /// The `VoiceId` enum value, e.g. `"Joanna"`.
    pub id: &'static str,
    /// The human-readable voice name. Amazon Polly uses the voice id as its name.
    pub name: &'static str,
    /// `"Female"` or `"Male"` — the `Gender` enum.
    pub gender: &'static str,
    /// The voice's default language, a `LanguageCode` enum value.
    pub language_code: &'static str,
    /// The language's name in English (`Voice$LanguageName`).
    pub language_name: &'static str,
    /// Extra languages a bilingual voice speaks (`Voice$AdditionalLanguageCodes`).
    pub additional_language_codes: &'static [&'static str],
    /// The `Engine` values this voice supports (`Voice$SupportedEngines`).
    pub supported_engines: &'static [&'static str],
}

impl StoredVoice {
    /// Whether this voice speaks `language`, counting its additional languages only
    /// when `include_additional` is set — the rule
    /// `DescribeVoices$IncludeAdditionalLanguageCodes` documents.
    pub fn speaks(&self, language: &str, include_additional: bool) -> bool {
        self.language_code == language
            || (include_additional && self.additional_language_codes.contains(&language))
    }

    /// Whether this voice is available on `engine`.
    pub fn supports_engine(&self, engine: &str) -> bool {
        self.supported_engines.contains(&engine)
    }
}

/// Per-service state for com-amazonaws-polly-parrot-v1. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsPollyParrotV1State {
    /// Stored lexicons by name. Ordered, so `ListLexicons` pages deterministically.
    pub lexicons: BTreeMap<String, StoredLexicon>,
    /// The seeded voice catalog, in [`VOICE_CATALOG`] order.
    pub voices: Vec<StoredVoice>,
}

impl Default for ComAmazonawsPollyParrotV1State {
    fn default() -> Self {
        Self {
            lexicons: BTreeMap::new(),
            // Seeded on first touch: `DescribeVoices` reports a catalog, and a
            // catalog that only appears after some other call would report an
            // empty voice list to the first caller.
            voices: VOICE_CATALOG.to_vec(),
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPollyParrotV1State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPollyParrotV1State>)
}

/// The region an ARN this service mints is scoped to, read from the request's SigV4
/// credential scope.
///
/// Falls back to [`DEFAULT_REGION`] for an unsigned request, which carries no scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request's ARNs are scoped to — the region the emulator's
/// generated client configuration uses.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `LexiconAttributes` wire shape for a stored lexicon.
///
/// Shared by `GetLexicon` and `ListLexicons` so the two cannot report different
/// attributes for one stored lexicon.
pub fn lexicon_attributes(stored: &StoredLexicon) -> crate::ops::types::LexiconAttributes {
    crate::ops::types::LexiconAttributes {
        alphabet: stored.alphabet.clone(),
        language_code: stored.language_code.clone(),
        last_modified: Some(::aws_smithy_types::DateTime::from(stored.last_modified)),
        lexicon_arn: Some(stored.lexicon_arn.clone()),
        lexemes_count: stored.lexemes_count,
        size: stored.size,
    }
}

// ---------------------------------------------------------------------------
// Pagination tokens
// ---------------------------------------------------------------------------

/// Encode a page cursor as an opaque `NextToken`.
///
/// Hex rather than the bare cursor so the token is opaque to a caller (it must be
/// round-tripped, not constructed) and so a corrupted one is *detectable*:
/// [`decode_page_token`] rejects anything that is not hex of UTF-8, which is what
/// lets the paginated reads answer `InvalidNextTokenException` instead of silently
/// restarting from the first page.
pub fn encode_page_token(cursor: &str) -> String {
    ::hex::encode(cursor)
}

/// Decode a `NextToken` produced by [`encode_page_token`]; `None` for any token
/// this service did not mint.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

// ---------------------------------------------------------------------------
// The seeded voice catalog
// ---------------------------------------------------------------------------

/// The voices `DescribeVoices` reports.
///
/// Every `id` is a value of the model's `VoiceId` enum
/// (`smithy_query(query="shape", shape="VoiceId")`), every `language_code` a value of
/// its `LanguageCode` enum, every `gender` a value of `Gender` and every entry of
/// `supported_engines` a value of `Engine`; the language names and the
/// voice→language→engine assignments come from the AWS "Available voices" table
/// linked from `SynthesizeSpeech$Engine`'s documentation. `Voice$Name` is the voice
/// id, which is how Amazon Polly names a voice.
///
/// `Aditi` carries `hi-IN` as an additional language code because
/// `Voice$AdditionalLanguageCodes`'s own documentation states it: "the default
/// language for Aditi is Indian English (en-IN) ... this parameter would show the
/// code `hi-IN`".
///
/// This is a subset of Amazon Polly's full voice list, not a fabrication of it: a
/// voice named here is a real one with its real language and engines. Adding the
/// remaining voices is additive and changes no behaviour around it.
pub const VOICE_CATALOG: &[StoredVoice] = &[
    // ---- English ----
    voice("Danielle", "Female", "en-US", "US English", &[], &["neural", "generative", "long-form"]),
    voice("Gregory", "Male", "en-US", "US English", &[], &["neural", "long-form"]),
    voice("Ivy", "Female", "en-US", "US English", &[], &["neural", "standard"]),
    voice("Joanna", "Female", "en-US", "US English", &[], &["neural", "standard", "generative"]),
    voice("Joey", "Male", "en-US", "US English", &[], &["neural", "standard"]),
    voice("Justin", "Male", "en-US", "US English", &[], &["neural", "standard"]),
    voice("Kendra", "Female", "en-US", "US English", &[], &["neural", "standard"]),
    voice("Kevin", "Male", "en-US", "US English", &[], &["neural"]),
    voice("Kimberly", "Female", "en-US", "US English", &[], &["neural", "standard"]),
    voice("Matthew", "Male", "en-US", "US English", &[], &["neural", "standard", "generative"]),
    voice("Ruth", "Female", "en-US", "US English", &[], &["neural", "generative", "long-form"]),
    voice("Salli", "Female", "en-US", "US English", &[], &["neural", "standard"]),
    voice("Stephen", "Male", "en-US", "US English", &[], &["neural", "generative"]),
    voice("Amy", "Female", "en-GB", "British English", &[], &["neural", "standard", "generative"]),
    voice("Arthur", "Male", "en-GB", "British English", &[], &["neural"]),
    voice("Brian", "Male", "en-GB", "British English", &[], &["neural", "standard"]),
    voice("Emma", "Female", "en-GB", "British English", &[], &["neural", "standard"]),
    voice("Geraint", "Male", "en-GB-WLS", "Welsh English", &[], &["standard"]),
    voice("Nicole", "Female", "en-AU", "Australian English", &[], &["standard"]),
    voice("Olivia", "Female", "en-AU", "Australian English", &[], &["neural", "generative"]),
    voice("Russell", "Male", "en-AU", "Australian English", &[], &["standard"]),
    voice("Aditi", "Female", "en-IN", "Indian English", &["hi-IN"], &["standard"]),
    voice("Raveena", "Female", "en-IN", "Indian English", &[], &["standard"]),
    voice("Aria", "Female", "en-NZ", "New Zealand English", &[], &["neural", "generative"]),
    voice("Ayanda", "Female", "en-ZA", "South African English", &[], &["neural"]),
    voice("Niamh", "Female", "en-IE", "Irish English", &[], &["neural"]),
    voice("Jasmine", "Female", "en-SG", "Singaporean English", &[], &["neural"]),
    // ---- Hindi ----
    voice("Kajal", "Female", "hi-IN", "Hindi", &["en-IN"], &["neural", "generative"]),
    // ---- German ----
    voice("Daniel", "Male", "de-DE", "German", &[], &["neural", "generative"]),
    voice("Hans", "Male", "de-DE", "German", &[], &["standard"]),
    voice("Marlene", "Female", "de-DE", "German", &[], &["standard"]),
    voice("Vicki", "Female", "de-DE", "German", &[], &["neural", "standard", "generative"]),
    voice("Hannah", "Female", "de-AT", "Austrian German", &[], &["neural"]),
    // ---- French ----
    voice("Celine", "Female", "fr-FR", "French", &[], &["standard"]),
    voice("Lea", "Female", "fr-FR", "French", &[], &["neural", "standard"]),
    voice("Mathieu", "Male", "fr-FR", "French", &[], &["standard"]),
    voice("Remi", "Male", "fr-FR", "French", &[], &["neural", "generative"]),
    voice("Chantal", "Female", "fr-CA", "Canadian French", &[], &["standard"]),
    voice("Gabrielle", "Female", "fr-CA", "Canadian French", &[], &["neural"]),
    voice("Liam", "Male", "fr-CA", "Canadian French", &[], &["neural"]),
    voice("Isabelle", "Female", "fr-BE", "Belgian French", &[], &["neural"]),
    // ---- Spanish ----
    voice("Conchita", "Female", "es-ES", "Castilian Spanish", &[], &["standard"]),
    voice("Enrique", "Male", "es-ES", "Castilian Spanish", &[], &["standard"]),
    voice("Lucia", "Female", "es-ES", "Castilian Spanish", &[], &["neural", "standard"]),
    voice("Sergio", "Male", "es-ES", "Castilian Spanish", &[], &["neural", "generative"]),
    voice("Mia", "Female", "es-MX", "Mexican Spanish", &[], &["neural", "standard"]),
    voice("Lupe", "Female", "es-US", "US Spanish", &[], &["neural", "standard", "generative"]),
    voice("Miguel", "Male", "es-US", "US Spanish", &[], &["standard"]),
    voice("Pedro", "Male", "es-US", "US Spanish", &[], &["neural", "generative"]),
    voice("Penelope", "Female", "es-US", "US Spanish", &[], &["standard"]),
    // ---- Italian ----
    voice("Adriano", "Male", "it-IT", "Italian", &[], &["neural", "generative"]),
    voice("Bianca", "Female", "it-IT", "Italian", &[], &["neural", "standard", "generative"]),
    voice("Carla", "Female", "it-IT", "Italian", &[], &["standard"]),
    voice("Giorgio", "Male", "it-IT", "Italian", &[], &["standard"]),
    // ---- Portuguese ----
    voice("Camila", "Female", "pt-BR", "Brazilian Portuguese", &[], &["neural", "standard", "generative"]),
    voice("Ricardo", "Male", "pt-BR", "Brazilian Portuguese", &[], &["standard"]),
    voice("Thiago", "Male", "pt-BR", "Brazilian Portuguese", &[], &["neural"]),
    voice("Vitoria", "Female", "pt-BR", "Brazilian Portuguese", &[], &["neural", "standard"]),
    voice("Cristiano", "Male", "pt-PT", "Portuguese", &[], &["standard"]),
    voice("Ines", "Female", "pt-PT", "Portuguese", &[], &["neural", "standard"]),
    // ---- East Asian ----
    voice("Kazuha", "Female", "ja-JP", "Japanese", &[], &["neural"]),
    voice("Mizuki", "Female", "ja-JP", "Japanese", &[], &["standard"]),
    voice("Takumi", "Male", "ja-JP", "Japanese", &[], &["neural", "standard"]),
    voice("Tomoko", "Female", "ja-JP", "Japanese", &[], &["neural"]),
    voice("Jihye", "Female", "ko-KR", "Korean", &[], &["neural"]),
    voice("Seoyeon", "Female", "ko-KR", "Korean", &[], &["neural", "standard"]),
    voice("Zhiyu", "Female", "cmn-CN", "Chinese Mandarin", &[], &["neural", "standard"]),
    voice("Hiujin", "Female", "yue-CN", "Cantonese", &[], &["neural"]),
    // ---- Arabic ----
    voice("Zeina", "Female", "arb", "Arabic", &[], &["standard"]),
    voice("Hala", "Female", "ar-AE", "Gulf Arabic", &["arb"], &["neural"]),
    voice("Zayd", "Male", "ar-AE", "Gulf Arabic", &[], &["neural"]),
    // ---- Nordic ----
    voice("Astrid", "Female", "sv-SE", "Swedish", &[], &["standard"]),
    voice("Elin", "Female", "sv-SE", "Swedish", &[], &["neural"]),
    voice("Mads", "Male", "da-DK", "Danish", &[], &["standard"]),
    voice("Naja", "Female", "da-DK", "Danish", &[], &["standard"]),
    voice("Sofie", "Female", "da-DK", "Danish", &[], &["neural"]),
    voice("Ida", "Female", "nb-NO", "Norwegian", &[], &["neural"]),
    voice("Liv", "Female", "nb-NO", "Norwegian", &[], &["standard"]),
    voice("Suvi", "Female", "fi-FI", "Finnish", &[], &["neural"]),
    voice("Dora", "Female", "is-IS", "Icelandic", &[], &["standard"]),
    voice("Karl", "Male", "is-IS", "Icelandic", &[], &["standard"]),
    // ---- Other European ----
    voice("Laura", "Female", "nl-NL", "Dutch", &[], &["neural"]),
    voice("Lotte", "Female", "nl-NL", "Dutch", &[], &["standard"]),
    voice("Ruben", "Male", "nl-NL", "Dutch", &[], &["standard"]),
    voice("Lisa", "Female", "nl-BE", "Belgian Dutch", &[], &["neural"]),
    voice("Ewa", "Female", "pl-PL", "Polish", &[], &["standard"]),
    voice("Jacek", "Male", "pl-PL", "Polish", &[], &["standard"]),
    voice("Jan", "Male", "pl-PL", "Polish", &[], &["standard"]),
    voice("Maja", "Female", "pl-PL", "Polish", &[], &["standard"]),
    voice("Ola", "Female", "pl-PL", "Polish", &[], &["neural"]),
    voice("Jitka", "Female", "cs-CZ", "Czech", &[], &["neural"]),
    voice("Carmen", "Female", "ro-RO", "Romanian", &[], &["standard"]),
    voice("Arlet", "Female", "ca-ES", "Catalan", &[], &["neural"]),
    voice("Gwyneth", "Female", "cy-GB", "Welsh", &[], &["standard"]),
    voice("Burcu", "Female", "tr-TR", "Turkish", &[], &["neural"]),
    voice("Filiz", "Female", "tr-TR", "Turkish", &[], &["standard"]),
    voice("Maxim", "Male", "ru-RU", "Russian", &[], &["standard"]),
    voice("Tatyana", "Female", "ru-RU", "Russian", &[], &["standard"]),
];

/// One catalog row. `const fn` so [`VOICE_CATALOG`] stays a compile-time constant
/// and the table above reads as data rather than as struct literals.
const fn voice(
    id: &'static str,
    gender: &'static str,
    language_code: &'static str,
    language_name: &'static str,
    additional_language_codes: &'static [&'static str],
    supported_engines: &'static [&'static str],
) -> StoredVoice {
    StoredVoice {
        id,
        // Amazon Polly's `Voice$Name` is the voice id.
        name: id,
        gender,
        language_code,
        language_name,
        additional_language_codes,
        supported_engines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every catalog row must name values the model declares, or `DescribeVoices`
    /// would serve a voice no SDK can deserialize.
    #[test]
    fn every_seeded_voice_uses_modeled_enum_values() {
        use crate::ops::types::{ENGINE_VALUES, GENDER_VALUES, LANGUAGE_CODE_VALUES, VOICE_ID_VALUES};
        for voice in VOICE_CATALOG {
            assert!(
                VOICE_ID_VALUES.contains(&voice.id),
                "voice {:?}: id is not a modeled VoiceId",
                voice.id
            );
            assert!(
                GENDER_VALUES.contains(&voice.gender),
                "voice {:?}: gender {:?} is not a modeled Gender",
                voice.id,
                voice.gender
            );
            assert!(
                LANGUAGE_CODE_VALUES.contains(&voice.language_code),
                "voice {:?}: language code {:?} is not a modeled LanguageCode",
                voice.id,
                voice.language_code
            );
            for code in voice.additional_language_codes {
                assert!(
                    LANGUAGE_CODE_VALUES.contains(code),
                    "voice {:?}: additional language code {code:?} is not a modeled LanguageCode",
                    voice.id
                );
            }
            assert!(
                !voice.supported_engines.is_empty(),
                "voice {:?}: no supported engines",
                voice.id
            );
            for engine in voice.supported_engines {
                assert!(
                    ENGINE_VALUES.contains(engine),
                    "voice {:?}: engine {engine:?} is not a modeled Engine",
                    voice.id
                );
            }
        }
    }

    /// A page token round-trips, and anything else is rejected rather than read as
    /// "start from the beginning".
    #[test]
    fn page_tokens_round_trip_and_reject_garbage() {
        for cursor in ["", "A", "MyLexicon", "zzzz"] {
            let token = encode_page_token(cursor);
            assert_eq!(
                decode_page_token(&token).as_deref(),
                Some(cursor),
                "cursor {cursor:?}: token {token:?} did not round-trip"
            );
        }
        // Not hex at all, odd-length hex, and hex of bytes that are not UTF-8.
        for garbage in ["not-hex", "abc", "ff", "fffe"] {
            assert!(
                decode_page_token(garbage).is_none(),
                "garbage token {garbage:?}: expected no cursor, got {:?}",
                decode_page_token(garbage)
            );
        }
    }
}
