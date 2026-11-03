// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-translate-awsshinefrontendservice-20170701 service.
//!
//! The resources this service owns are the custom terminologies
//! `ImportTerminology` stores and `GetTerminology`, `ListTerminologies`,
//! `TranslateText` and `DeleteTerminology` read back. Everything those
//! operations report — the ARN, the languages, the term and byte counts, the
//! timestamps, the terms a translation applied — is derived from the record
//! held here, so a create followed by a read answers with what was created.
//!
//! The shapes shared by those five operations live here rather than in one
//! `ops/` file so two of them cannot report one stored terminology by different
//! rules. For the same reason the translation transform the two synchronous
//! translate operations share — resolving a requested terminology, substituting
//! its terms and marking the output as synthetic — lives here too, so `Text`
//! and a document of the same text are translated by one rule.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::sha2::{Digest, Sha256};

use crate::ops::types::{
    EncryptionKey, InvalidParameterValueException, InvalidRequestException,
    LimitExceededException, ResourceNotFoundException, Tag, Term, TerminologyDataLocation,
    TerminologyProperties, TextSizeLimitExceededException, TooManyTagsException,
    TranslationSettings, UnsupportedLanguagePairException,
};

/// The ARN namespace this service stamps into every resource ARN: `translate`
/// (`awsService.arnNamespace`, `smithy_query query=service_info`).
pub const ARN_SERVICE: &str = "translate";

/// The Region an ARN carries when the request's credential scope names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `Directionality`'s `UNI` value: the resource has one source language and all
/// of its other languages are target languages (`smithy_query shape=Directionality`).
pub const DIRECTIONALITY_UNI: &str = "UNI";

/// `Directionality`'s `MULTI` value: any language in the resource can be the
/// source language or a target language.
pub const DIRECTIONALITY_MULTI: &str = "MULTI";

/// `TerminologyDataFormat`'s three values (`smithy_query shape=TerminologyDataFormat`).
pub const FORMAT_CSV: &str = "CSV";
pub const FORMAT_TSV: &str = "TSV";
pub const FORMAT_TMX: &str = "TMX";

/// The `SourceLanguageCode` that asks Amazon Translate to determine the source
/// language of the text itself ("you can specify `auto` in the
/// `SourceLanguageCode` field", `smithy_query shape=TranslateTextRequest
/// member=SourceLanguageCode`).
pub const AUTO_LANGUAGE_CODE: &str = "auto";

/// The repository a terminology's input file is presigned from.
const REPOSITORY_TYPE_S3: &str = "S3";

/// How long a reported data location stays valid: "The location is returned as
/// a presigned URL that has a 30-minute expiration"
/// (`smithy_query shape=TerminologyDataLocation member=Location`).
const PRESIGNED_EXPIRY_SECONDS: u32 = 1800;

/// The largest terminology input file this emulator parses, which is the
/// documented quota for a custom terminology file (10 MB). The cap is applied to
/// the uploaded blob BEFORE it is parsed or stored, so a caller cannot choose
/// how much work one request does.
pub const MAX_TERMINOLOGY_FILE_BYTES: usize = 10 * 1024 * 1024;

/// How many custom terminology resources this store holds, which is the
/// documented quota for custom terminologies per account per Region (100).
///
/// The store only ever shrinks on a caller's `DeleteTerminology`, so without
/// this bound a caller looping on import grows it until the host runs out of
/// memory; with it, that caller meets the modelled quota instead.
pub const MAX_STORED_TERMINOLOGIES: usize = 100;

/// How many terms one terminology resource may hold: "A terminology list can
/// contain a maximum of 256 terms" (`smithy_query shape=TranslateTextRequest
/// member=TerminologyNames`).
pub const MAX_TERMS_PER_TERMINOLOGY: usize = 256;

/// How many applied terms one translation reports: "A maximum of 250 terms will
/// be returned, and the specific terms applied will be the first 250 terms in
/// the source text" (`smithy_query shape=AppliedTerminology member=Terms`).
pub const MAX_APPLIED_TERMS: usize = 250;

/// The longest text one term carries: `Term$SourceText` and `Term$TargetText`
/// are both `length 0..10000` (`smithy_query shape=Term`), so a cell longer
/// than that could never be reported back as an applied term. The import counts
/// such a row as skipped rather than storing text no response can carry, which
/// is also what keeps one stored term from being re-emitted at an arbitrary
/// size by every match a translation makes.
pub const MAX_TERM_TEXT_CHARS: usize = 10_000;

/// The longest translation one response carries: `TranslatedTextString` is
/// `length 0..20000` (`smithy_query shape=TranslatedTextString`). The
/// terminology substitution stops at this ceiling and the request is refused,
/// so one small request cannot make the emulator build — or answer with — a
/// larger body than the model's own output member admits. It also bounds the
/// applied-terms report, since every term reported there was emitted into the
/// translation at least once.
pub const MAX_TRANSLATED_TEXT_CHARS: usize = 20_000;

/// How many custom terminology resources one translation request may name: "You
/// can use one custom terminology resource in your translation request"
/// (`smithy_query shape=TranslateTextRequest member=TerminologyNames`).
pub const MAX_TERMINOLOGIES_PER_TRANSLATION: usize = 1;

/// How many tags one resource carries: "You have added too many tags to this
/// resource. The maximum is 50 tags" (`smithy_query shape=TooManyTagsException`).
/// The tag list a request carries is bounded far wider than that by the model
/// (`TagList` is `length 0..200`), so this quota is the operation's own rule
/// rather than a shape constraint: the boundary walk admits the request and the
/// handler refuses it.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest `Text` one translation request may carry, in BYTES: "The text
/// string can be a maximum of 10,000 bytes long. Depending on your character
/// set, this may be fewer than 10,000 characters"
/// (`smithy_query shape=TranslateTextRequest member=Text`).
///
/// The documented limit counts BYTES, which no character-length shape
/// constraint expresses, so the size of a request's text is the operation's own
/// rule and its own modelled refusal.
pub const MAX_TEXT_BYTES: usize = 10_000;

/// The values `DisplayLanguageCode` declares (`smithy_query
/// shape=DisplayLanguageCode`), in the order this service renders an
/// accepted-value set into a constraint-violation message.
///
/// The set is the model's; the ORDER is the service's own — neither sorted nor
/// the model's declaration order — and it is part of the fixed text of that
/// message, so a caller parsing the accepted values out of a refusal reads the
/// list the service really emits. `tests/all/rejection_paths.rs` pins this list
/// to the same ten codes the model declares, so it can only ever differ from
/// the model in order.
pub const DISPLAY_LANGUAGE_CODE_VALUE_SET: &[&str] =
    &["de", "zh-TW", "pt", "ko", "en", "it", "fr", "zh", "es", "ja"];

/// `MaxResultsInteger`'s ceiling, which is also the page size a listing uses
/// when the request names none (`range 1..500`).
pub const MAX_LIST_PAGE_SIZE: i32 = 500;

/// The marker an opaque `NextToken` this service issues carries, so a token
/// another service (or a caller) made up is refused rather than silently read
/// as a terminology name.
const PAGE_TOKEN_PREFIX: &str = "terminology:";

/// Every language Amazon Translate translates between, as
/// `(language code, detector code)`.
///
/// The first column is the `LanguageCodeString` a request names; its source is
/// the Supported languages table the model links to from
/// `smithy_query shape=TranslateTextRequest member=SourceLanguageCode`
/// (<https://docs.aws.amazon.com/translate/latest/dg/what-is-languages.html>).
/// The second column is the ISO 639-3 code the language detector reports for
/// that same language, and is empty for a regional variant the detector cannot
/// distinguish from its base language (it is never auto-detected, only named).
pub const SUPPORTED_LANGUAGES: &[(&str, &str)] = &[
    ("af", "afr"),
    ("sq", "sqi"),
    ("am", "amh"),
    ("ar", "ara"),
    ("hy", "hye"),
    ("az", "aze"),
    ("bn", "ben"),
    ("bs", "bos"),
    ("bg", "bul"),
    ("ca", "cat"),
    ("zh", "cmn"),
    ("zh-TW", ""),
    ("hr", "hrv"),
    ("cs", "ces"),
    ("da", "dan"),
    ("fa-AF", "prs"),
    ("nl", "nld"),
    ("en", "eng"),
    ("et", "est"),
    ("fa", "pes"),
    ("tl", "tgl"),
    ("fi", "fin"),
    ("fr", "fra"),
    ("fr-CA", ""),
    ("ka", "kat"),
    ("de", "deu"),
    ("el", "ell"),
    ("gu", "guj"),
    ("ht", "hat"),
    ("ha", "hau"),
    ("he", "heb"),
    ("hi", "hin"),
    ("hu", "hun"),
    ("is", "isl"),
    ("id", "ind"),
    ("ga", "gle"),
    ("it", "ita"),
    ("ja", "jpn"),
    ("kn", "kan"),
    ("kk", "kaz"),
    ("ko", "kor"),
    ("lv", "lav"),
    ("lt", "lit"),
    ("mk", "mkd"),
    ("ms", "msa"),
    ("ml", "mal"),
    ("mt", "mlt"),
    ("mr", "mar"),
    ("mn", "mon"),
    ("no", "nob"),
    ("ps", "pus"),
    ("pl", "pol"),
    ("pt", "por"),
    ("pt-PT", ""),
    ("pa", "pan"),
    ("ro", "ron"),
    ("ru", "rus"),
    ("sr", "srp"),
    ("si", "sin"),
    ("sk", "slk"),
    ("sl", "slv"),
    ("so", "som"),
    ("es", "spa"),
    ("es-MX", ""),
    ("sw", "swa"),
    ("sv", "swe"),
    ("ta", "tam"),
    ("te", "tel"),
    ("th", "tha"),
    ("tr", "tur"),
    ("uk", "ukr"),
    ("ur", "urd"),
    ("uz", "uzb"),
    ("vi", "vie"),
    ("cy", "cym"),
];

/// Whether Amazon Translate translates this language at all.
pub fn is_supported_language(code: &str) -> bool {
    SUPPORTED_LANGUAGES.iter().any(|(known, _)| *known == code)
}

/// The language code a terminology file's column or a TMX `xml:lang` names,
/// resolved against [`SUPPORTED_LANGUAGES`].
///
/// A file may carry a regional subtag the service has no separate code for
/// (`en-US`), so the primary subtag is tried second: the column still names a
/// language this service translates.
pub fn canonical_language(declared: &str) -> Option<&'static str> {
    let declared = declared.trim();
    if let Some((code, _)) = SUPPORTED_LANGUAGES
        .iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(declared))
    {
        return Some(code);
    }
    let primary = declared.split('-').next()?;
    SUPPORTED_LANGUAGES
        .iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(primary))
        .map(|(code, _)| *code)
}

/// The `LanguageCodeString` for a language the detector reported, or `None` when
/// Amazon Translate does not translate it.
pub fn language_for_detector_code(detected: &str) -> Option<&'static str> {
    SUPPORTED_LANGUAGES
        .iter()
        .find(|(_, detector)| !detector.is_empty() && *detector == detected)
        .map(|(code, _)| *code)
}

/// One row of a terminology file: the text of a term in each language the file
/// declares, keyed by that language's code.
///
/// Keeping the whole row — rather than a single source/target pair — is what
/// lets a multi-directional resource serve any pair of its languages, which is
/// exactly what `MULTI` means: "Any language in the terminology resource can be
/// the source language or a target language".
#[derive(Debug, Clone, Default)]
pub struct TermEntry {
    pub values: BTreeMap<String, String>,
}

impl TermEntry {
    /// This term's text in `language`, or `None` when the row leaves that
    /// column empty.
    pub fn text(&self, language: &str) -> Option<&str> {
        self.values
            .get(language)
            .map(String::as_str)
            .filter(|text| !text.is_empty())
    }
}

/// One stored custom terminology resource.
#[derive(Debug, Clone)]
pub struct Terminology {
    pub name: String,
    pub description: Option<String>,
    /// The revision segment of [`Terminology::arn`], derived from the imported
    /// file's bytes, so re-importing different content moves the ARN on and
    /// re-importing the same content does not.
    pub revision: String,
    pub arn: String,
    pub source_language_code: String,
    pub target_language_codes: Vec<String>,
    pub format: String,
    /// `None` when the import named no directionality: "it behaves as
    /// uni-directional terminology, although this parameter will have a null
    /// value" (`smithy_query shape=TerminologyData member=Directionality`).
    pub directionality: Option<String>,
    pub encryption_key: Option<EncryptionKey>,
    pub size_bytes: i32,
    pub term_count: i32,
    pub skipped_term_count: i32,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    pub terms: Vec<TermEntry>,
    pub tags: Vec<Tag>,
}

impl Terminology {
    /// Whether any of this resource's languages may be the source language.
    pub fn is_multi_directional(&self) -> bool {
        self.directionality.as_deref() == Some(DIRECTIONALITY_MULTI)
    }

    /// Every language this resource declares: its source language first, then
    /// its target languages, in the order the imported file declared them.
    pub fn languages(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.source_language_code.as_str())
            .chain(self.target_language_codes.iter().map(String::as_str))
    }

    /// Whether this resource can be applied to a `source` -> `target`
    /// translation: a multi-directional resource serves any pair of its
    /// languages, a uni-directional one only its own source language into one
    /// of its target languages.
    pub fn covers(&self, source: &str, target: &str) -> bool {
        if source == target {
            return false;
        }
        if self.is_multi_directional() {
            let mut languages = self.languages();
            let has_source = languages.any(|language| language == source);
            has_source && self.languages().any(|language| language == target)
        } else {
            self.source_language_code == source
                && self
                    .target_language_codes
                    .iter()
                    .any(|language| language == target)
        }
    }

    /// The `(source text, target text)` pairs this resource provides for a
    /// `source` -> `target` translation, in the order the imported file
    /// declared them. A row that leaves either column empty provides no pair.
    pub fn term_pairs(&self, source: &str, target: &str) -> Vec<(&str, &str)> {
        if !self.covers(source, target) {
            return Vec::new();
        }
        self.terms
            .iter()
            .filter_map(|entry| Some((entry.text(source)?, entry.text(target)?)))
            .collect()
    }
}

/// Per-service state for com-amazonaws-translate-awsshinefrontendservice-20170701. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTranslateAwsshinefrontendservice20170701State {
    /// Every imported custom terminology, keyed by its `Name`. A [`BTreeMap`]
    /// so `ListTerminologies` enumerates them in one stable order and its
    /// `NextToken` can resume strictly after the name a page ended on.
    pub terminologies: BTreeMap<String, Terminology>,
}

impl ComAmazonawsTranslateAwsshinefrontendservice20170701State {
    /// The stored terminology an ARN names, or `None` when this account holds
    /// none with that ARN.
    ///
    /// The tagging operations address a resource by ARN rather than by name, so
    /// they resolve it against the ARN each record already carries — the same
    /// one every read reports it by, revision segment included, so an ARN from a
    /// superseded import names no resource.
    pub fn terminology_for_arn(&self, arn: &str) -> Option<&Terminology> {
        self.terminologies
            .values()
            .find(|record| record.arn == arn)
    }

    /// The `Name` of the terminology an ARN names, which is the key a mutating
    /// tagging call reaches the record through.
    pub fn terminology_name_for_arn(&self, arn: &str) -> Option<String> {
        self.terminology_for_arn(arn)
            .map(|record| record.name.clone())
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTranslateAwsshinefrontendservice20170701State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTranslateAwsshinefrontendservice20170701State>)
}

/// The Region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// An ARN's region segment is `[a-z0-9-]+`, so a credential scope outside that
/// grammar is not stamped into one.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A stored instant as the wire timestamp the `Timestamp` members carry.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// `arn:aws:translate:{region}:{account}:terminology/{name}/{revision}`.
pub fn terminology_arn(region: &str, name: &str, revision: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("terminology/{name}/{revision}"),
    )
}

/// The revision a terminology's ARN carries, derived from the imported file's
/// bytes: the same content always reports the same revision, and different
/// content always reports a different one.
pub fn content_revision(file: &[u8]) -> String {
    ::hex::encode(Sha256::digest(file)).chars().take(16).collect()
}

/// The stored terminology as the `TerminologyProperties` every one of the five
/// operations reports it by.
pub fn terminology_properties(record: &Terminology) -> TerminologyProperties {
    TerminologyProperties {
        name: Some(record.name.clone()),
        description: record.description.clone(),
        arn: Some(record.arn.clone()),
        source_language_code: Some(record.source_language_code.clone()),
        target_language_codes: Some(record.target_language_codes.clone()),
        encryption_key: record.encryption_key.clone(),
        size_bytes: Some(record.size_bytes),
        term_count: Some(record.term_count),
        created_at: Some(timestamp(record.created_at)),
        last_updated_at: Some(timestamp(record.last_updated_at)),
        directionality: record.directionality.clone(),
        message: None,
        skipped_term_count: Some(record.skipped_term_count),
        format: Some(record.format.clone()),
    }
}

/// The object key the terminology's input file is held under, which carries the
/// revision so one name's successive imports are distinct objects.
fn terminology_object_key(record: &Terminology, format: &str, suffix: &str) -> String {
    let extension = format.to_ascii_lowercase();
    format!(
        "{}/{}/{}{suffix}.{extension}",
        record.name, record.revision, record.name
    )
}

/// A presigned-style S3 location for `key`, derived entirely from the stored
/// record and the request's Region, so two reads of one terminology report one
/// location.
fn presigned_location(region: &str, key: &str) -> String {
    let bucket = format!(
        "translate-terminology-{}-{region}",
        ::engine::emulator_account_id()
    );
    let signature = ::hex::encode(Sha256::digest(format!("{bucket}/{key}").as_bytes()));
    format!(
        "https://{bucket}.s3.{region}.amazonaws.com/{key}\
         ?X-Amz-Algorithm=AWS4-HMAC-SHA256\
         &X-Amz-Expires={PRESIGNED_EXPIRY_SECONDS}\
         &X-Amz-SignedHeaders=host\
         &X-Amz-Signature={signature}"
    )
}

/// Where the terminology's input file is reported from, in `format`.
pub fn terminology_data_location(
    region: &str,
    record: &Terminology,
    format: &str,
) -> TerminologyDataLocation {
    TerminologyDataLocation {
        repository_type: REPOSITORY_TYPE_S3.to_string(),
        location: presigned_location(region, &terminology_object_key(record, format, "")),
    }
}

/// The auxiliary location, which reports "any errors or warnings that were
/// produced by your input file" (`smithy_query shape=ImportTerminologyResponse
/// member=AuxiliaryDataLocation`) — so it exists only for an import that had
/// something to report, i.e. one whose file held rows the service skipped.
pub fn auxiliary_data_location(
    region: &str,
    record: &Terminology,
) -> Option<TerminologyDataLocation> {
    if record.skipped_term_count <= 0 {
        return None;
    }
    Some(TerminologyDataLocation {
        repository_type: REPOSITORY_TYPE_S3.to_string(),
        location: presigned_location(
            region,
            &terminology_object_key(record, FORMAT_CSV, "-auxiliary"),
        ),
    })
}

/// The opaque `NextToken` a listing hands back for the name its page ended on.
///
/// Base64 of a marked cursor: `NextToken` is `^\p{ASCII}{0,8192}$`, which base64
/// satisfies for every name the store can hold, and the codec is the one the
/// wire serializer already uses (`aws_smithy_types::base64`), so this crate
/// grows no second one.
pub fn encode_page_token(name: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{PAGE_TOKEN_PREFIX}{name}"))
}

/// The name a `NextToken` resumes after, or `None` when the token was not one
/// this listing issued.
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(PAGE_TOKEN_PREFIX)
        .map(ToString::to_string)
}

/// How many entries one page holds: the request's `MaxResults`, or the member's
/// own ceiling when it names none, clamped into the modelled range.
///
/// The boundary walk already refuses a `MaxResults` outside `1..500`, so the
/// clamp is defensive — a page size of zero would be a silent dead end.
pub fn page_size(requested: Option<i32>) -> usize {
    let size = requested
        .unwrap_or(MAX_LIST_PAGE_SIZE)
        .clamp(1, MAX_LIST_PAGE_SIZE);
    usize::try_from(size).unwrap_or(1)
}

/// The resource a not-found message names when the request asked for the
/// terminology ITSELF — the record a read or a translation resolves.
pub const TERMINOLOGY_RESOURCE: &str = "Terminology";

/// The resource a not-found message names when the request asked to REMOVE the
/// terminology, which addresses its metadata record rather than its content.
pub const TERMINOLOGY_METADATA_RESOURCE: &str = "TerminologyMetadata";

/// The `ResourceNotFoundException` a terminology operation answers for a name no
/// import stored: which resource of this account, under which name, was not
/// there.
///
/// `resource` is the operation's own resource word — [`TERMINOLOGY_RESOURCE`] for
/// a call that resolves the terminology, [`TERMINOLOGY_METADATA_RESOURCE`] for
/// one that deletes it — so a caller can tell a missing terminology from missing
/// terminology metadata, and `name` is the one the request asked for.
pub fn terminology_not_found(resource: &str, name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "The {resource} for {} {name} was not found",
            ::engine::emulator_account_id()
        )),
    }
}

/// The `ResourceNotFoundException` a call that addresses a resource by ARN
/// answers when this account holds no resource with that ARN.
///
/// Such a call names no resource name to report back, so the wording is the
/// sentence the model documents for the error class
/// (`smithy_query shape=ResourceNotFoundException`); the ARN the caller asked
/// for is already in its own request.
pub fn resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(
            "The resource you are looking for has not been found. Review the resource \
             you're looking for and see if a different resource will accomplish your \
             needs before retrying the revised request."
                .to_string(),
        ),
    }
}

/// The `TextSizeLimitExceededException` a request whose `Text` is larger than
/// [`MAX_TEXT_BYTES`] is refused with: the limit the service allows and the size
/// this request sent, both as byte counts.
pub fn text_size_limit_exceeded(bytes: usize) -> TextSizeLimitExceededException {
    TextSizeLimitExceededException {
        message: Some(format!(
            "Input text size exceeds limit. Max length of request text allowed is \
             {MAX_TEXT_BYTES} bytes while in this request the text size is {bytes} bytes"
        )),
    }
}

/// The `UnsupportedLanguagePairException` a translation the service does not
/// perform is refused with: the message names the pair that was rejected, and
/// the same two codes travel in the shape's own members.
///
/// The codes are the RESOLVED ones — for a request that asked the service to
/// determine the source language, the code it resolved the source to — so the
/// refusal reports the pair the service actually judged.
pub fn unsupported_language_pair(source: &str, target: &str) -> UnsupportedLanguagePairException {
    UnsupportedLanguagePairException {
        message: Some(format!("Unsupported language pair: {source} to {target}.")),
        source_language_code: Some(source.to_string()),
        target_language_code: Some(target.to_string()),
    }
}

/// The `TooManyTagsException` a request that would leave a resource holding more
/// than [`MAX_TAGS_PER_RESOURCE`] tags is refused with. One wording for the one
/// error class, whichever operation reached the limit.
pub fn too_many_tags(resource_arn: &str) -> TooManyTagsException {
    TooManyTagsException {
        message: Some("Allowable tag limit for resource exceeded".to_string()),
        resource_arn: Some(resource_arn.to_string()),
    }
}

/// The `InvalidParameterValueException` wording this service uses: which
/// parameter was wrong and why, in front of the sentence the model documents for
/// the error ("Review the value of the parameter you are using to correct it,
/// and then retry your operation").
pub fn invalid_parameter_value(parameter: &str, reason: &str) -> InvalidParameterValueException {
    InvalidParameterValueException {
        message: Some(format!(
            "The value of the parameter {parameter} is not valid: {reason}. \
             Review the value of the parameter you are using to correct it, and \
             then retry your operation."
        )),
    }
}

/// A `(source text, target text)` pair as the wire reports an applied term.
pub fn applied_term(source_text: &str, target_text: &str) -> Term {
    Term {
        source_text: Some(source_text.to_string()),
        target_text: Some(target_text.to_string()),
    }
}

/// A byte length as the `Integer` members report it. Saturating rather than
/// wrapping: the import path caps the file at
/// [`MAX_TERMINOLOGY_FILE_BYTES`] first, so the saturation is unreachable and
/// only keeps the conversion total.
pub fn counted(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// The largest document one synchronous translation carries, in BYTES: "The
/// document size must not exceed 100 KB" (`smithy_query
/// shape=TranslateDocumentRequest member=Document`), which is also the length
/// `DocumentContent` is modelled with (`length 0..102400`).
pub const MAX_DOCUMENT_BYTES: usize = 100 * 1024;

/// The `InvalidRequestException` a request this service cannot act on is
/// refused with: the sentence the model documents for the error class
/// (`smithy_query shape=InvalidRequestException`), for a request whose own
/// members are each admissible but that asks for something the service does not
/// do.
pub fn invalid_request() -> InvalidRequestException {
    InvalidRequestException {
        message: Some(
            "The request that you made is not valid. Check your request to determine why \
             it's not valid and then retry the request."
                .to_string(),
        ),
    }
}

/// The `LimitExceededException` a request whose document grew past what one
/// synchronous translation carries is refused with: the sentence the model
/// documents for the error class (`smithy_query shape=LimitExceededException`),
/// since what was exceeded is a size limit on the request's own document.
pub fn limit_exceeded() -> LimitExceededException {
    LimitExceededException {
        message: Some(
            "The specified limit has been exceeded. Review your request and retry it with a \
             quantity below the stated limit."
                .to_string(),
        ),
    }
}

/// The marker a synthetic translation carries, so a caller can never mistake
/// this emulator's output for a real translation. The target language is part of
/// it, which is what makes the output differ per requested pair.
const SYNTHETIC_MARKER: &str = "mt";

/// The marker the synthetic translation carries in front of the
/// terminology-substituted text: the resolved language pair and the settings
/// this translation honoured.
///
/// The settings are part of the output rather than merely echoed back, so
/// `AppliedSettings` reports settings the translation really applied: a
/// different `Settings` produces a different translation, and `Brevity`
/// — which "reduces the length of the translation output" — produces a shorter
/// one by dropping the pair and setting detail from the marker.
pub fn translation_marker(
    source_language_code: &str,
    target_language_code: &str,
    settings: Option<&TranslationSettings>,
) -> String {
    let brevity = settings.and_then(|settings| settings.brevity.clone());
    if brevity.is_some() {
        return format!("[{SYNTHETIC_MARKER}:{target_language_code}] ");
    }
    let mut detail = format!("{source_language_code}->{target_language_code}");
    for applied in [
        settings.and_then(|settings| settings.formality.clone()),
        settings.and_then(|settings| settings.profanity.clone()),
    ]
    .into_iter()
    .flatten()
    {
        detail.push(':');
        detail.push_str(&applied);
    }
    format!("[{SYNTHETIC_MARKER}:{detail}] ")
}

/// One requested terminology, resolved against the store: the stored record's
/// name and the `(source text, target text)` pairs it provides for this
/// translation's language pair, longest source first so a longer phrase wins
/// over a word inside it.
pub struct ResolvedTerminology {
    pub name: String,
    pub pairs: Vec<(String, String)>,
}

/// Every terminology a translation request named, resolved against the store for
/// one language pair.
///
/// `Err` carries the requested name this account holds no terminology under,
/// which each translation operation reports through its own modelled
/// not-found error.
pub fn resolve_terminologies(
    store: &ComAmazonawsTranslateAwsshinefrontendservice20170701State,
    requested: &[String],
    source_language_code: &str,
    target_language_code: &str,
) -> Result<Vec<ResolvedTerminology>, String> {
    let mut resolved = Vec::new();
    for name in requested {
        let record = store
            .terminologies
            .get(name)
            .ok_or_else(|| name.clone())?;
        let mut pairs: Vec<(String, String)> = record
            .term_pairs(source_language_code, target_language_code)
            .into_iter()
            .map(|(source, target)| (source.to_string(), target.to_string()))
            .collect();
        // Longest source text first, so a phrase is substituted as a phrase
        // rather than word by word.
        pairs.sort_by(|left, right| right.0.len().cmp(&left.0.len()).then(left.0.cmp(&right.0)));
        resolved.push(ResolvedTerminology {
            name: record.name.clone(),
            pairs,
        });
    }
    Ok(resolved)
}

/// Substitute every terminology term that occurs in `text` as a whole word, and
/// report the pairs that matched in the order they first occur in the source
/// text.
///
/// A term matches only at a word boundary, so a terminology for `hello` does not
/// rewrite the middle of `othello`, and a multi-word phrase matches as a phrase.
/// The scan is a single left-to-right pass over the text against at most one
/// terminology's terms.
///
/// `budget` is how many characters the substituted text may still grow to.
/// Substituting a term EMITS text rather than copying it, so without a ceiling
/// a short request against long stored terms would build an arbitrarily long
/// answer; the scan stops the moment the budget would be passed and reports
/// that (the third value), which the calling operation refuses.
pub fn substitute<'t>(
    text: &str,
    pairs: &'t [(String, String)],
    budget: usize,
) -> (String, Vec<(&'t str, &'t str)>, bool) {
    let mut translated = String::new();
    let mut emitted = 0usize;
    let mut applied: Vec<(&'t str, &'t str)> = Vec::new();
    let mut remaining = text;
    let mut previous: Option<char> = None;

    while !remaining.is_empty() {
        let at_word_start = previous.is_none_or(|character| !character.is_alphanumeric());
        let matched = if at_word_start {
            pairs.iter().find_map(|(source, target)| {
                let tail = remaining.strip_prefix(source.as_str())?;
                let ends_word = tail
                    .chars()
                    .next()
                    .is_none_or(|character| !character.is_alphanumeric());
                (!source.is_empty() && ends_word).then_some((source.as_str(), target.as_str()))
            })
        } else {
            None
        };

        match matched {
            Some((source, target)) => {
                let grows_by = target.chars().count();
                if emitted.saturating_add(grows_by) > budget {
                    return (translated, applied, true);
                }
                translated.push_str(target);
                emitted = emitted.saturating_add(grows_by);
                if !applied.iter().any(|(seen, _)| *seen == source)
                    && applied.len() < MAX_APPLIED_TERMS
                {
                    applied.push((source, target));
                }
                // The boundary the NEXT match is judged against is the text
                // that was emitted, which is the target term.
                previous = target.chars().next_back();
                remaining = remaining.get(source.len()..).unwrap_or_default();
            }
            None => {
                let Some(character) = remaining.chars().next() else {
                    break;
                };
                if emitted.saturating_add(1) > budget {
                    return (translated, applied, true);
                }
                translated.push(character);
                emitted = emitted.saturating_add(1);
                previous = Some(character);
                remaining = remaining.get(character.len_utf8()..).unwrap_or_default();
            }
        }
    }

    (translated, applied, false)
}
