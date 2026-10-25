// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice service.
//!
//! The media-pipeline resources this service mints live here, in ONE store behind
//! the engine's per-service lock. `MediaCapturePipeline` and the media insights
//! pipeline CONFIGURATIONS are the kinds wired so far; the sibling kinds
//! (concatenation, live-connector, stream and insights pipelines, and Kinesis
//! video stream pools) get their own collections beside [`CapturePipeline`] and
//! [`InsightsConfiguration`] as their operations are implemented, and
//! `GetMediaPipeline`/`ListMediaPipelines` read over all of them.
//!
//! Besides the store this module holds the few derivations every pipeline handler
//! shares — the ARN shape, the signed region, the modelled error shapes and the
//! pagination cursor — so no two handlers spell one of them differently.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    BadRequestException, ChimeSdkMeetingConfiguration, MediaCapturePipeline,
    MediaCapturePipelineSummary, MediaInsightsPipelineConfiguration,
    MediaInsightsPipelineConfigurationElement, MediaPipelineSummary, NotFoundException,
    RealTimeAlertConfiguration, ResourceLimitExceededException, ServiceFailureException,
    SseAwsKeyManagementParams, Tag,
};

/// The `MediaPipelineStatus` value a freshly created pipeline carries.
/// From `smithy_query(query="shape", shape="MediaPipelineStatus")`:
/// `Initializing | InProgress | Failed | Stopping | Stopped | Paused | NotStarted`.
pub const STATUS_INITIALIZING: &str = "Initializing";

/// The `MediaPipelineStatus` value a pipeline reaches once it is running.
pub const STATUS_IN_PROGRESS: &str = "InProgress";

/// The `ErrorCode` enum value a `BadRequestException` carries.
/// From `smithy_query(query="shape", shape="ErrorCode")`.
const ERROR_CODE_BAD_REQUEST: &str = "BadRequest";

/// The `ErrorCode` enum value a `NotFoundException` carries.
const ERROR_CODE_NOT_FOUND: &str = "NotFound";

/// The `ErrorCode` enum value a `ServiceFailureException` carries.
const ERROR_CODE_SERVICE_FAILURE: &str = "ServiceFailure";

/// The `ErrorCode` enum value a `ResourceLimitExceededException` carries.
const ERROR_CODE_RESOURCE_LIMIT_EXCEEDED: &str = "ResourceLimitExceeded";

/// The most media capture pipelines this store holds at once.
///
/// This is the store's RESOURCE BOUND: the record count grows once per
/// `CreateMediaCapturePipeline`, so without a maximum a caller could grow resident
/// memory without limit (per-record size is already bounded by the model's
/// `@length` constraints) and make the per-read `settle_clock` sweep arbitrarily
/// long while holding the shared lock. It is also the more faithful answer: the
/// real service enforces a per-account quota and declares
/// `ResourceLimitExceededException` (HTTP 400) on this operation for it.
///
/// The number itself is CHOSEN, not quoted: AWS's quota for concurrent media
/// pipelines is not in the model and is adjustable per account, so this is the
/// emulator's own ceiling, set far above any realistic client workflow.
pub const MAX_CAPTURE_PIPELINES: usize = 1_000;

/// The most media insights pipeline configurations this store holds at once.
///
/// The same RESOURCE BOUND as [`MAX_CAPTURE_PIPELINES`], for the collection
/// `CreateMediaInsightsPipelineConfiguration` grows: the real service enforces a
/// per-account quota and declares `ResourceLimitExceededException` (HTTP 400) on
/// that operation for it. The number itself is CHOSEN, not quoted — AWS's quota is
/// not in the model and is adjustable per account.
pub const MAX_INSIGHTS_CONFIGURATIONS: usize = 1_000;

/// The most `Elements` one stored media insights pipeline configuration may carry.
///
/// This is the store's PER-RECORD bound, and it is what makes
/// [`MAX_INSIGHTS_CONFIGURATIONS`] a memory bound rather than only a record count: the
/// element list is the one caller-chosen size inside the record that the model leaves
/// open. `smithy_query(query="shape", shape="MediaInsightsPipelineConfigurationElements")`
/// reports a list with no `@length`, so no boundary validator can refuse an arbitrarily
/// long one, while every member INSIDE an element is `@length`-bounded by the model (the
/// ARNs at 1024, the vocabulary names at 200–3000, `PostCallAnalyticsSettings`' strings at
/// 4096, `CallAnalyticsStreamCategories` at 20 entries) — so capping the COUNT caps the
/// bytes one record can hold, and the two constants together cap what this store retains.
///
/// The number is CHOSEN, not quoted: AWS publishes no element quota in the model. It is
/// set far above any realistic configuration — the model declares nine element types
/// (`MediaInsightsPipelineConfigurationElementType`), and a real configuration pairs a few
/// processors with a few sinks — so no faithful client reaches it.
pub const MAX_INSIGHTS_CONFIGURATION_ELEMENTS: usize = 64;

/// The region an unsigned request is attributed to; the emulator's default.
const DEFAULT_REGION: &str = "us-east-1";

/// How long after `CreateMediaCapturePipeline` the `Initializing -> InProgress`
/// transition falls due.
///
/// `Initializing` is a NON-TERMINAL `MediaPipelineStatus`, so the real service
/// leaves that transition to its own clock rather than to a caller's call. AWS
/// publishes no duration for it, so this span is CHOSEN, and kept inside the
/// 250–750 ms window this project fixes for a chosen transition: the span is never
/// zero, so a caller reading straight back still observes `Initializing`.
pub const INITIALIZING_TO_IN_PROGRESS: Duration = Duration::from_millis(500);

/// A `MediaPipelineId`: a lowercase v4 GUID, matching the model's `GuidString`
/// pattern `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`.
///
/// Minted from a counter the STORE advances (see
/// [`ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState::mint_media_pipeline_id`]),
/// never from the request, so two creates of the same pipeline yield two ids. The
/// RFC 4122 version and variant bits are set, so the value is a well-formed v4
/// UUID and not merely 32 hex digits.
fn format_guid(high: u64, low: u64) -> String {
    let high = (high & 0xFFFF_FFFF_FFFF_0FFF) | 0x0000_0000_0000_4000;
    let low = (low & 0x3FFF_FFFF_FFFF_FFFF) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (high >> 32) as u32,
        ((high >> 16) & 0xFFFF) as u16,
        (high & 0xFFFF) as u16,
        ((low >> 48) & 0xFFFF) as u16,
        low & 0x0000_FFFF_FFFF_FFFF,
    )
}

/// Wall-clock nanoseconds since the epoch, forced odd, used only to seed the id
/// counter on first use so two emulator runs do not mint the same first id.
fn seed_from_clock() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(1, |elapsed| elapsed.as_nanos() as u64)
        | 1
}

/// SplitMix64: a bijection on `u64`, so advancing the id counter through it never
/// revisits a value before the counter wraps.
fn splitmix64(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A media pipeline's ARN, in this service's ARN namespace (`chime`, per the
/// model's `aws.api#service`): `arn:aws:chime:<region>:<account>:media-pipeline/<id>`.
pub fn media_pipeline_arn(region: &str, media_pipeline_id: &str) -> String {
    ::engine::emulator_arn(
        "chime",
        region,
        &format!("media-pipeline/{media_pipeline_id}"),
    )
}

/// A media insights pipeline configuration's ARN, in the same (`chime`) namespace:
/// `arn:aws:chime:<region>:<account>:media-insights-pipeline-configuration/<name>`.
///
/// The resource part carries the configuration NAME, which is the key that
/// identifies the configuration in an account: the model documents
/// `Identifier` — the `@httpLabel` of the get, update and delete — as "the name and
/// ARN of the media insights pipeline configuration"
/// (`smithy_query(query="doc",shape="DeleteMediaInsightsPipelineConfigurationRequest",
/// member="Identifier")`), so the two forms name one resource. The minted
/// `MediaInsightsPipelineConfigurationId` is reported beside it as its own member.
pub fn media_insights_pipeline_configuration_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        "chime",
        region,
        &format!("media-insights-pipeline-configuration/{name}"),
    )
}

/// The region the caller signed this request for, from the SigV4 credential
/// scope; [`DEFAULT_REGION`] for a request carrying no scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A wall-clock instant as the modelled `Iso8601Timestamp`, truncated to the
/// millisecond the service's `date-time` rendering carries.
pub fn timestamp(at: SystemTime) -> DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::from_secs_and_nanos(
        since_epoch.as_secs() as i64,
        since_epoch.subsec_millis() * 1_000_000,
    )
}

/// The source of every `RequestId` this service reports.
///
/// Process-wide rather than a field of the state struct on purpose: a reporting
/// operation puts a `RequestId` on the error it answers with, and a read must not
/// write to the store it reports on.
static REQUEST_IDS: AtomicU64 = AtomicU64::new(1);

/// The `RequestId` to report on one response: a well-formed RFC 4122 v4 UUID string,
/// which is the shape an AWS request id has.
///
/// Every error shape of this service carries a `RequestId` member
/// (`smithy_query(query="shape",shape="NotFoundException")`), and AWS fills it on
/// every response: it is what a caller quotes when reporting a failed call. The value
/// is the monotonic counter rather than randomness, so it is unique per response
/// without being derived from the request.
pub fn next_request_id() -> String {
    let n = REQUEST_IDS.fetch_add(1, Ordering::Relaxed);
    format!("00000000-0000-4000-8000-{n:012x}")
}

/// The modelled 400 `BadRequestException`, carrying `message`.
pub fn bad_request(message: impl Into<String>) -> BadRequestException {
    BadRequestException {
        code: Some(ERROR_CODE_BAD_REQUEST.to_string()),
        message: Some(message.into()),
        request_id: Some(next_request_id()),
    }
}

/// The modelled 404 `NotFoundException`, carrying `message`.
pub fn not_found(message: impl Into<String>) -> NotFoundException {
    NotFoundException {
        code: Some(ERROR_CODE_NOT_FOUND.to_string()),
        message: Some(message.into()),
        request_id: Some(next_request_id()),
    }
}

/// The modelled 500 `ServiceFailureException`, carrying `message`. Returned when
/// the engine hands back no store, which is the one condition a handler cannot
/// serve through.
pub fn service_failure(message: impl Into<String>) -> ServiceFailureException {
    ServiceFailureException {
        code: Some(ERROR_CODE_SERVICE_FAILURE.to_string()),
        message: Some(message.into()),
        request_id: Some(next_request_id()),
    }
}

/// The modelled 400 `ResourceLimitExceededException`, carrying `message`.
pub fn resource_limit_exceeded(message: impl Into<String>) -> ResourceLimitExceededException {
    ResourceLimitExceededException {
        code: Some(ERROR_CODE_RESOURCE_LIMIT_EXCEEDED.to_string()),
        message: Some(message.into()),
        request_id: Some(next_request_id()),
    }
}

/// The `message` a refused pagination token is reported with.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "The next-token value is not a valid pagination token.";

/// A pagination token this service did not issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPageToken;

/// The store already holds [`MAX_CAPTURE_PIPELINES`] pipelines, so it refused to
/// grow. A create reports it as the modelled `ResourceLimitExceededException`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturePipelineLimitReached;

/// Why the store refused to persist a media insights pipeline configuration. A
/// create reports each as the modelled error named here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightsConfigurationRefused {
    /// The store already holds [`MAX_INSIGHTS_CONFIGURATIONS`] configurations — the
    /// modelled `ResourceLimitExceededException` (HTTP 400).
    LimitReached,
    /// A configuration with that name already exists. The name is what identifies a
    /// configuration in an account (it is one of the two forms of `Identifier`), so
    /// a second one under the same name is refused — and since the model declares no
    /// `ConflictException` for the create, it is the modelled
    /// `BadRequestException` (HTTP 400).
    NameInUse,
    /// The configuration carries more than [`MAX_INSIGHTS_CONFIGURATION_ELEMENTS`]
    /// elements; the payload is how many it carried. The model puts no `@length` on the
    /// element list, so this is the one caller-chosen size in the record the boundary
    /// cannot refuse, and the store is where it is admitted or not — reported as the
    /// modelled `BadRequestException` (HTTP 400), the input-restriction failure it is.
    TooManyElements(usize),
}

/// How a refusal of too many elements is worded: built from the request (the count it
/// sent and the maximum), stated ONCE here so the admission check and the handler's
/// pre-lock check cannot word the same refusal differently.
pub fn too_many_elements_message(count: usize) -> String {
    format!(
        "Elements carries {count} elements, more than the {MAX_INSIGHTS_CONFIGURATION_ELEMENTS} \
         a media insights pipeline configuration accepts."
    )
}

/// Encode the page cursor — the creation sequence of the last item returned — as
/// the opaque token a caller echoes back in `next-token`.
///
/// The cursor is the creation SEQUENCE and not the last item's id, so deleting the
/// item a token names does not strand the pages after it. Rendered as fixed-width
/// hex so the token is a flat, protocol-safe string with no internal structure a
/// caller could rely on.
fn encode_page_token(after_seq: u64) -> String {
    format!("{after_seq:016x}")
}

/// Recover the cursor from a token [`encode_page_token`] produced; `Err` for
/// anything else, which the caller reports as a `BadRequestException`.
fn decode_page_token(token: &str) -> Result<u64, InvalidPageToken> {
    if token.len() != 16 {
        return Err(InvalidPageToken);
    }
    u64::from_str_radix(token, 16).map_err(|_| InvalidPageToken)
}

/// One persisted media capture pipeline: everything `CreateMediaCapturePipeline`
/// minted or stored, which is what every later read renders from.
#[derive(Debug, Clone)]
pub struct CapturePipeline {
    /// Creation sequence, assigned by [`ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState::insert_capture_pipeline`].
    /// Monotonic and never reused, so it is a stable pagination cursor.
    pub seq: u64,
    pub media_pipeline_id: String,
    pub media_pipeline_arn: String,
    pub source_type: String,
    pub source_arn: String,
    pub sink_type: String,
    pub sink_arn: String,
    pub sink_iam_role_arn: Option<String>,
    pub status: String,
    pub created_timestamp: DateTime,
    pub updated_timestamp: DateTime,
    pub chime_sdk_meeting_configuration: Option<ChimeSdkMeetingConfiguration>,
    pub sse_aws_key_management_params: Option<SseAwsKeyManagementParams>,
    pub tags: Vec<Tag>,
    /// When the `Initializing -> InProgress` transition the clock owes falls due,
    /// or `None` once it has been materialised. Stored as an instant rather than
    /// slept for, so the transition is owed whether or not anybody calls.
    pub in_progress_due_at: Option<SystemTime>,
}

impl CapturePipeline {
    /// This pipeline as the modelled `MediaCapturePipeline` every read returns.
    pub fn as_modeled(&self) -> MediaCapturePipeline {
        MediaCapturePipeline {
            media_pipeline_id: Some(self.media_pipeline_id.clone()),
            media_pipeline_arn: Some(self.media_pipeline_arn.clone()),
            source_type: Some(self.source_type.clone()),
            source_arn: Some(self.source_arn.clone()),
            status: Some(self.status.clone()),
            sink_type: Some(self.sink_type.clone()),
            sink_arn: Some(self.sink_arn.clone()),
            created_timestamp: Some(self.created_timestamp),
            updated_timestamp: Some(self.updated_timestamp),
            chime_sdk_meeting_configuration: self.chime_sdk_meeting_configuration.clone(),
            sse_aws_key_management_params: self.sse_aws_key_management_params.clone(),
            sink_iam_role_arn: self.sink_iam_role_arn.clone(),
        }
    }

    /// This pipeline as `ListMediaCapturePipelines` summarises it.
    pub fn as_capture_summary(&self) -> MediaCapturePipelineSummary {
        MediaCapturePipelineSummary {
            media_pipeline_id: Some(self.media_pipeline_id.clone()),
            media_pipeline_arn: Some(self.media_pipeline_arn.clone()),
        }
    }

    /// This pipeline as `ListMediaPipelines` summarises it.
    pub fn as_pipeline_summary(&self) -> MediaPipelineSummary {
        MediaPipelineSummary {
            media_pipeline_id: Some(self.media_pipeline_id.clone()),
            media_pipeline_arn: Some(self.media_pipeline_arn.clone()),
        }
    }
}

/// One persisted media insights pipeline configuration: everything
/// `CreateMediaInsightsPipelineConfiguration` minted or stored, which is what every
/// later read renders from.
///
/// The processors and sinks (`elements`) and the alert rules are kept exactly as the
/// caller submitted them: they are the caller's configuration, which the service
/// stores and reports back, not something it derives.
#[derive(Debug, Clone)]
pub struct InsightsConfiguration {
    /// Creation sequence, assigned by
    /// [`ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState::insert_insights_configuration`].
    /// Monotonic and never reused, so it is a stable listing order.
    pub seq: u64,
    pub name: String,
    pub arn: String,
    pub id: String,
    pub resource_access_role_arn: String,
    pub real_time_alert_configuration: Option<RealTimeAlertConfiguration>,
    pub elements: Vec<MediaInsightsPipelineConfigurationElement>,
    pub tags: Vec<Tag>,
    pub created_timestamp: DateTime,
    pub updated_timestamp: DateTime,
}

impl InsightsConfiguration {
    /// Whether `identifier` names this configuration. The model documents the
    /// `Identifier` label as the configuration's NAME or its ARN, so both forms
    /// resolve here and nothing else does.
    pub fn is_named_by(&self, identifier: &str) -> bool {
        self.name == identifier || self.arn == identifier
    }

    /// This configuration as the modelled `MediaInsightsPipelineConfiguration`
    /// every read returns.
    pub fn as_modeled(&self) -> MediaInsightsPipelineConfiguration {
        MediaInsightsPipelineConfiguration {
            media_insights_pipeline_configuration_name: Some(self.name.clone()),
            media_insights_pipeline_configuration_arn: Some(self.arn.clone()),
            resource_access_role_arn: Some(self.resource_access_role_arn.clone()),
            real_time_alert_configuration: self.real_time_alert_configuration.clone(),
            elements: Some(self.elements.clone()),
            media_insights_pipeline_configuration_id: Some(self.id.clone()),
            created_timestamp: Some(self.created_timestamp),
            updated_timestamp: Some(self.updated_timestamp),
        }
    }
}

/// Per-service state for com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState {
    /// Every created-and-not-deleted capture pipeline, in creation order, so
    /// paging over it is stable.
    capture_pipelines: Vec<CapturePipeline>,
    /// `ClientRequestToken -> MediaPipelineId`, so a replayed create answers with
    /// the pipeline the first one minted instead of minting a second.
    ///
    /// Bounded by `capture_pipelines`, and so by [`MAX_CAPTURE_PIPELINES`]: an
    /// entry is written ONLY by [`Self::insert_capture_pipeline`], beside the
    /// pipeline it minted, and dropped by [`Self::remove_capture_pipeline`], so
    /// every entry names a live pipeline and ids are unique — the map can never
    /// hold more entries than there are records.
    request_tokens: HashMap<String, String>,
    /// Every created-and-not-deleted media insights pipeline configuration, in
    /// creation order.
    insights_configurations: Vec<InsightsConfiguration>,
    /// `ClientRequestToken -> the configuration NAME`, so a replayed create answers
    /// with the configuration the first one minted instead of refusing the name.
    ///
    /// Bounded exactly as [`Self::request_tokens`] is: an entry is written ONLY by
    /// [`Self::insert_insights_configuration`], beside the configuration it minted,
    /// and dropped by [`Self::remove_insights_configuration`].
    insights_configuration_tokens: HashMap<String, String>,
    /// The next creation sequence to hand out.
    next_seq: u64,
    /// The id counter, seeded from the clock on first use and advanced through
    /// `splitmix64` once per minted id, so no id is ever a function of a request.
    id_seed: u64,
}

impl ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState {
    /// Mint a fresh `MediaPipelineId`: a lowercase v4 GUID matching the model's
    /// `GuidString` pattern, drawn from this store's own counter.
    pub fn mint_media_pipeline_id(&mut self) -> String {
        if self.id_seed == 0 {
            self.id_seed = seed_from_clock();
        }
        self.id_seed = splitmix64(self.id_seed);
        let high = self.id_seed;
        let low = splitmix64(high ^ 0xA076_1D64_78BD_642F);
        format_guid(high, low)
    }

    /// Materialise every transition the clock already owes as of `now`.
    ///
    /// A reporting operation calls this before it answers: the change was due at
    /// `due` whether or not anybody called, so the timestamp recorded is `due` and
    /// not the instant of the call that happened to observe it.
    pub fn settle_clock(&mut self, now: SystemTime) {
        for pipeline in &mut self.capture_pipelines {
            let Some(due) = pipeline.in_progress_due_at else {
                continue;
            };
            if now < due {
                continue;
            }
            pipeline.status = STATUS_IN_PROGRESS.to_string();
            pipeline.updated_timestamp = timestamp(due);
            pipeline.in_progress_due_at = None;
        }
    }

    /// Persist `pipeline` (and, when the create carried one, the
    /// `ClientRequestToken` that minted it), assign its creation sequence, and hand
    /// back the STORED record so a create renders its response from the store.
    ///
    /// THE STORE'S BOUND: refuses before growing EITHER collection once the store
    /// already holds [`MAX_CAPTURE_PIPELINES`] records. Both growth sites are here,
    /// behind this one check, so the cap dominates them.
    pub fn insert_capture_pipeline(
        &mut self,
        mut pipeline: CapturePipeline,
        request_token: Option<String>,
    ) -> Result<&CapturePipeline, CapturePipelineLimitReached> {
        if self.capture_pipelines.len() >= MAX_CAPTURE_PIPELINES {
            return Err(CapturePipelineLimitReached);
        }
        pipeline.seq = self.next_seq;
        self.next_seq += 1;
        if let Some(token) = request_token {
            self.request_tokens
                .insert(token, pipeline.media_pipeline_id.clone());
        }
        self.capture_pipelines.push(pipeline);
        Ok(self
            .capture_pipelines
            .last()
            .expect("a pipeline was just pushed"))
    }

    /// The capture pipeline with this id, or `None` when none was created under it
    /// or it has been deleted.
    pub fn capture_pipeline(&self, media_pipeline_id: &str) -> Option<&CapturePipeline> {
        self.capture_pipelines
            .iter()
            .find(|pipeline| pipeline.media_pipeline_id == media_pipeline_id)
    }

    /// Remove the capture pipeline with this id, with its idempotency record.
    /// `false` when there was nothing to remove.
    pub fn remove_capture_pipeline(&mut self, media_pipeline_id: &str) -> bool {
        let before = self.capture_pipelines.len();
        self.capture_pipelines
            .retain(|pipeline| pipeline.media_pipeline_id != media_pipeline_id);
        if self.capture_pipelines.len() == before {
            return false;
        }
        self.request_tokens.retain(|_, id| id != media_pipeline_id);
        true
    }

    /// The pipeline a previous create minted under `token`, for the idempotent
    /// replay. `None` when the token is new, or named a pipeline since deleted.
    pub fn capture_pipeline_for_request_token(&self, token: &str) -> Option<&CapturePipeline> {
        let id = self.request_tokens.get(token)?;
        self.capture_pipeline(id)
    }

    /// One page of the capture pipelines, in creation order: the items, and the
    /// token to resume after them — present ONLY when at least one pipeline
    /// remains beyond the page.
    ///
    /// `next_token` resumes after the cursor it carries; `max_results` truncates.
    /// An absent `max_results` returns everything remaining — as does a
    /// non-positive one, which the modelled 1..100 range already refuses at the
    /// boundary; treating it as "no limit" rather than as a page size of zero keeps
    /// the function total, so no caller can ever be handed an empty page with no
    /// resume token while records remain.
    pub fn page_capture_pipelines(
        &self,
        next_token: Option<&str>,
        max_results: Option<i32>,
    ) -> Result<(Vec<&CapturePipeline>, Option<String>), InvalidPageToken> {
        let after = match next_token.filter(|token| !token.is_empty()) {
            Some(token) => Some(decode_page_token(token)?),
            None => None,
        };
        let remaining: Vec<&CapturePipeline> = self
            .capture_pipelines
            .iter()
            .filter(|pipeline| after.is_none_or(|cursor| pipeline.seq > cursor))
            .collect();
        let limit = max_results
            .filter(|max| *max > 0)
            .map_or(remaining.len(), |max| max as usize);
        let more = remaining.len() > limit;
        let page: Vec<&CapturePipeline> = remaining.into_iter().take(limit).collect();
        let token = if more {
            page.last().map(|pipeline| encode_page_token(pipeline.seq))
        } else {
            None
        };
        Ok((page, token))
    }

    /// Persist `configuration` (and, when the create carried one, the
    /// `ClientRequestToken` that minted it), assign its creation sequence, and hand
    /// back the STORED record so a create renders its response from the store.
    ///
    /// Refuses before growing either collection once the store already holds
    /// [`MAX_INSIGHTS_CONFIGURATIONS`] records, and refuses a name another live
    /// configuration already carries: the name is one of the two forms of the
    /// `Identifier` every later read, update and delete resolves, so two
    /// configurations under one name would leave that lookup ambiguous.
    pub fn insert_insights_configuration(
        &mut self,
        mut configuration: InsightsConfiguration,
        request_token: Option<String>,
    ) -> Result<&InsightsConfiguration, InsightsConfigurationRefused> {
        // The record's own caller-chosen size, checked BEFORE any mutation: the element
        // list is the one part of the record the model leaves unbounded, and this is the
        // store's growth site, so admission is decided here as well as at the handler's
        // pre-lock check — the constant cannot fall out of use while this is the gate.
        if configuration.elements.len() > MAX_INSIGHTS_CONFIGURATION_ELEMENTS {
            return Err(InsightsConfigurationRefused::TooManyElements(
                configuration.elements.len(),
            ));
        }
        if self.insights_configurations.len() >= MAX_INSIGHTS_CONFIGURATIONS {
            return Err(InsightsConfigurationRefused::LimitReached);
        }
        if self
            .insights_configurations
            .iter()
            .any(|stored| stored.name == configuration.name)
        {
            return Err(InsightsConfigurationRefused::NameInUse);
        }
        configuration.seq = self.next_seq;
        self.next_seq += 1;
        if let Some(token) = request_token {
            self.insights_configuration_tokens
                .insert(token, configuration.name.clone());
        }
        self.insights_configurations.push(configuration);
        Ok(self
            .insights_configurations
            .last()
            .expect("a configuration was just pushed"))
    }

    /// The configuration `identifier` names — its own name or its ARN — or `None`
    /// when none was created under it or it has been deleted.
    pub fn insights_configuration(&self, identifier: &str) -> Option<&InsightsConfiguration> {
        self.insights_configurations
            .iter()
            .find(|configuration| configuration.is_named_by(identifier))
    }

    /// The configuration a previous create minted under `token`, for the idempotent
    /// replay. `None` when the token is new, or named a configuration since deleted.
    pub fn insights_configuration_for_request_token(
        &self,
        token: &str,
    ) -> Option<&InsightsConfiguration> {
        let name = self.insights_configuration_tokens.get(token)?;
        self.insights_configuration(name)
    }

    /// Remove the configuration `identifier` names, with its idempotency record.
    /// `false` when there was nothing to remove.
    pub fn remove_insights_configuration(&mut self, identifier: &str) -> bool {
        let Some(name) = self
            .insights_configuration(identifier)
            .map(|configuration| configuration.name.clone())
        else {
            return false;
        };
        self.insights_configurations
            .retain(|configuration| configuration.name != name);
        self.insights_configuration_tokens
            .retain(|_, minted| *minted != name);
        true
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState>)
}
