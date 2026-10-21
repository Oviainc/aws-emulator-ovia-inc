// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-textract-textract service, plus the one
//! document/`Block` model every read path derives its answer from.
//!
//! ## Why the derivation lives here
//!
//! Amazon Textract has four read paths over the SAME result — `DetectDocumentText`,
//! `AnalyzeDocument`, `GetDocumentTextDetection` and `GetDocumentAnalysis` — and the
//! synchronous and asynchronous answers for one document must agree. So a document is
//! registered once (keyed by its identity: an S3 object, or the SHA-256 of its inline
//! bytes), the text read out of it is stored, and every path derives its blocks from
//! that stored content with [`blocks`].
//!
//! ## What this emulator can read, and what it refuses
//!
//! [`read_content`] is the ONLY source of a document's text, and it never invents one:
//!
//! * inline `Bytes` that decode as text — their real lines and words are extracted
//!   verbatim (form feed, `U+000C`, is the page break), and FORMS/TABLES analysis reads
//!   that same real text;
//! * inline `Bytes` that are not text (a PNG, JPEG or TIFF raster) — no OCR engine runs
//!   in this emulator, so there is no text to report: refused as
//!   [`Unreadable::UnsupportedFormat`], which the handlers answer with the modelled
//!   `UnsupportedDocumentException`;
//! * an S3 object — this emulator's Amazon S3 cannot serve object bytes yet, so Textract
//!   genuinely cannot access it: refused as [`Unreadable::S3Unreadable`], which the
//!   handlers answer with the modelled `InvalidS3ObjectException` (synchronously) or a
//!   job that ends in the modelled `FAILED` state carrying the reason (asynchronously).
//!
//! Nothing here is derived from a digest, echoed from the request, or randomised: a
//! block only ever describes text that was actually read out of the document.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    Block, BoundingBox, Document, DocumentLocation, DocumentMetadata, Geometry,
    NotificationChannel, OutputConfig, Point, Relationship, Warning, FEATURE_TYPE_VALUES,
};

/// The value Textract reports as `DetectDocumentTextModelVersion`.
pub const DETECT_DOCUMENT_TEXT_MODEL_VERSION: &str = "1.0";

/// The value Textract reports as `AnalyzeDocumentModelVersion`.
pub const ANALYZE_DOCUMENT_MODEL_VERSION: &str = "1.0";

/// How long after a `Start*` call this emulator makes the job's `IN_PROGRESS` ->
/// terminal transition due.
///
/// AWS documents no duration for asynchronous Textract jobs, so this is a CHOSEN span
/// inside the emulator's 250–750 ms window. It is never zero: a caller that polls
/// `Get*` immediately after `Start*` still observes `IN_PROGRESS`.
pub const JOB_COMPLETION_DELAY: Duration = Duration::from_millis(500);

/// The page size `Get*` uses when the caller passes no `MaxResults`.
pub const DEFAULT_MAX_RESULTS: usize = 1000;

/// The largest inline document this service accepts.
///
/// AWS, on `Document$Bytes`: "The maximum size of a document that's provided in a blob
/// of bytes is 5 MB." A larger one is refused with `DocumentTooLargeException`.
pub const MAX_INLINE_DOCUMENT_BYTES: usize = 5 * 1024 * 1024;

/// The most pages one document may hold.
///
/// AWS's Textract hard limits cap a PDF or TIFF document at 3,000 pages.
pub const MAX_DOCUMENT_PAGES: usize = 3000;

/// An emulator-side ceiling on how many `Block` values one document may expand into.
///
/// NOT an AWS limit: it exists so a caller-supplied document cannot amplify one request
/// into unbounded CPU, resident memory and response bytes. A document that would exceed
/// it is refused with `DocumentTooLargeException` BEFORE any block is built, so the work
/// a single request can cause is bounded by this constant.
pub const MAX_BLOCKS_PER_DOCUMENT: usize = 50_000;

/// How many documents this service keeps. NOT an AWS limit: an emulator-side bound so a
/// caller cannot grow the heap without end. The least recently registered is evicted.
pub const MAX_STORED_DOCUMENTS: usize = 256;

/// How many jobs this service keeps. NOT an AWS limit: see [`MAX_STORED_DOCUMENTS`].
/// The oldest job (and the idempotency entry that named it) is evicted.
pub const MAX_STORED_JOBS: usize = 256;

/// The `Confidence` every block reports.
///
/// This emulator READS a document's text rather than recognising it, so there is no
/// recogniser confidence to measure. One documented constant says exactly that; a
/// per-block spread would look like a measurement nothing made. Inside `Percent`'s
/// `0..=100` range.
pub const EXTRACTED_TEXT_CONFIDENCE: f32 = 99.0;

/// Characters in a pagination token issued by [`encode_page_token`].
const PAGE_TOKEN_LEN: usize = 32;

/// How many of those characters carry the offset, in hex, at the end of the token.
const PAGE_TOKEN_OFFSET_DIGITS: usize = 8;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Per-service state for com-amazonaws-textract-textract. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTextractTextractState {
    /// Every readable document this service has been handed, keyed by
    /// [`DocumentRef::identity_key`]. Bounded by [`MAX_STORED_DOCUMENTS`].
    pub documents: HashMap<String, DocumentRecord>,
    /// Asynchronous jobs, keyed by `JobId`. Bounded by [`MAX_STORED_JOBS`].
    pub jobs: HashMap<String, Job>,
    /// `"<job kind>:<ClientRequestToken>"` -> `JobId`, backing `Start*` idempotency.
    pub idempotency: HashMap<String, String>,
    /// Monotonic counter mixed into a minted `JobId` so two starts of the same
    /// document never collide.
    pub next_job_seq: u64,
}

impl ComAmazonawsTextractTextractState {
    /// The job a `ClientRequestToken` slot already names, and whether that job was
    /// started for the SAME request (same document, same `FeatureTypes`).
    ///
    /// One helper for both `Start*` handlers, so their idempotency semantics cannot
    /// drift apart: `Some((job_id, true))` is the idempotent answer, `Some((job_id,
    /// false))` is an `IdempotentParameterMismatchException`, `None` means the token is
    /// new.
    pub fn job_for_token(
        &self,
        slot: &str,
        document: &DocumentRef,
        feature_types: &[String],
    ) -> Option<(String, bool)> {
        self.idempotency
            .get(slot)
            .and_then(|job_id| self.jobs.get(job_id))
            .map(|job| {
                (
                    job.job_id.clone(),
                    job.document == *document && job.feature_types == *feature_types,
                )
            })
    }

    /// The stored content of `key`, if this service has already read that document.
    pub fn stored_content(&self, key: &str) -> Option<DocumentContent> {
        self.documents.get(key).map(|record| record.content.clone())
    }

    /// Store `content` for `reference` unless it is already stored, and hand back the
    /// stored copy.
    ///
    /// Idempotent: a second call with the same document returns the content stored by
    /// the first, which is what makes the sync and async answers for one document agree.
    pub fn register_document(
        &mut self,
        reference: DocumentRef,
        content: DocumentContent,
    ) -> DocumentContent {
        let key = reference.identity_key();
        if let Some(record) = self.documents.get(&key) {
            return record.content.clone();
        }
        // Bounded store: make room before inserting, oldest registration first.
        while self.documents.len() >= MAX_STORED_DOCUMENTS {
            let oldest = self
                .documents
                .iter()
                .min_by_key(|(_, record)| record.first_seen)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.documents.remove(&key);
                }
                None => break,
            }
        }
        self.documents.insert(
            key,
            DocumentRecord {
                reference,
                content: content.clone(),
                first_seen: SystemTime::now(),
            },
        );
        content
    }

    /// Store `job` under its own id, evicting the oldest job when the store is full.
    pub fn insert_job(&mut self, job: Job) {
        while self.jobs.len() >= MAX_STORED_JOBS {
            let oldest = self
                .jobs
                .iter()
                .min_by_key(|(_, job)| job.created_at)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => {
                    self.jobs.remove(&id);
                    // An idempotency entry naming an evicted job would answer with a
                    // JobId that no longer resolves, so it goes with the job.
                    self.idempotency.retain(|_, job_id| *job_id != id);
                }
                None => break,
            }
        }
        self.jobs.insert(job.job_id.clone(), job);
    }

    /// Mint a `JobId`: 64 hex characters, within `JobId`'s `1..64` length and its
    /// `^[a-zA-Z0-9-_]+$` pattern.
    pub fn mint_job_id(&mut self, document_key: &str) -> String {
        self.next_job_seq = self.next_job_seq.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        hex::encode(Sha256::digest(
            format!("{document_key}|{}|{nanos}", self.next_job_seq).as_bytes(),
        ))
    }
}

/// A document this service has read, and the text it holds.
#[derive(Debug, Clone)]
pub struct DocumentRecord {
    /// The document's identity.
    pub reference: DocumentRef,
    /// Its text content, page by page.
    pub content: DocumentContent,
    /// When this service first read it. Read by the store's eviction policy.
    pub first_seen: SystemTime,
}

/// Which asynchronous operation started a job. A `Get*` of the other kind must not
/// serve it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    /// Started by `StartDocumentTextDetection`, read by `GetDocumentTextDetection`.
    TextDetection,
    /// Started by `StartDocumentAnalysis`, read by `GetDocumentAnalysis`.
    DocumentAnalysis,
}

impl JobKind {
    /// A stable name, used to scope `ClientRequestToken` idempotency per operation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TextDetection => "TextDetection",
            Self::DocumentAnalysis => "DocumentAnalysis",
        }
    }
}

/// The terminal state a job reaches once the clock passes its due instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    /// The document was read and its blocks derived.
    Succeeded,
    /// The document could not be read; `Job::status_message` says why.
    Failed,
}

impl JobOutcome {
    /// The modelled `JobStatus` this outcome reports.
    pub const fn as_job_status(self) -> &'static str {
        match self {
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
        }
    }
}

/// An asynchronous Textract job and its stored result.
#[derive(Debug, Clone)]
pub struct Job {
    pub job_id: String,
    pub kind: JobKind,
    pub document: DocumentRef,
    pub feature_types: Vec<String>,
    pub client_request_token: Option<String>,
    pub job_tag: Option<String>,
    pub notification_channel: Option<NotificationChannel>,
    pub output_config: Option<OutputConfig>,
    pub kms_key_id: Option<String>,
    /// The terminal state the job settles into.
    pub outcome: JobOutcome,
    /// Why the job failed, for a `Failed` outcome.
    pub status_message: Option<String>,
    pub warnings: Vec<Warning>,
    /// When the job was started. Read by the job store's eviction policy.
    pub created_at: SystemTime,
    /// The instant the `IN_PROGRESS` -> terminal transition became due.
    pub settles_at: SystemTime,
    pub pages: i32,
    /// The job's whole result, computed once when the job was started.
    pub blocks: Vec<Block>,
    pub model_version: String,
}

impl Job {
    /// The job's status at `now`.
    ///
    /// Derived from the stored due instant rather than from a stored status, so a
    /// reporting call never has to write state to move the job along.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.settles_at {
            self.outcome.as_job_status()
        } else {
            "IN_PROGRESS"
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTextractTextractState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTextractTextractState>)
}

// ---------------------------------------------------------------------------
// Document identity
// ---------------------------------------------------------------------------

/// An S3 object, as a document source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct S3Ref {
    pub bucket: String,
    pub name: String,
    pub version: Option<String>,
}

/// The stable identity of a submitted document: an S3 object, or the digest of the
/// inline bytes. Everything stored or derived for a document keys off this.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocumentRef {
    pub s3: Option<S3Ref>,
    pub bytes_digest: Option<String>,
}

impl DocumentRef {
    /// The document's identity as one string.
    pub fn identity_key(&self) -> String {
        match (&self.s3, &self.bytes_digest) {
            (Some(s3), _) => match &s3.version {
                Some(version) => format!("s3://{}/{}?versionId={version}", s3.bucket, s3.name),
                None => format!("s3://{}/{}", s3.bucket, s3.name),
            },
            (None, Some(digest)) => format!("bytes:sha256:{digest}"),
            // Not constructible: both constructors below refuse an empty document.
            (None, None) => "bytes:sha256:none".to_string(),
        }
    }
}

/// The identity of a synchronous `Document`.
///
/// Textract requires exactly one of `Bytes` and `S3Object`: neither (and, equally,
/// both) violates the input constraint `InvalidParameterException` describes.
pub fn reference_for_document(document: &Document) -> Result<DocumentRef, String> {
    match (document.bytes.as_deref(), document.s3_object.as_ref()) {
        (None, None) => Err(
            "Document must have either its Bytes or its S3Object member set, but has neither"
                .to_string(),
        ),
        (Some(_), Some(_)) => Err(
            "Document must have either its Bytes or its S3Object member set, but not both"
                .to_string(),
        ),
        (Some(bytes), None) => Ok(DocumentRef {
            s3: None,
            bytes_digest: Some(hex::encode(Sha256::digest(bytes))),
        }),
        (None, Some(s3)) => Ok(DocumentRef {
            s3: Some(s3_ref(
                s3.bucket.as_deref(),
                s3.name.as_deref(),
                s3.version.as_deref(),
            )?),
            bytes_digest: None,
        }),
    }
}

/// The identity of an asynchronous `DocumentLocation`, which is always an S3 object.
pub fn reference_for_location(location: &DocumentLocation) -> Result<DocumentRef, String> {
    let s3 = location
        .s3_object
        .as_ref()
        .ok_or_else(|| "DocumentLocation must have its S3Object member set".to_string())?;
    Ok(DocumentRef {
        s3: Some(s3_ref(
            s3.bucket.as_deref(),
            s3.name.as_deref(),
            s3.version.as_deref(),
        )?),
        bytes_digest: None,
    })
}

fn s3_ref(
    bucket: Option<&str>,
    name: Option<&str>,
    version: Option<&str>,
) -> Result<S3Ref, String> {
    let bucket = bucket.filter(|b| !b.is_empty());
    let name = name.filter(|n| !n.is_empty());
    match (bucket, name) {
        (Some(bucket), Some(name)) => Ok(S3Ref {
            bucket: bucket.to_string(),
            name: name.to_string(),
            version: version.filter(|v| !v.is_empty()).map(ToString::to_string),
        }),
        (None, _) => Err("S3Object must have its Bucket member set".to_string()),
        (_, None) => Err("S3Object must have its Name member set".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Feature types
// ---------------------------------------------------------------------------

/// Which of the requested `FeatureTypes` change the derived block set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeatureSet {
    /// `FORMS`: add `KEY_VALUE_SET` blocks.
    pub forms: bool,
    /// `TABLES`: add `TABLE` and `CELL` blocks.
    pub tables: bool,
}

/// Why a `FeatureTypes` list cannot be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureError {
    /// The list is empty, which no analysis can satisfy.
    Empty,
    /// A value that is not a `FeatureType` at all.
    Unknown(String),
    /// A modelled `FeatureType` this emulator does not derive blocks for yet.
    NotDerived(String),
}

/// Read a `FeatureTypes` list.
///
/// `FORMS` and `TABLES` are derived; the other modelled values (`QUERIES`,
/// `SIGNATURES`, `LAYOUT`) are reported as [`FeatureError::NotDerived`] so the caller
/// answers 501 rather than a 200 that silently omits the blocks they asked for.
pub fn parse_features(values: &[String]) -> Result<FeatureSet, FeatureError> {
    if values.is_empty() {
        return Err(FeatureError::Empty);
    }
    let mut features = FeatureSet::default();
    for value in values {
        match value.as_str() {
            "FORMS" => features.forms = true,
            "TABLES" => features.tables = true,
            // `FEATURE_TYPE_VALUES` is the FeatureType enum from the Smithy model.
            other if FEATURE_TYPE_VALUES.contains(&other) => {
                return Err(FeatureError::NotDerived(other.to_string()))
            }
            other => return Err(FeatureError::Unknown(other.to_string())),
        }
    }
    Ok(features)
}

// ---------------------------------------------------------------------------
// Reading a document
// ---------------------------------------------------------------------------

/// A document's text, one entry per page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentContent {
    pub pages: Vec<String>,
}

impl DocumentContent {
    /// The page count `DocumentMetadata.Pages` reports.
    pub fn page_count(&self) -> i32 {
        i32::try_from(self.pages.len()).unwrap_or(i32::MAX)
    }

    /// `DocumentMetadata` for this document.
    pub fn metadata(&self) -> DocumentMetadata {
        DocumentMetadata {
            pages: Some(self.page_count()),
        }
    }
}

/// Why a document's text cannot be read. Each variant has exactly one modelled answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unreadable {
    /// The bytes are not a document this emulator can extract text from.
    /// Answered with `UnsupportedDocumentException`.
    UnsupportedFormat(String),
    /// The document is an S3 object whose bytes this emulator cannot obtain.
    /// Answered with `InvalidS3ObjectException`.
    S3Unreadable(String),
    /// The document, or the block list it would expand into, is over a documented or
    /// emulator-side limit. Answered with `DocumentTooLargeException`.
    TooLarge(String),
}

impl Unreadable {
    /// The reason, as a message for the modelled error or a job's `StatusMessage`.
    pub fn message(&self) -> &str {
        match self {
            Self::UnsupportedFormat(message)
            | Self::S3Unreadable(message)
            | Self::TooLarge(message) => message,
        }
    }
}

/// Read the text of the document `reference` names.
///
/// The ONLY source of a document's text. It extracts what is really there or refuses;
/// it never derives, guesses or fabricates content. See the module docs for the three
/// cases.
pub fn read_content(
    reference: &DocumentRef,
    inline: Option<&[u8]>,
) -> Result<DocumentContent, Unreadable> {
    if reference.s3.is_some() {
        return Err(Unreadable::S3Unreadable(format!(
            "Amazon Textract is unable to access {}: this emulator's Amazon S3 cannot serve \
             object bytes yet, so the document cannot be read",
            reference.identity_key()
        )));
    }
    let bytes = inline.unwrap_or_default();
    if bytes.len() > MAX_INLINE_DOCUMENT_BYTES {
        return Err(Unreadable::TooLarge(format!(
            "the document is {} bytes; the maximum size of a document provided as inline bytes \
             is {MAX_INLINE_DOCUMENT_BYTES} bytes",
            bytes.len()
        )));
    }
    let Some(text) = decode_text(bytes) else {
        return Err(Unreadable::UnsupportedFormat(
            "the document's bytes are not text, and this emulator runs no OCR engine, so no \
             text can be detected in it"
                .to_string(),
        ));
    };
    // `U+000C` FORM FEED is the page break in a text document.
    let pages: Vec<String> = text
        .split('\u{c}')
        .filter(|page| !page.trim().is_empty())
        .map(ToString::to_string)
        .collect();
    if pages.is_empty() {
        return Err(Unreadable::UnsupportedFormat(
            "the document holds no text".to_string(),
        ));
    }
    if pages.len() > MAX_DOCUMENT_PAGES {
        return Err(Unreadable::TooLarge(format!(
            "the document has {} pages; the maximum is {MAX_DOCUMENT_PAGES}",
            pages.len()
        )));
    }
    let estimated = estimated_block_count(&pages);
    if estimated > MAX_BLOCKS_PER_DOCUMENT {
        return Err(Unreadable::TooLarge(format!(
            "the document would expand into about {estimated} blocks; this emulator derives at \
             most {MAX_BLOCKS_PER_DOCUMENT} blocks for one document"
        )));
    }
    Ok(DocumentContent { pages })
}

/// An upper bound on the number of `Block` values these pages expand into, over EVERY
/// feature set: one PAGE per page; per line one LINE, at most two `KEY_VALUE_SET` and at
/// most one TABLE; per word one WORD and at most one CELL; plus one CELL per tab.
///
/// Checked before any block is built, so [`blocks`] does work bounded by
/// [`MAX_BLOCKS_PER_DOCUMENT`] whatever the caller submits.
fn estimated_block_count(pages: &[String]) -> usize {
    let mut total = pages.len();
    for page in pages {
        for line in page.lines().filter(|line| !line.trim().is_empty()) {
            total = total.saturating_add(4);
            total = total.saturating_add(line.split_whitespace().count().saturating_mul(2));
            total = total.saturating_add(line.matches('\t').count());
            if total > MAX_BLOCKS_PER_DOCUMENT {
                return total;
            }
        }
    }
    total
}

/// `Some(text)` when these bytes are a text document: valid UTF-8 carrying at least one
/// non-whitespace character and no control characters beyond the layout ones.
fn decode_text(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{c}'))
    {
        return None;
    }
    Some(text.to_string())
}

// ---------------------------------------------------------------------------
// Block derivation
// ---------------------------------------------------------------------------

/// Left and top margin of the page layout, as a page-relative fraction.
const PAGE_MARGIN: f32 = 0.05;
/// Width of one character cell, as a page-relative fraction.
const CHAR_WIDTH: f32 = 0.0075;
/// Height of a line of text, as a page-relative fraction.
const LINE_HEIGHT: f32 = 0.018;
/// Baseline-to-baseline distance, as a page-relative fraction.
const LINE_PITCH: f32 = 0.024;

/// A word of a line, with the character column it starts at.
struct WordLayout {
    column: usize,
    text: String,
    id: String,
}

/// A line of a page: its text, its words, and how analysis reads it.
struct LineLayout {
    id: String,
    index: usize,
    text: String,
    words: Vec<WordLayout>,
    /// Word counts per tab-separated cell, when the line is a table row.
    cells: Vec<usize>,
    /// Character column of the `:` that separates a label from its value.
    colon: Option<usize>,
}

/// A deterministic block id, rendered in the 8-4-4-4-12 form Textract's ids take.
///
/// Derived from the document's identity and the block's position in it, so the four read
/// paths over one document name the same block by the same id.
fn block_id(key: &str, kind: &str, path: &[usize]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hasher.update(b"|");
    hasher.update(kind.as_bytes());
    for part in path {
        hasher.update(format!("|{part}").as_bytes());
    }
    let d = hex::encode(hasher.finalize());
    format!(
        "{}-{}-{}-{}-{}",
        &d[0..8],
        &d[8..12],
        &d[12..16],
        &d[16..20],
        &d[20..32]
    )
}

fn bounding_box(line_index: usize, column: usize, chars: usize) -> BoundingBox {
    // The two bounds are computed independently and never derived from each other:
    // `f32::clamp` panics when its min exceeds its max, and rounding makes `1.0 - left`
    // fall just under `CHAR_WIDTH` once `left` saturates — which any line past the
    // page's character ceiling reaches, from a single caller-supplied document.
    let left = (PAGE_MARGIN + column as f32 * CHAR_WIDTH).clamp(0.0, 1.0 - CHAR_WIDTH);
    let top = (PAGE_MARGIN + line_index as f32 * LINE_PITCH).clamp(0.0, 1.0 - LINE_HEIGHT);
    let max_width = (1.0 - left).max(0.0);
    let width = (chars as f32 * CHAR_WIDTH).max(0.0).min(max_width);
    BoundingBox {
        width,
        height: LINE_HEIGHT,
        left,
        top,
    }
}

fn union(boxes: &[BoundingBox]) -> BoundingBox {
    let left = boxes.iter().map(|b| b.left).fold(f32::MAX, f32::min);
    let top = boxes.iter().map(|b| b.top).fold(f32::MAX, f32::min);
    let right = boxes.iter().map(|b| b.left + b.width).fold(0.0, f32::max);
    let bottom = boxes.iter().map(|b| b.top + b.height).fold(0.0, f32::max);
    BoundingBox {
        width: right - left,
        height: bottom - top,
        left,
        top,
    }
}

fn geometry(bbox: BoundingBox) -> Geometry {
    let polygon = vec![
        Point {
            x: bbox.left,
            y: bbox.top,
        },
        Point {
            x: bbox.left + bbox.width,
            y: bbox.top,
        },
        Point {
            x: bbox.left + bbox.width,
            y: bbox.top + bbox.height,
        },
        Point {
            x: bbox.left,
            y: bbox.top + bbox.height,
        },
    ];
    Geometry {
        bounding_box: Some(bbox),
        polygon: Some(polygon),
        rotation_angle: None,
    }
}

fn relationship(kind: &str, ids: Vec<String>) -> Relationship {
    Relationship {
        r#type: Some(kind.to_string()),
        ids: Some(ids),
    }
}

fn new_block(block_type: &str, id: String, page: i32) -> Block {
    Block {
        block_type: Some(block_type.to_string()),
        id: Some(id),
        page: Some(page),
        ..Block::default()
    }
}

/// The words of a line, each with the character column it starts at.
fn words_of(line: &str) -> Vec<(usize, String)> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut start = 0usize;
    for (column, ch) in line.chars().enumerate() {
        if ch.is_whitespace() {
            if !current.is_empty() {
                words.push((start, std::mem::take(&mut current)));
            }
        } else {
            if current.is_empty() {
                start = column;
            }
            current.push(ch);
        }
    }
    if !current.is_empty() {
        words.push((start, current));
    }
    words
}

/// Lay out one page's lines, numbering their words so every read path agrees on ids.
fn layout_page(key: &str, page_index: usize, page_text: &str) -> Vec<LineLayout> {
    page_text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(index, line)| {
            let cell_texts: Vec<&str> = line.split('\t').collect();
            let is_row = cell_texts.len() > 1;
            let words: Vec<WordLayout> = words_of(line)
                .into_iter()
                .enumerate()
                .map(|(wi, (column, text))| WordLayout {
                    column,
                    text,
                    id: block_id(key, "WORD", &[page_index, index, wi]),
                })
                .collect();
            // A tab-separated line is a table row; otherwise a `:` with words on both
            // sides makes it a labelled field.
            let colon = if is_row {
                None
            } else {
                line.chars().position(|c| c == ':').filter(|at| {
                    words.iter().any(|w| w.column <= *at) && words.iter().any(|w| w.column > *at)
                })
            };
            let cells = if is_row {
                cell_texts
                    .iter()
                    .map(|cell| cell.split_whitespace().count())
                    .collect()
            } else {
                Vec::new()
            };
            LineLayout {
                id: block_id(key, "LINE", &[page_index, index]),
                index,
                text: line.to_string(),
                words,
                cells,
                colon,
            }
        })
        .collect()
}

/// Every `Block` for a document's stored text, for the requested features.
///
/// With [`FeatureSet::default`] (which is what `DetectDocumentText` and
/// `GetDocumentTextDetection` pass) the result holds only `PAGE`, `LINE` and `WORD`
/// blocks, and by construction none of them carries `RowIndex`, `ColumnIndex`,
/// `RowSpan`, `ColumnSpan` or `EntityTypes` — the `Block` docs say those are not
/// returned by the text-detection operations.
///
/// Bounded work: `read_content` refuses a document whose block count would exceed
/// [`MAX_BLOCKS_PER_DOCUMENT`], so this only ever runs over content under that ceiling.
pub fn blocks(key: &str, content: &DocumentContent, features: FeatureSet) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    for (page_index, page_text) in content.pages.iter().enumerate() {
        let page_number = i32::try_from(page_index + 1).unwrap_or(i32::MAX);
        let lines = layout_page(key, page_index, page_text);

        let mut text_blocks: Vec<Block> = Vec::new();
        for line in &lines {
            let mut line_block = new_block("LINE", line.id.clone(), page_number);
            line_block.text = Some(line.text.trim().to_string());
            line_block.confidence = Some(EXTRACTED_TEXT_CONFIDENCE);
            line_block.geometry = Some(geometry(bounding_box(
                line.index,
                line.words.first().map_or(0, |w| w.column),
                line.text.trim_end().chars().count(),
            )));
            if !line.words.is_empty() {
                line_block.relationships = Some(vec![relationship(
                    "CHILD",
                    line.words.iter().map(|w| w.id.clone()).collect(),
                )]);
            }
            text_blocks.push(line_block);
            for word in &line.words {
                let mut word_block = new_block("WORD", word.id.clone(), page_number);
                word_block.text = Some(word.text.clone());
                word_block.confidence = Some(EXTRACTED_TEXT_CONFIDENCE);
                word_block.text_type = Some("PRINTED".to_string());
                word_block.geometry = Some(geometry(bounding_box(
                    line.index,
                    word.column,
                    word.text.chars().count(),
                )));
                text_blocks.push(word_block);
            }
        }

        let (form_blocks, form_ids) = if features.forms {
            form_blocks(key, page_index, page_number, &lines)
        } else {
            (Vec::new(), Vec::new())
        };
        let (table_blocks, table_ids) = if features.tables {
            table_blocks(key, page_index, page_number, &lines)
        } else {
            (Vec::new(), Vec::new())
        };

        // A PAGE block's CHILD relationship lists every top-level block on the page.
        let mut children: Vec<String> = lines.iter().map(|l| l.id.clone()).collect();
        children.extend(form_ids);
        children.extend(table_ids);

        let mut page_block = new_block("PAGE", block_id(key, "PAGE", &[page_index]), page_number);
        page_block.geometry = Some(geometry(BoundingBox {
            width: 1.0,
            height: 1.0,
            left: 0.0,
            top: 0.0,
        }));
        if !children.is_empty() {
            page_block.relationships = Some(vec![relationship("CHILD", children)]);
        }

        out.push(page_block);
        out.extend(text_blocks);
        out.extend(form_blocks);
        out.extend(table_blocks);
    }
    out
}

/// `KEY_VALUE_SET` blocks for the page's labelled fields, and their ids in page order.
fn form_blocks(
    key: &str,
    page_index: usize,
    page_number: i32,
    lines: &[LineLayout],
) -> (Vec<Block>, Vec<String>) {
    let mut out = Vec::new();
    let mut ids = Vec::new();
    for line in lines {
        let Some(colon) = line.colon else { continue };
        let key_words: Vec<&WordLayout> = line.words.iter().filter(|w| w.column <= colon).collect();
        let value_words: Vec<&WordLayout> =
            line.words.iter().filter(|w| w.column > colon).collect();
        if key_words.is_empty() || value_words.is_empty() {
            continue;
        }
        let key_id = block_id(key, "KEY", &[page_index, line.index]);
        let value_id = block_id(key, "VALUE", &[page_index, line.index]);

        let mut key_block = new_block("KEY_VALUE_SET", key_id.clone(), page_number);
        key_block.entity_types = Some(vec!["KEY".to_string()]);
        key_block.confidence = Some(EXTRACTED_TEXT_CONFIDENCE);
        key_block.geometry = Some(geometry(union(&word_boxes(line, &key_words))));
        key_block.relationships = Some(vec![
            relationship("VALUE", vec![value_id.clone()]),
            relationship("CHILD", key_words.iter().map(|w| w.id.clone()).collect()),
        ]);

        let mut value_block = new_block("KEY_VALUE_SET", value_id.clone(), page_number);
        value_block.entity_types = Some(vec!["VALUE".to_string()]);
        value_block.confidence = Some(EXTRACTED_TEXT_CONFIDENCE);
        value_block.geometry = Some(geometry(union(&word_boxes(line, &value_words))));
        value_block.relationships = Some(vec![relationship(
            "CHILD",
            value_words.iter().map(|w| w.id.clone()).collect(),
        )]);

        ids.push(key_id);
        ids.push(value_id);
        out.push(key_block);
        out.push(value_block);
    }
    (out, ids)
}

fn word_boxes(line: &LineLayout, words: &[&WordLayout]) -> Vec<BoundingBox> {
    words
        .iter()
        .map(|w| bounding_box(line.index, w.column, w.text.chars().count()))
        .collect()
}

/// `TABLE` and `CELL` blocks for the page's tab-separated rows, and the table ids.
///
/// A contiguous run of tab-separated lines is one table; a cell's `RowIndex` and
/// `ColumnIndex` are 1-based, as the `Block` docs state.
fn table_blocks(
    key: &str,
    page_index: usize,
    page_number: i32,
    lines: &[LineLayout],
) -> (Vec<Block>, Vec<String>) {
    let mut out = Vec::new();
    let mut ids = Vec::new();
    let mut table_index = 0usize;
    let mut cursor = 0usize;
    while cursor < lines.len() {
        if lines[cursor].cells.is_empty() {
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < lines.len() && !lines[cursor].cells.is_empty() {
            cursor += 1;
        }
        let run = &lines[start..cursor];
        let table_id = block_id(key, "TABLE", &[page_index, table_index]);
        let mut cell_blocks = Vec::new();
        let mut cell_ids = Vec::new();
        for (row, line) in run.iter().enumerate() {
            let mut word_cursor = 0usize;
            for (column, count) in line.cells.iter().copied().enumerate() {
                let cell_words: Vec<&WordLayout> = line
                    .words
                    .get(word_cursor..word_cursor + count)
                    .unwrap_or_default()
                    .iter()
                    .collect();
                word_cursor += count;
                let cell_id = block_id(key, "CELL", &[page_index, table_index, row, column]);
                let mut cell = new_block("CELL", cell_id.clone(), page_number);
                cell.row_index = Some(i32::try_from(row + 1).unwrap_or(i32::MAX));
                cell.column_index = Some(i32::try_from(column + 1).unwrap_or(i32::MAX));
                cell.row_span = Some(1);
                cell.column_span = Some(1);
                cell.confidence = Some(EXTRACTED_TEXT_CONFIDENCE);
                let boxes = word_boxes(line, &cell_words);
                cell.geometry = Some(geometry(if boxes.is_empty() {
                    bounding_box(line.index, column, 1)
                } else {
                    union(&boxes)
                }));
                if !cell_words.is_empty() {
                    cell.relationships = Some(vec![relationship(
                        "CHILD",
                        cell_words.iter().map(|w| w.id.clone()).collect(),
                    )]);
                }
                cell_ids.push(cell_id);
                cell_blocks.push(cell);
            }
        }
        let row_boxes: Vec<BoundingBox> = run
            .iter()
            .map(|line| bounding_box(line.index, 0, line.text.chars().count()))
            .collect();
        let mut table = new_block("TABLE", table_id.clone(), page_number);
        table.confidence = Some(EXTRACTED_TEXT_CONFIDENCE);
        table.geometry = Some(geometry(union(&row_boxes)));
        table.relationships = Some(vec![relationship("CHILD", cell_ids)]);

        ids.push(table_id);
        out.push(table);
        out.extend(cell_blocks);
        table_index += 1;
    }
    (out, ids)
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// An opaque cursor into a job's block list.
///
/// Stateless and self-verifying on purpose: `Get*` is a reporting call, so it must not
/// write state to hand out a cursor. The token binds its offset to the job with a
/// digest, so a token minted for one job cannot be replayed against another.
pub fn encode_page_token(job_id: &str, offset: usize) -> String {
    let digest = hex::encode(Sha256::digest(format!("{job_id}|{offset}").as_bytes()));
    let prefix = PAGE_TOKEN_LEN - PAGE_TOKEN_OFFSET_DIGITS;
    format!("{}{offset:08x}", &digest[..prefix])
}

/// The offset a token names, or `None` when it was not minted for this job.
///
/// TOTAL over any `&str`: `NextToken` is caller-controlled, so the token is rejected
/// unless it is ASCII of exactly the issued length, and every read of it uses a
/// non-panicking accessor.
pub fn decode_page_token(job_id: &str, token: &str) -> Option<usize> {
    if !token.is_ascii() || token.len() != PAGE_TOKEN_LEN {
        return None;
    }
    let digits = token.get(PAGE_TOKEN_LEN - PAGE_TOKEN_OFFSET_DIGITS..)?;
    let offset = usize::from_str_radix(digits, 16).ok()?;
    (encode_page_token(job_id, offset) == token).then_some(offset)
}

/// One page of `blocks`, plus the token for the next page (`None` on the last one).
///
/// `None` when `max_results` is present but not a usable page size, which the caller
/// refuses with `InvalidParameterException` rather than quietly substituting a default.
pub fn page_of_blocks(
    job_id: &str,
    blocks: &[Block],
    max_results: Option<i32>,
    offset: usize,
) -> Option<(Vec<Block>, Option<String>)> {
    let limit = match max_results {
        None => DEFAULT_MAX_RESULTS,
        Some(requested) => usize::try_from(requested).ok().filter(|m| *m > 0)?,
    };
    let end = offset.saturating_add(limit).min(blocks.len());
    let page = blocks.get(offset..end).unwrap_or_default().to_vec();
    let next = (end < blocks.len()).then(|| encode_page_token(job_id, end));
    Some((page, next))
}
