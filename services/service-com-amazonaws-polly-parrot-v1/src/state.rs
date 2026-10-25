// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-polly-parrot-v1 service.
//!
//! Three stores live here, and every handler reaches them through [`service_state`]:
//!
//! * [`ComAmazonawsPollyParrotV1State::lexicons`] — the pronunciation lexicons
//!   `PutLexicon` writes and `GetLexicon`, `ListLexicons`, `DeleteLexicon` and
//!   `SynthesizeSpeech` read. Keyed by [`LexiconKey`] — the Region the lexicon was
//!   put in, then its name — in a `BTreeMap`, so one Region's lexicons sit
//!   contiguously in name order and `ListLexicons` is paginable over a range scan
//!   without sorting on every call. Reach them through the accessors on the state
//!   rather than through the map, so no operation can read past its own Region.
//! * [`ComAmazonawsPollyParrotV1State::voices`] — the voice catalog `DescribeVoices`
//!   serves. Seeded once from [`VOICE_CATALOG`] when the state is first touched and
//!   never written at runtime; Amazon Polly models no operation that mutates it.
//! * [`ComAmazonawsPollyParrotV1State::synthesis_tasks`] — the asynchronous speech
//!   synthesis tasks `StartSpeechSynthesisTask` creates and
//!   `GetSpeechSynthesisTask` and `ListSpeechSynthesisTasks` report. Keyed by
//!   `TaskId` in a `BTreeMap`, so the list read is ordered and paginable the same way.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::time::{Duration, SystemTime};

/// The key one stored lexicon sits under: the Region it was put in, then its name.
///
/// A lexicon is a REGIONAL resource, so its name alone does not identify it: the
/// model's `LexiconNotFoundException` documents the error as caused by "specifying a
/// lexicon that is in a different region", and `PutLexicon` as overwriting "a lexicon
/// with the same name" that "already exists in the region".
///
/// Region first and name second, so the derived `Ord` lays one Region's lexicons out
/// contiguously in name order: [`ComAmazonawsPollyParrotV1State::lexicons_in_region`]
/// is then a range scan rather than a filter over the whole store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LexiconKey {
    /// The Region the lexicon lives in, as [`request_region`] read it from the
    /// storing call's credential scope.
    pub region: String,
    /// The lexicon's name, unique within `region`.
    pub name: String,
}

impl LexiconKey {
    /// The key for `name` in `region`.
    pub fn new(region: &str, name: &str) -> Self {
        Self {
            region: region.to_string(),
            name: name.to_string(),
        }
    }
}

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
    /// The PLS `xml:lang` attribute. Every stored lexicon has one: a PLS document
    /// element that declares no language is not a lexicon this service stores, so
    /// there is no stored lexicon whose reported attributes omit `LanguageCode`.
    pub language_code: String,
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

/// A speech synthesis task, as the asynchronous synthesis operations keep it.
///
/// Every member of the wire `SynthesisTask` shape is stored, so the read operations
/// report the task the start call actually recorded rather than a shape rebuilt from
/// a later request. `task_status` is the task's status NOW, and
/// [`StoredSynthesisTask::pending_status`] is the one the clock owes it.
#[derive(Debug, Clone)]
pub struct StoredSynthesisTask {
    /// The service-generated `TaskId`, which is also this entry's key.
    pub task_id: String,
    /// The engine the task synthesises with.
    pub engine: String,
    /// The task's `OutputFormat`.
    pub output_format: String,
    /// Where the finished output is written (`SynthesisTask$OutputUri`).
    pub output_uri: String,
    /// "Timestamp for the time the synthesis task was started".
    pub creation_time: SystemTime,
    /// "Number of billable characters synthesized".
    pub request_characters: i32,
    /// The SNS topic the start call asked for status notifications on, if any.
    pub sns_topic_arn: Option<String>,
    /// The lexicons the task applies.
    pub lexicon_names: Vec<String>,
    /// The requested sample rate, as the request spelled it.
    pub sample_rate: Option<String>,
    /// The requested speech mark types.
    pub speech_mark_types: Vec<String>,
    /// `text` or `ssml`.
    pub text_type: String,
    /// The voice the task synthesises with.
    pub voice_id: String,
    /// The language code the start call named, for a bilingual voice.
    pub language_code: Option<String>,
    /// The task's status now — a value of the model's `TaskStatus` enum.
    pub task_status: String,
    /// "Reason for the current status of a specific speech synthesis task".
    pub task_status_reason: Option<String>,
    /// The status the CLOCK owes this task, and when it falls due. A task Amazon
    /// Polly returns in a non-terminal status reaches its terminal one on its own,
    /// without any further call, so the due instant is stored and
    /// [`settle_task_status`] materialises it when a read arrives after it.
    pub pending_status: Option<PendingTaskStatus>,
}

/// A status transition the clock owes a task.
#[derive(Debug, Clone)]
pub struct PendingTaskStatus {
    /// The instant from which this task is in `status`.
    pub due_at: SystemTime,
    /// The `TaskStatus` the task moves to.
    pub status: String,
    /// The `TaskStatusReason` that moves with it (a failure's reason).
    pub reason: Option<String>,
}

/// How long after the start call a synthesis task reaches its terminal status.
///
/// The emulator chooses this span: Amazon Polly's own synthesis time depends on the
/// text and the voice, and nothing here synthesises speech. It is deliberately NOT
/// zero, so a caller that polls immediately after starting a task still observes the
/// non-terminal status and the transition, and short enough that a test need not
/// wait on it.
pub const TASK_COMPLETION_DELAY: Duration = Duration::from_millis(500);

/// Materialise the status transition the clock has ALREADY made due on `task`.
///
/// A read calls this before reporting a task: it changes nothing a caller could not
/// have observed by waiting, because the transition was owed at `now` whether or not
/// anybody called. It never moves a status that is not yet due.
pub fn settle_task_status(task: &mut StoredSynthesisTask, now: SystemTime) {
    if task.pending_status.as_ref().is_some_and(|p| now >= p.due_at) {
        let pending = task
            .pending_status
            .take()
            .expect("the pending status was just observed to be due");
        task.task_status = pending.status;
        task.task_status_reason = pending.reason;
    }
}

/// The `SynthesisTask` wire shape for a stored task.
///
/// Shared by `GetSpeechSynthesisTask`, `ListSpeechSynthesisTasks` and
/// `StartSpeechSynthesisTask` so no two of them can report one task differently.
pub fn synthesis_task(stored: &StoredSynthesisTask) -> crate::ops::types::SynthesisTask {
    crate::ops::types::SynthesisTask {
        engine: Some(stored.engine.clone()),
        task_id: Some(stored.task_id.clone()),
        task_status: Some(stored.task_status.clone()),
        task_status_reason: stored.task_status_reason.clone(),
        output_uri: Some(stored.output_uri.clone()),
        creation_time: Some(::aws_smithy_types::DateTime::from(stored.creation_time)),
        request_characters: stored.request_characters,
        sns_topic_arn: stored.sns_topic_arn.clone(),
        lexicon_names: Some(stored.lexicon_names.clone()),
        output_format: Some(stored.output_format.clone()),
        sample_rate: stored.sample_rate.clone(),
        speech_mark_types: Some(stored.speech_mark_types.clone()),
        text_type: Some(stored.text_type.clone()),
        voice_id: Some(stored.voice_id.clone()),
        language_code: stored.language_code.clone(),
    }
}

/// Per-service state for com-amazonaws-polly-parrot-v1. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsPollyParrotV1State {
    /// Stored lexicons by Region and name. Ordered, so `ListLexicons` pages one
    /// Region deterministically.
    pub lexicons: BTreeMap<LexiconKey, StoredLexicon>,
    /// The seeded voice catalog, in [`VOICE_CATALOG`] order.
    pub voices: Vec<StoredVoice>,
    /// Speech synthesis tasks by `TaskId`. Ordered for the same reason as the
    /// lexicons: `ListSpeechSynthesisTasks` pages deterministically over it.
    pub synthesis_tasks: BTreeMap<String, StoredSynthesisTask>,
}

impl ComAmazonawsPollyParrotV1State {
    /// The lexicon `name` of `region`, or `None` when that Region stores none under
    /// that name — a lexicon of ANOTHER Region never answers here, which is what
    /// makes a cross-Region read the modelled "not found".
    pub fn lexicon(&self, region: &str, name: &str) -> Option<&StoredLexicon> {
        self.lexicons.get(&LexiconKey::new(region, name))
    }

    /// The lexicons `region` holds whose name sorts strictly after `after`, in name
    /// order; `after` is `None` for the whole Region.
    ///
    /// A range scan from the cursor, bounded by the Region: the keys of one Region
    /// are contiguous, so this reads exactly that Region's entries and stops at the
    /// first key belonging to another.
    pub fn lexicons_in_region<'a>(
        &'a self,
        region: &'a str,
        after: Option<&str>,
    ) -> impl Iterator<Item = &'a StoredLexicon> + 'a {
        // A lexicon name matches `^[0-9A-Za-z]{1,20}$`, so the empty name sorts
        // before every real one and excluding it excludes nothing.
        let start = LexiconKey::new(region, after.unwrap_or_default());
        self.lexicons
            .range((Bound::Excluded(start), Bound::Unbounded))
            .take_while(move |(key, _)| key.region == region)
            .map(|(_, stored)| stored)
    }

    /// How many lexicons `region` holds — what the per-Region count quota counts.
    pub fn lexicon_count_in_region(&self, region: &str) -> usize {
        self.lexicons_in_region(region, None).count()
    }

    /// Store `stored` in `region` under its own name, replacing whatever that Region
    /// held under that name and leaving every other Region untouched.
    pub fn store_lexicon(&mut self, region: &str, stored: StoredLexicon) {
        self.lexicons
            .insert(LexiconKey::new(region, &stored.name), stored);
    }

    /// Remove the lexicon `name` of `region`, answering `None` — and deleting
    /// nothing — when that Region holds none under that name.
    pub fn remove_lexicon(&mut self, region: &str, name: &str) -> Option<StoredLexicon> {
        self.lexicons.remove(&LexiconKey::new(region, name))
    }
}

impl Default for ComAmazonawsPollyParrotV1State {
    fn default() -> Self {
        Self {
            lexicons: BTreeMap::new(),
            // No task exists until a start call creates one.
            synthesis_tasks: BTreeMap::new(),
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

/// The region this request was signed for, read from its SigV4 credential scope: the
/// region the ARNs it mints are scoped to, and the region its resources live in.
///
/// Falls back to [`DEFAULT_REGION`] for an unsigned request, which carries no scope.
/// The scope is caller-controlled text that lands in an ARN and keys the lexicon
/// store, so anything that is not an AWS region token is refused here and the default
/// used instead: a minted ARN can never carry text a client cannot parse back, and a
/// store key stays a short token.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= MAX_REGION_CHARACTERS
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The most characters a credential scope's region may carry to be read as one.
///
/// Comfortably above every published AWS region name, and short enough that the
/// region cannot itself be the unbounded part of a store key.
pub const MAX_REGION_CHARACTERS: usize = 64;

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
        language_code: Some(stored.language_code.clone()),
        last_modified: Some(::aws_smithy_types::DateTime::from(stored.last_modified)),
        lexicon_arn: Some(stored.lexicon_arn.clone()),
        lexemes_count: stored.lexemes_count,
        size: stored.size,
    }
}

// ---------------------------------------------------------------------------
// Task identifiers
// ---------------------------------------------------------------------------

/// Length of a UUID written in the canonical hyphenated form (`8-4-4-4-12` hex
/// digits), which is the only form [`is_task_id`] accepts.
const HYPHENATED_UUID_LENGTH: usize = ::uuid::fmt::Hyphenated::LENGTH;

/// A fresh `TaskId`, in the form Amazon Polly mints one: a random UUID written in the
/// canonical hyphenated form.
///
/// Every `TaskId` the service hands out comes from here, so the identifiers the read
/// operations accept ([`is_task_id`]) are exactly the identifiers the service can have
/// minted.
pub fn mint_task_id() -> String {
    ::uuid::Uuid::new_v4().hyphenated().to_string()
}

/// Whether `task_id` is an identifier this service could have minted — that is, a UUID
/// in the canonical hyphenated form [`mint_task_id`] produces.
///
/// The model's `TaskId` pattern (`^[a-zA-Z0-9_-]{1,100}$`) is far wider than the
/// identifiers the service mints, so a value can satisfy the pattern — and therefore
/// reach a handler — while being no task identifier at all. That is the case the model
/// declares `InvalidTaskIdException` for, as distinct from an identifier of the right
/// form that simply names no stored task.
///
/// The length test is what pins the HYPHENATED form: a UUID parser also accepts the
/// 32-digit, braced and URN spellings, and this service mints none of those. The hex
/// digits are accepted in either case, since case is not what makes a value an
/// identifier.
pub fn is_task_id(task_id: &str) -> bool {
    task_id.len() == HYPHENATED_UUID_LENGTH && ::uuid::Uuid::try_parse(task_id).is_ok()
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

    /// A pending status is materialised exactly when the clock has made it due, and
    /// never before — polling early must not complete a task.
    #[test]
    fn a_task_status_moves_only_when_the_clock_makes_it_due() {
        let now = SystemTime::now();
        let task = |due: SystemTime| StoredSynthesisTask {
            task_id: "task".to_string(),
            engine: "standard".to_string(),
            output_format: "mp3".to_string(),
            output_uri: "s3://bucket/task.mp3".to_string(),
            creation_time: now,
            request_characters: 5,
            sns_topic_arn: None,
            lexicon_names: Vec::new(),
            sample_rate: None,
            speech_mark_types: Vec::new(),
            text_type: "text".to_string(),
            voice_id: "Joanna".to_string(),
            language_code: None,
            task_status: "inProgress".to_string(),
            task_status_reason: None,
            pending_status: Some(PendingTaskStatus {
                due_at: due,
                status: "completed".to_string(),
                reason: None,
            }),
        };

        for (case, due, expected) in [
            ("due in the future", now + TASK_COMPLETION_DELAY, "inProgress"),
            ("due now", now, "completed"),
            ("due in the past", now - TASK_COMPLETION_DELAY, "completed"),
        ] {
            let mut subject = task(due);
            settle_task_status(&mut subject, now);
            assert_eq!(
                subject.task_status, expected,
                "case {case:?}: wrong status, pending {:?}",
                subject.pending_status
            );
        }

        // A settled task keeps the status it reached: there is nothing further owed.
        let mut settled = task(now);
        settle_task_status(&mut settled, now);
        settle_task_status(&mut settled, now + TASK_COMPLETION_DELAY);
        assert_eq!(
            (settled.task_status.as_str(), settled.pending_status.is_some()),
            ("completed", false),
            "a settled task must stay settled"
        );
    }

    /// Every identifier the service mints is one it recognises, and the minted ids are
    /// distinct — the two halves of [`mint_task_id`]/[`is_task_id`] agreeing is what
    /// keeps a task the service created readable.
    #[test]
    fn a_minted_task_id_is_recognised_as_one() {
        let mut minted = std::collections::BTreeSet::new();
        for round in 0..64 {
            let id = mint_task_id();
            assert!(
                is_task_id(&id),
                "round {round}: the minted id {id:?} must be recognised as a task id"
            );
            // A minted id must also satisfy the model's own `TaskId` pattern
            // (`^[a-zA-Z0-9_-]{1,100}$`), or no request could ever carry it.
            assert!(
                (1..=100).contains(&id.len())
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "round {round}: the minted id {id:?} is outside the modeled TaskId pattern"
            );
            assert!(
                minted.insert(id.clone()),
                "round {round}: the minted id {id:?} was minted twice"
            );
        }
    }

    /// A value the model's pattern admits is NOT thereby an identifier: only the
    /// canonical hyphenated UUID form is.
    #[test]
    fn only_the_minted_form_is_a_task_id() {
        for (case, value, expected) in [
            ("a canonical uuid", "6f3a1b2c-9d4e-4f5a-8b6c-7d8e9f0a1b2c", true),
            ("the nil uuid", "00000000-0000-0000-0000-000000000000", true),
            ("upper-case hex digits", "6F3A1B2C-9D4E-4F5A-8B6C-7D8E9F0A1B2C", true),
            ("a word", "notataskid", false),
            ("one character", "a", false),
            ("digits", "0123456789", false),
            // The 32-digit, braced and URN spellings of a UUID: a parser takes them,
            // but this service mints none of them.
            ("an unhyphenated uuid", "6f3a1b2c9d4e4f5a8b6c7d8e9f0a1b2c", false),
            ("a uuid with a group too short", "6f3a1b2c-9d4e-4f5a-8b6c-7d8e9f0a1b2", false),
            ("a uuid with a non-hex digit", "6f3a1b2c-9d4e-4f5a-8b6c-7d8e9f0a1b2z", false),
            ("hyphens in the wrong places", "6f3a1b2c9-d4e-4f5a-8b6c-7d8e9f0a1b2c", false),
            ("a 36-character word", &"a".repeat(36), false),
        ] {
            let actual = is_task_id(value);
            assert_eq!(
                actual, expected,
                "case {case:?}: is_task_id({value:?}) is {actual}, expected {expected}"
            );
        }
    }

    /// A lexicon exists only in the Region it was stored in: a read, a count and a
    /// removal in another Region see nothing of it.
    #[test]
    fn the_lexicon_store_is_partitioned_by_region() {
        let lexicon = |name: &str, region: &str| StoredLexicon {
            name: name.to_string(),
            content: "<lexicon/>".to_string(),
            alphabet: Some("ipa".to_string()),
            language_code: "en-US".to_string(),
            last_modified: SystemTime::now(),
            lexicon_arn: ::engine::emulator_arn("polly", region, &format!("lexicon/{name}")),
            lexemes_count: 1,
            size: 10,
        };

        let mut state = ComAmazonawsPollyParrotV1State::default();
        // The same name in two regions, plus a second name in one of them.
        for (region, name) in [("us-east-1", "Shared"), ("eu-west-1", "Shared"), ("eu-west-1", "Only")] {
            state.store_lexicon(region, lexicon(name, region));
        }

        for (region, expected) in [
            ("us-east-1", vec!["Shared"]),
            ("eu-west-1", vec!["Only", "Shared"]),
            ("ap-southeast-2", Vec::new()),
        ] {
            let names: Vec<&str> = state
                .lexicons_in_region(region, None)
                .map(|stored| stored.name.as_str())
                .collect();
            assert_eq!(
                names, expected,
                "region {region:?}: wrong lexicons, store {:?}",
                state.lexicons.keys().collect::<Vec<_>>()
            );
            assert_eq!(
                state.lexicon_count_in_region(region),
                expected.len(),
                "region {region:?}: the count must be that region's own"
            );
        }
        assert!(
            state.lexicon("us-east-1", "Only").is_none(),
            "a name stored only in eu-west-1 must not be readable in us-east-1"
        );

        // A cursor pages within the region it was taken in.
        let after: Vec<&str> = state
            .lexicons_in_region("eu-west-1", Some("Only"))
            .map(|stored| stored.name.as_str())
            .collect();
        assert_eq!(after, vec!["Shared"], "the cursor must page one region");

        // Removing in one region leaves the other region's same-named lexicon.
        assert!(
            state.remove_lexicon("ap-southeast-2", "Shared").is_none(),
            "removing a lexicon of another region must remove nothing"
        );
        assert!(
            state.remove_lexicon("us-east-1", "Shared").is_some(),
            "removing a lexicon of its own region must remove it"
        );
        assert!(
            state.lexicon("eu-west-1", "Shared").is_some(),
            "another region's lexicon of the same name must survive the delete"
        );
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
