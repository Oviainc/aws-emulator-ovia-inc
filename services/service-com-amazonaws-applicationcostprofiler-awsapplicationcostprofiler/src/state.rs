// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-applicationcostprofiler-awsapplicationcostprofiler service.
//!
//! Besides the state struct this module owns the two shared rules every handler
//! in `src/ops/` reaches through `crate::state::…`: the pagination-token codec
//! and the one input check the framework's constraint walk cannot make. They
//! live here rather than in `src/ops/`, because `build.rs` treats every
//! `src/ops/*.rs` file as an operation.
//!
//! Everything the model states as a constraint trait — `@length`, `@range`,
//! `@pattern` and an enum shape's value set — is refused by
//! `aws_smithy_schema_server::constraints::validate` BEFORE a handler runs, so
//! no handler re-checks it.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::aws_smithy_types::base64;

/// The page size `ListReportDefinitions` uses when the caller sends no
/// `maxResults`: the modelled maximum of that member, so an unpaginated caller
/// sees as much as one page can carry.
///
/// Source: `smithy_query(query="operation", operation="ListReportDefinitions")`
/// -> `maxResults (integer, range 1..100, @httpQuery(maxResults))`.
pub const MAX_RESULTS_MAX: usize = 100;

/// How many report definitions this emulator keeps for one account.
///
/// The real service publishes a per-account report-definition quota and answers
/// `ServiceQuotaExceededException` past it; the number itself is not in the
/// model, so this bound is the emulator's own. It exists so a caller cannot grow
/// [`ComAmazonawsApplicationcostprofilerAwsapplicationcostprofilerState::report_definitions`]
/// without limit, and it is answered with the operation's own modelled error
/// rather than a silent refusal.
pub const MAX_REPORT_DEFINITIONS: usize = 100;

/// How many accepted usage imports this emulator retains.
///
/// The model binds no operation that reads an import back — there is no
/// Get/List/Delete for one — so retaining only the most recent submissions
/// cannot change any caller-visible answer, while it bounds the one store that
/// would otherwise grow once per request forever. Id uniqueness does not depend
/// on retention: it comes from
/// [`ComAmazonawsApplicationcostprofilerAwsapplicationcostprofilerState::next_import_seq`],
/// which never decreases.
pub const MAX_RETAINED_IMPORTS: usize = 1024;

/// A report definition exactly as the service stores it.
///
/// `GetReportDefinition` and `ListReportDefinitions` render their output from
/// this record, so what a reader sees is what a writer stored.
#[derive(Debug, Clone)]
pub struct ReportDefinitionRecord {
    /// The caller-chosen id this definition is keyed by.
    pub report_id: String,
    /// Free-text description.
    pub report_description: String,
    /// One of the `ReportFrequency` enum's wire values.
    pub report_frequency: String,
    /// One of the `Format` enum's wire values.
    pub format: String,
    /// `destinationS3Location.bucket`.
    pub destination_bucket: String,
    /// `destinationS3Location.prefix`.
    pub destination_prefix: String,
    /// When the definition was first created. Never changes afterwards.
    pub created_at: SystemTime,
    /// When the definition was last written (created or updated).
    pub last_updated: SystemTime,
}

/// One accepted `ImportApplicationUsage` submission.
#[derive(Debug, Clone)]
pub struct UsageImportRecord {
    /// The minted, service-owned id returned to the caller.
    pub import_id: String,
    /// `sourceS3Location.bucket`.
    pub source_bucket: String,
    /// `sourceS3Location.key`.
    pub source_key: String,
    /// `sourceS3Location.region`, when the caller named one.
    pub source_region: Option<String>,
    /// When the import was accepted.
    pub submitted_at: SystemTime,
}

/// Per-service state for com-amazonaws-applicationcostprofiler-awsapplicationcostprofiler. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsApplicationcostprofilerAwsapplicationcostprofilerState {
    /// Report definitions by `reportId`, at most [`MAX_REPORT_DEFINITIONS`] of
    /// them. A `BTreeMap` so `ListReportDefinitions` has one stable, resumable
    /// order: a page token is the last id of the page and the next page is the
    /// range strictly after it.
    pub report_definitions: BTreeMap<String, ReportDefinitionRecord>,
    /// The most recent accepted usage imports by `importId`, at most
    /// [`MAX_RETAINED_IMPORTS`] of them.
    pub imports: BTreeMap<String, UsageImportRecord>,
    /// Monotonic counter behind [`Self::mint_import_id`]; never decreases, so no
    /// two imports in one process share an id.
    pub next_import_seq: u64,
}

impl ComAmazonawsApplicationcostprofilerAwsapplicationcostprofilerState {
    /// Mint the next `importId`.
    ///
    /// Hex of the submission instant followed by the monotonic counter: inside
    /// `^[0-9A-Za-z\.\-_]*$` with length 1..255 (the `ImportId` shape), and
    /// unique because the counter alone already is.
    pub fn mint_import_id(&mut self, now: SystemTime) -> String {
        self.next_import_seq = self.next_import_seq.saturating_add(1);
        let millis = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        format!("{millis:x}-{:016x}", self.next_import_seq)
    }

    /// Record an accepted import, dropping the oldest retained ones once the
    /// store holds more than [`MAX_RETAINED_IMPORTS`].
    pub fn record_import(&mut self, record: UsageImportRecord) {
        self.imports.insert(record.import_id.clone(), record);
        while self.imports.len() > MAX_RETAINED_IMPORTS {
            let oldest = self
                .imports
                .values()
                .min_by(|left, right| {
                    (left.submitted_at, &left.import_id)
                        .cmp(&(right.submitted_at, &right.import_id))
                })
                .map(|record| record.import_id.clone());
            let Some(oldest) = oldest else { break };
            self.imports.remove(&oldest);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApplicationcostprofilerAwsapplicationcostprofilerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApplicationcostprofilerAwsapplicationcostprofilerState>)
}

/// A `SystemTime` as the wire timestamp the modelled `Timestamp` members carry.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since_epoch) => ::aws_smithy_types::DateTime::from_secs_and_nanos(
            i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
            since_epoch.subsec_nanos(),
        ),
        Err(before_epoch) => {
            let delta = before_epoch.duration();
            ::aws_smithy_types::DateTime::from_secs_and_nanos(
                -i64::try_from(delta.as_secs()).unwrap_or(i64::MAX),
                0,
            )
        }
    }
}

/// Refuse `value` unless it is a legal Amazon S3 bucket name.
///
/// This is the ONE input rule a handler has to make itself. The `S3Bucket`
/// shape's `@pattern` is
/// `^(?=^.{3,63}$)(?!^(\d+\.)+\d+$)(^(([a-z0-9]|[a-z0-9][a-z0-9\-]*[a-z0-9])\.)*([a-z0-9]|[a-z0-9][a-z0-9\-]*[a-z0-9])$)$`,
/// whose lookahead and negative lookahead the `regex` crate cannot compile — and
/// `aws_smithy_schema_server::constraints` treats a pattern it cannot compile as
/// satisfied, so the framework's walk lets EVERY bucket name through. (Its
/// `@length 3..63` is checked there, and is re-stated here only because the
/// pattern restates it too.)
///
/// The rule: dot-separated labels of lowercase letters, digits and hyphens, each
/// label starting and ending with a letter or digit, total length 3..63, and not
/// an all-numeric dotted (IPv4-shaped) name.
pub fn validate_s3_bucket(member: &str, value: &str) -> Result<(), String> {
    let labels: Vec<&str> = value.split('.').collect();
    let label_ok = |label: &str| {
        let bytes = label.as_bytes();
        let inner_ok = bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
        let edges_ok = bytes
            .first()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && bytes
                .last()
                .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        inner_ok && edges_ok
    };
    // `(?!^(\d+\.)+\d+$)` only excludes a DOTTED all-numeric name, so a single
    // all-digit label is legal and must not be refused here.
    let ipv4_shaped = labels.len() > 1
        && labels
            .iter()
            .all(|label| !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit()));
    if (3..=63).contains(&value.len()) && labels.iter().all(|label| label_ok(label)) && !ipv4_shaped
    {
        return Ok(());
    }
    Err(format!(
        "Member {member} must be 3 to 63 characters long and must be a valid Amazon S3 bucket \
         name: dot-separated labels of lowercase letters, digits and hyphens, each starting and \
         ending with a letter or digit, and not an all-numeric dotted name"
    ))
}

/// The pagination cursor for a page whose last item was `report_id`.
///
/// Padded standard base64, which is what the `Token` shape's pattern
/// `^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$` describes.
pub fn encode_page_token(report_id: &str) -> String {
    base64::encode(report_id.as_bytes())
}

/// The cursor [`encode_page_token`] produced, or a refusal naming `member`.
///
/// A token the service did not mint cannot name a resume point, so it is
/// refused rather than silently treated as "start from the beginning". The
/// `Token` shape's own pattern only says the token is base64-SHAPED, so whether
/// it decodes to an id this service could have issued is the handler's to say.
pub fn decode_page_token(member: &str, token: &str) -> Result<String, String> {
    let refusal = || format!("Member {member} is not a valid pagination token");
    let bytes = base64::decode(token).map_err(|_| refusal())?;
    let cursor = String::from_utf8(bytes).map_err(|_| refusal())?;
    // The cursor is a `reportId`, so it must satisfy that shape to be one this
    // service could have put in a token.
    let plausible = (1..=255).contains(&cursor.len())
        && cursor
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_');
    if !plausible {
        return Err(refusal());
    }
    Ok(cursor)
}
