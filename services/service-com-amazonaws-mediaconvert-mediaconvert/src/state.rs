// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediaconvert-mediaconvert service.
//!
//! The transcoding-job control plane: the `queues` a job can be sent to, the `jobs`
//! themselves and the account's input `policy`. Every response the handlers in
//! `src/ops/` build is projected from a record stored here, so a value a caller reads
//! back is the value some earlier call persisted.
//!
//! The account policy is a SINGLETON — the API has no policy name or id, so `PutPolicy`
//! replaces one record and `DeletePolicy` drops it; it is not caller-driven growth.
//!
//! # Bounds on caller-driven growth and per-request work
//!
//! Nothing here grows without a ceiling and nothing here walks an unbounded set:
//!
//! * `queues` is capped at [`MAX_QUEUES`] and `jobs` at [`MAX_JOBS`]; a create past the
//!   cap is refused with the modelled `ServiceQuotaExceededException` (HTTP 402) instead
//!   of growing the store. `idempotency` holds at most one entry per stored job, so it
//!   is bounded by `MAX_JOBS` too, and `endpoints` \u2014 one account endpoint per region a
//!   request was signed for \u2014 is capped at [`MAX_ENDPOINTS`] the same way.
//! * `job_templates` is seeded once, at state construction, with the built-in system
//!   templates and is not grown by any request path yet, so its size is a constant;
//!   the cap on caller-created templates belongs with `CreateJobTemplate`, which is
//!   still a scaffold stub.
//! * Every lookup is an O(log n) `BTreeMap` probe; no handler clones the whole store.
//! * [`ComAmazonawsMediaconvertMediaconvertState::settle`] — the only work a reporting
//!   operation does — touches ONLY the jobs whose due instant has already passed, by
//!   draining the `pending` index that is ordered by that instant. It costs
//!   O(k log n) in the number k of jobs actually settling, which is zero on the
//!   overwhelming majority of calls; it is never a sweep of the job set.
//! * `ListJobs` is the one operation that orders the jobs, over at most `MAX_JOBS`
//!   records, and returns at most one page of twenty. `ListJobTemplates` orders the
//!   job templates the same way and pages them the same way.
//!
//! # No panic on caller-supplied input
//!
//! Every fallible step on a request path returns instead of unwrapping: the state lock
//! is taken through `engine`'s poison-recovering `ServiceState::lock`, every map access
//! is `get`/`get_mut` with a let-else (never an index), every slice is taken with a
//! `min`-clamped range, `duration_since(UNIX_EPOCH)` falls back with `map_or`, and the
//! opaque pagination cursor is decoded with `ok()?` so a hostile `nextToken` becomes a
//! modelled `BadRequestException`. There is no `unwrap`, `expect`, `panic!` or slice
//! index anywhere in this module or in the handlers that use it.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AccelerationSettings, HopDestination, Job, JobSettings, JobTemplate, JobTemplateSettings,
    Policy, Queue, ReservationPlan, ReservationPlanSettings, Timing,
};

/// The built-in queue a job with no `Queue` member is sent to.
///
/// The `Queue` shape's own documentation: "If you don't specify a queue, the service
/// sends all jobs through the default queue."
pub const DEFAULT_QUEUE_NAME: &str = "Default";

/// `Type` values (`smithy_query(query="shape",shape="Type")`), the two kinds every
/// named MediaConvert resource comes in. `JobTemplate$Type`'s doc: "A job template can
/// be of two types: system or custom. System or built-in job templates can't be modified
/// or deleted by the user."
pub const TYPE_SYSTEM: &str = "SYSTEM";
/// See [`TYPE_SYSTEM`]: the type of a resource a caller created.
pub const TYPE_CUSTOM: &str = "CUSTOM";

/// `Type` for the built-in queue. Its doc: "System queues are built in. You can't
/// modify or delete system queues."
pub const QUEUE_TYPE_SYSTEM: &str = TYPE_SYSTEM;
/// `Type` for a queue a caller created with `CreateQueue`.
pub const QUEUE_TYPE_CUSTOM: &str = TYPE_CUSTOM;

/// `QueueStatus` values (`smithy_query(query="shape",shape="QueueStatus")`).
pub const QUEUE_STATUS_ACTIVE: &str = "ACTIVE";

/// `PricingPlan` values (`smithy_query(query="shape",shape="PricingPlan")`).
pub const PRICING_PLAN_ON_DEMAND: &str = "ON_DEMAND";
/// See [`PRICING_PLAN_ON_DEMAND`].
pub const PRICING_PLAN_RESERVED: &str = "RESERVED";

/// `JobStatus` values (`smithy_query(query="shape",shape="JobStatus")`).
pub const JOB_STATUS_SUBMITTED: &str = "SUBMITTED";
/// See [`JOB_STATUS_SUBMITTED`].
pub const JOB_STATUS_PROGRESSING: &str = "PROGRESSING";
/// See [`JOB_STATUS_SUBMITTED`].
pub const JOB_STATUS_CANCELED: &str = "CANCELED";
/// See [`JOB_STATUS_SUBMITTED`].
pub const JOB_STATUS_ERROR: &str = "ERROR";

/// `Order` values (`smithy_query(query="shape",shape="Order")`).
pub const ORDER_ASCENDING: &str = "ASCENDING";
/// See [`ORDER_ASCENDING`].
pub const ORDER_DESCENDING: &str = "DESCENDING";

/// `JobTemplateListBy` values (`smithy_query(query="shape",shape="JobTemplateListBy")`).
/// Its doc: "you can choose to list them alphabetically by NAME or chronologically by
/// CREATION_DATE. If you don't specify, the service will list them by name."
pub const LIST_BY_NAME: &str = "NAME";
/// See [`LIST_BY_NAME`].
pub const LIST_BY_CREATION_DATE: &str = "CREATION_DATE";
/// See [`LIST_BY_NAME`]: the third `JobTemplateListBy` value, which selects the built-in
/// ([`TYPE_SYSTEM`]) templates instead of the account's own ([`TYPE_CUSTOM`]) ones.
pub const LIST_BY_SYSTEM: &str = "SYSTEM";

/// `AccelerationMode`/`AccelerationStatus`: "For jobs that have Acceleration
/// (AccelerationMode) set to DISABLED, AccelerationStatus is always NOT_APPLICABLE."
pub const ACCELERATION_MODE_DISABLED: &str = "DISABLED";
/// See [`ACCELERATION_MODE_DISABLED`].
pub const ACCELERATION_STATUS_NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// `ReservationPlanStatus` values (`smithy_query(query="shape",shape="ReservationPlanStatus")`).
pub const RESERVATION_PLAN_STATUS_ACTIVE: &str = "ACTIVE";

/// `DescribeEndpointsMode` values (`smithy_query(query="shape",shape="DescribeEndpointsMode")`).
///
/// Its doc: "Specify DEFAULT for this operation to return your endpoints if any exist,
/// or to create an endpoint for you and return it if one doesn't already exist. Specify
/// GET_ONLY to return your endpoints if any exist, or an empty list if none exist."
pub const DESCRIBE_ENDPOINTS_MODE_DEFAULT: &str = "DEFAULT";
/// See [`DESCRIBE_ENDPOINTS_MODE_DEFAULT`].
pub const DESCRIBE_ENDPOINTS_MODE_GET_ONLY: &str = "GET_ONLY";

/// How long a `ClientRequestToken` keeps naming the job it first created.
///
/// `ClientRequestToken`'s own documentation: "If you reuse a client request token within
/// one minute of a successful request, the API returns the job details for the original
/// request." Past the window the token is free again and names a new job.
pub const IDEMPOTENCY_WINDOW: Duration = Duration::from_secs(60);

/// How many queues this store holds before `CreateQueue` is refused.
///
/// A CHOSEN capacity bound, not a documented AWS quota: it exists so a caller cannot
/// grow the store without limit, and is far above any real workflow. The refusal uses
/// the error the model already declares for it, `ServiceQuotaExceededException` — "You
/// attempted to create more resources than the service allows based on service quotas."
pub const MAX_QUEUES: usize = 1_000;

/// How many jobs this store holds before `CreateJob` is refused. A chosen capacity
/// bound, as [`MAX_QUEUES`] is, and the ceiling on every walk over the job set.
pub const MAX_JOBS: usize = 10_000;

/// How many per-region account endpoints this store mints before `DescribeEndpoints`
/// refuses to mint another.
///
/// The region comes from the request's SigV4 credential scope, so the set of regions an
/// account endpoint can be minted for is caller-driven and needs a ceiling. A CHOSEN
/// capacity bound, not a documented AWS quota, far above the number of AWS regions; past
/// it the refusal uses the error the model already declares for it,
/// `ServiceQuotaExceededException`.
pub const MAX_ENDPOINTS: usize = 100;

/// The documented ceiling on `DescribeEndpoints`' `MaxResults`: "Optional. Max number of
/// endpoints, up to twenty, that will be returned at one time." The member itself is a
/// bare `__integer` in the model, so nothing refuses an out-of-window value before the
/// handler runs.
pub const MAX_ENDPOINT_RESULTS: i32 = 20;

/// The term of a reserved queue's pricing plan. `PricingPlan`'s own documentation:
/// "Reserved pricing requires a 12-month commitment", and `Commitment` models exactly
/// one value, `ONE_YEAR`.
pub const RESERVATION_TERM: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// The `arn:` service/resource namespace for this service's ARNs
/// (`awsService.arnNamespace` is `mediaconvert`).
const ARN_NAMESPACE: &str = "mediaconvert";

/// The region used when a request carries no SigV4 credential scope to read one from
/// (an unsigned request to an emulator with authentication switched off).
const FALLBACK_REGION: &str = "us-east-1";

/// How long after `CreateJob` a job leaves the non-terminal `SUBMITTED` status.
///
/// A chosen span, not a documented one: real MediaConvert takes as long as the
/// transcode takes. It is deliberately non-zero so a caller acting immediately after
/// `CreateJob` still observes `SUBMITTED` and the state errors that go with it.
const JOB_SETTLES_AFTER: Duration = Duration::from_millis(750);

/// Why a job that leaves `SUBMITTED` here ends in `ERROR` rather than `COMPLETE`.
///
/// This emulator has no media transcoding engine, so no transcode ever runs; reporting
/// `COMPLETE` would be a result synthesised for work that never happened. The job
/// therefore ends in the service's own modelled failure status with a reason naming the
/// missing engine.
const NO_TRANSCODER_MESSAGE: &str =
    "This emulator has no media transcoding engine, so the transcode for this job never \
     ran. The job's control-plane lifecycle (queue, priority, metadata, status) is \
     emulated; its output files are not produced.";

/// The region a request was signed for, from its SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| FALLBACK_REGION.to_string())
}

/// `arn:aws:mediaconvert:<region>:<account>:queues/<name>`.
pub fn queue_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("queues/{name}"))
}

/// `arn:aws:mediaconvert:<region>:<account>:jobs/<id>`.
pub fn job_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("jobs/{id}"))
}

/// `arn:aws:mediaconvert:<region>:<account>:jobTemplates/<name>`.
pub fn job_template_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("jobTemplates/{name}"))
}

/// The account's MediaConvert API endpoint URL in `region`.
///
/// MediaConvert gives an account its own subdomain of the regional service host —
/// `https://<account-specific-label>.mediaconvert.<region>.amazonaws.com` — and
/// `DescribeEndpoints` is how a caller reads it. The label here is the first four bytes
/// of SHA-256 over the account id and the region, so it is DERIVED from the account and
/// region and from nothing else: identical on every call in a region, different between
/// regions, and never drawn from a clock or a random source. It is also persisted on
/// first use ([`ComAmazonawsMediaconvertMediaconvertState::ensure_endpoint`]), so the URL
/// a later caller reads is the one an earlier call stored.
pub fn account_endpoint_url(region: &str) -> String {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(
        format!("{}:{region}", ::engine::emulator_account_id()).as_bytes(),
    );
    let label = ::hex::encode(&digest[..4]);
    format!("https://{label}.mediaconvert.{region}.amazonaws.com")
}

/// The queue name a caller named, whether they gave the name or the queue's ARN.
///
/// `CreateJob`'s `Queue` and `ListJobs`' `queue` both accept either form; the resource
/// id in a MediaConvert queue ARN is `queues/<name>`.
pub fn queue_name_from_reference(reference: &str) -> &str {
    match reference.rsplit_once(":queues/") {
        Some((prefix, name)) if prefix.starts_with("arn:") => name,
        _ => reference,
    }
}

/// A timestamp as the model's `__timestampUnix` (`epoch-seconds`) renders it.
///
/// Truncated to whole seconds so the wire value is the integer epoch-seconds field the
/// model declares rather than a fractional one. A pre-epoch instant cannot panic here:
/// `duration_since` is read through `map_or`.
pub fn epoch_seconds(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let secs = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    ::aws_smithy_types::DateTime::from_secs(i64::try_from(secs).unwrap_or(i64::MAX))
}

/// A timestamp as nanoseconds since the epoch — half of the ordering key the paged
/// listings resume on. Reads `duration_since` through `map_or`, so a pre-epoch instant
/// cannot panic.
pub fn epoch_nanos(at: SystemTime) -> u128 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos())
}

/// The page size a listing uses when the caller gives no `maxResults`.
///
/// Twenty, which is what `ListJobs` ("up to twenty of your most recently created jobs"),
/// `ListJobTemplates` ("up to twenty of your job templates") and the `__integerMin1Max20`
/// range on both `MaxResults` members all say.
pub const DEFAULT_PAGE_SIZE: i32 = 20;

/// The opaque `nextToken` of a paged listing: the hex of `<creation instant in epoch
/// nanoseconds>:<id>` — the ORDERING KEY of the last record of the page just returned.
///
/// Encoding the key rather than an index means the next page resumes at the first record
/// strictly past that key in the requested direction, so a record that changed, appeared
/// or went away between two pages cannot duplicate or skip one the caller has already
/// seen. It is hex so it is opaque to a caller: the documented contract is only that the
/// string returned by one request is handed back to the next one.
///
/// Defined ONCE here, so every listing in this service issues and reads the same cursor
/// format.
pub fn encode_cursor(created_at: SystemTime, id: &str) -> String {
    ::hex::encode(format!("{}:{id}", epoch_nanos(created_at)))
}

/// The `(creation instant, id)` ordering key inside a cursor, or `None` when the token is
/// not one [`encode_cursor`] produced — which a handler answers with its modelled
/// `BadRequestException`. Decoded with `ok()?` throughout, so a hostile token cannot
/// panic.
pub fn decode_cursor(token: &str) -> Option<(u128, String)> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (nanos, id) = decoded.split_once(':')?;
    Some((nanos.parse().ok()?, id.to_string()))
}

/// One transcoding queue.
///
/// The ARN is NOT stored: it is derived from the region of the request that reads the
/// queue, so no resource carries the region of whichever caller happened to touch the
/// store first.
#[derive(Debug, Clone)]
pub struct QueueRecord {
    pub name: String,
    pub description: Option<String>,
    pub pricing_plan: String,
    pub status: String,
    pub queue_type: String,
    pub concurrent_jobs: Option<i32>,
    pub maximum_concurrent_feeds: Option<i32>,
    pub reservation_plan: Option<ReservationPlan>,
    pub created_at: SystemTime,
    pub last_updated: SystemTime,
    pub tags: BTreeMap<String, String>,
}

impl QueueRecord {
    /// The modelled `Queue`, with the two job counts computed from `jobs`.
    pub fn view(&self, region: &str, submitted_jobs: i32, progressing_jobs: i32) -> Queue {
        Queue {
            arn: Some(queue_arn(region, &self.name)),
            concurrent_jobs: self.concurrent_jobs,
            created_at: Some(epoch_seconds(self.created_at)),
            description: self.description.clone(),
            last_updated: Some(epoch_seconds(self.last_updated)),
            maximum_concurrent_feeds: self.maximum_concurrent_feeds,
            name: self.name.clone(),
            pricing_plan: Some(self.pricing_plan.clone()),
            progressing_jobs_count: Some(progressing_jobs),
            reservation_plan: self.reservation_plan.clone(),
            // No override is ever applied to a queue here, and the member's doc says a
            // caller seeing one should contact AWS Support — so an empty list, not a
            // fabricated entry.
            service_overrides: Some(Vec::new()),
            status: Some(self.status.clone()),
            submitted_jobs_count: Some(submitted_jobs),
            r#type: Some(self.queue_type.clone()),
        }
    }
}

/// A reserved queue's pricing plan, built from the settings the caller purchased it with.
pub fn reservation_plan(
    settings: &ReservationPlanSettings,
    purchased_at: SystemTime,
) -> ReservationPlan {
    ReservationPlan {
        commitment: Some(settings.commitment.clone()),
        expires_at: Some(epoch_seconds(purchased_at + RESERVATION_TERM)),
        purchased_at: Some(epoch_seconds(purchased_at)),
        renewal_type: Some(settings.renewal_type.clone()),
        reserved_slots: Some(settings.reserved_slots),
        status: Some(RESERVATION_PLAN_STATUS_ACTIVE.to_string()),
    }
}

/// The built-in ([`TYPE_SYSTEM`]) job templates this store is seeded with.
///
/// That an account holds templates it did not create is what `JobTemplate$Type` ("A job
/// template can be of two types: system or custom. System or built-in job templates
/// can't be modified or deleted by the user.") and `JobTemplateListBy`'s `SYSTEM` value
/// say. The model carries no catalogue of them, so this SET is the emulator's own, named
/// after the system job templates AWS Elemental MediaConvert publishes; nothing in the
/// emulator or its tests keys on a particular entry, because a built-in template is
/// recognised by its `Type` and never by its name.
///
/// They carry no encoder settings: this emulator has no transcoding engine, so an
/// output-group graph here would be settings invented for work nothing can run. What the
/// control plane reports about them — name, type, ARN, timestamps — is stored state, read
/// back by every operation that reads a job template.
const SYSTEM_JOB_TEMPLATES: [&str; 4] = [
    "System-Avc_16x9_1080p_29_97fps_8500kbps",
    "System-Avc_16x9_720p_29_97fps_5000kbps",
    "System-Ott_Dash_Mp4_Avc_Aac",
    "System-Ott_Hls_Ts_Avc_Aac",
];

/// One job template: "a pre-made set of encoding instructions that you can use to
/// quickly create a job".
///
/// The ARN is NOT stored, for the reason [`QueueRecord`]'s is not: it is derived from the
/// region of the request that reads the template.
#[derive(Debug, Clone)]
pub struct JobTemplateRecord {
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub queue: Option<String>,
    pub priority: Option<i32>,
    pub settings: JobTemplateSettings,
    pub status_update_interval: Option<String>,
    pub acceleration_settings: Option<AccelerationSettings>,
    pub hop_destinations: Vec<HopDestination>,
    /// [`TYPE_SYSTEM`] for a built-in template, [`TYPE_CUSTOM`] for one a caller created.
    pub template_type: String,
    pub created_at: SystemTime,
    pub last_updated: SystemTime,
    pub tags: BTreeMap<String, String>,
}

impl JobTemplateRecord {
    /// A built-in template, as [`SYSTEM_JOB_TEMPLATES`] ships it.
    fn system(name: &str, at: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            category: None,
            description: None,
            queue: None,
            priority: None,
            settings: JobTemplateSettings::default(),
            status_update_interval: None,
            acceleration_settings: None,
            hop_destinations: Vec::new(),
            template_type: TYPE_SYSTEM.to_string(),
            created_at: at,
            last_updated: at,
            tags: BTreeMap::new(),
        }
    }

    /// The modelled `JobTemplate`.
    pub fn view(&self, region: &str) -> JobTemplate {
        JobTemplate {
            acceleration_settings: self.acceleration_settings.clone(),
            arn: Some(job_template_arn(region, &self.name)),
            category: self.category.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            description: self.description.clone(),
            hop_destinations: Some(self.hop_destinations.clone()),
            last_updated: Some(epoch_seconds(self.last_updated)),
            name: self.name.clone(),
            priority: self.priority,
            queue: self.queue.clone(),
            settings: self.settings.clone(),
            status_update_interval: self.status_update_interval.clone(),
            r#type: Some(self.template_type.clone()),
        }
    }

    /// The pair a paged listing orders this template on: its creation instant and its
    /// name, which is unique within the account, so the pair is a total order.
    pub fn ordering_key(&self) -> (u128, &str) {
        (epoch_nanos(self.created_at), self.name.as_str())
    }

    /// This template's [`encode_cursor`] token.
    pub fn cursor(&self) -> String {
        encode_cursor(self.created_at, &self.name)
    }
}

/// One transcoding job, exactly as `CreateJob` persisted it plus whatever its status
/// has become since. The ARN is derived from the reading request's region, as
/// [`QueueRecord`]'s is.
#[derive(Debug, Clone)]
pub struct JobRecord {
    pub id: String,
    pub role: String,
    pub settings: JobSettings,
    pub queue: String,
    pub job_template: Option<String>,
    pub priority: Option<i32>,
    pub status: String,
    pub created_at: SystemTime,
    pub status_update_interval: Option<String>,
    pub billing_tags_source: Option<String>,
    pub acceleration_settings: Option<AccelerationSettings>,
    pub hop_destinations: Vec<HopDestination>,
    pub job_engine_version_requested: Option<String>,
    pub simulate_reserved_queue: Option<String>,
    pub user_metadata: BTreeMap<String, String>,
    pub client_request_token: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub timing: JobTiming,
    /// When the clock owes this job its exit from `SUBMITTED`; `None` once the job has
    /// reached a status nothing further is due from.
    pub settles_at: Option<SystemTime>,
    pub error_message: Option<String>,
}

/// When a job was submitted, started and finished.
#[derive(Debug, Clone, Default)]
pub struct JobTiming {
    pub submit_time: Option<SystemTime>,
    pub start_time: Option<SystemTime>,
    pub finish_time: Option<SystemTime>,
}

impl JobTiming {
    fn view(&self) -> Timing {
        Timing {
            finish_time: self.finish_time.map(epoch_seconds),
            start_time: self.start_time.map(epoch_seconds),
            submit_time: self.submit_time.map(epoch_seconds),
        }
    }
}

impl JobRecord {
    /// The pair a paged listing orders this job on: its creation instant and its id,
    /// which is unique, so the pair is a total order.
    pub fn ordering_key(&self) -> (u128, &str) {
        (epoch_nanos(self.created_at), self.id.as_str())
    }

    /// This job's [`encode_cursor`] token.
    pub fn cursor(&self) -> String {
        encode_cursor(self.created_at, &self.id)
    }

    /// The modelled `Job`.
    pub fn view(&self, region: &str) -> Job {
        Job {
            acceleration_settings: self.acceleration_settings.clone(),
            acceleration_status: self.acceleration_status(),
            arn: Some(job_arn(region, &self.id)),
            billing_tags_source: self.billing_tags_source.clone(),
            client_request_token: self.client_request_token.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            current_phase: None,
            elemental_inference_configuration: None,
            error_code: None,
            error_message: self.error_message.clone(),
            hop_destinations: Some(self.hop_destinations.clone()),
            id: Some(self.id.clone()),
            job_engine_version_requested: self.job_engine_version_requested.clone(),
            // Nothing transcoded this job, so no engine version was used.
            job_engine_version_used: None,
            job_percent_complete: None,
            job_template: self.job_template.clone(),
            last_share_details: None,
            messages: None,
            output_group_details: None,
            priority: self.priority,
            queue: Some(self.queue.clone()),
            // A job is never hopped between queues here, so its hopping history is empty.
            queue_transitions: Some(Vec::new()),
            retry_count: Some(0),
            role: self.role.clone(),
            settings: self.settings.clone(),
            share_status: None,
            simulate_reserved_queue: self.simulate_reserved_queue.clone(),
            status: Some(self.status.clone()),
            status_update_interval: self.status_update_interval.clone(),
            timing: Some(self.timing.view()),
            user_metadata: Some(self.user_metadata.clone().into_iter().collect()),
            warnings: Some(Vec::new()),
        }
    }

    /// `NOT_APPLICABLE` whenever acceleration is off, per `AccelerationStatus`'s doc.
    /// For a job that asked for acceleration the status is decided by probing the
    /// input, which never happens here, so it stays absent.
    fn acceleration_status(&self) -> Option<String> {
        let mode = self
            .acceleration_settings
            .as_ref()
            .map(|settings| settings.mode.as_str())
            .unwrap_or(ACCELERATION_MODE_DISABLED);
        (mode == ACCELERATION_MODE_DISABLED)
            .then(|| ACCELERATION_STATUS_NOT_APPLICABLE.to_string())
    }
}

/// The account's input policy: the decision the caller stated for each kind of job
/// input, and nothing more.
///
/// Every member of the modelled `Policy` is optional and each targets `InputPolicy`,
/// whose value set is the generated [`crate::ops::types::INPUT_POLICY_VALUES`]
/// (`ALLOWED`/`DISALLOWED`) the boundary validator enforces, so the policy a caller puts
/// can name any subset of the three input kinds. The record keeps each decision exactly
/// as it was stated, including "not stated": an input kind a policy does not restrict
/// needs no stored decision — it is allowed, which is also what an account with no
/// policy at all allows — so no member is materialised on the caller's behalf.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyRecord {
    pub http_inputs: Option<String>,
    pub https_inputs: Option<String>,
    pub s3_inputs: Option<String>,
}

impl PolicyRecord {
    /// The record a `PutPolicy` request asks for. The policy is REPLACED, not merged, so
    /// this is the whole of what that request stated.
    pub fn from_requested(requested: &Policy) -> Self {
        Self {
            http_inputs: requested.http_inputs.clone(),
            https_inputs: requested.https_inputs.clone(),
            s3_inputs: requested.s3_inputs.clone(),
        }
    }

    /// The modelled `Policy`.
    pub fn view(&self) -> Policy {
        Policy {
            http_inputs: self.http_inputs.clone(),
            https_inputs: self.https_inputs.clone(),
            s3_inputs: self.s3_inputs.clone(),
        }
    }
}

/// What the store did when asked to cancel a job.
#[derive(Debug)]
pub enum CancelOutcome {
    /// The job was `SUBMITTED` and is now `CANCELED`.
    Canceled,
    /// No job is stored under that id.
    NotFound,
    /// The job has left `SUBMITTED`, carrying the status it is in now.
    NotCancelable(String),
}

/// What the store did when asked to accept a new job.
#[derive(Debug)]
pub enum AcceptOutcome {
    /// The job is stored under this id.
    Stored(String),
    /// The store is at [`MAX_JOBS`].
    AtCapacity,
}

/// What a `ClientRequestToken` recorded: the job it created and when.
#[derive(Debug, Clone)]
pub struct IdempotencyEntry {
    pub job_id: String,
    pub recorded_at: SystemTime,
}

/// Per-service state for com-amazonaws-mediaconvert-mediaconvert. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsMediaconvertMediaconvertState {
    /// Queues by `Name`; "Each name must be unique within your account". At most
    /// [`MAX_QUEUES`] entries.
    pub queues: BTreeMap<String, QueueRecord>,
    /// Jobs by `Id`. At most [`MAX_JOBS`] entries.
    pub jobs: BTreeMap<String, JobRecord>,
    /// The account's input policy, once `PutPolicy` has stored one. `None` until then
    /// and again after `DeletePolicy`, when the account carries no policy and
    /// `GetPolicy` reports none. A single record, never a growing collection: the
    /// policy API names no resource.
    pub policy: Option<PolicyRecord>,
    /// Job templates by `Name`; "Each name must be unique within your account". Seeded
    /// with the built-in [`SYSTEM_JOB_TEMPLATES`]; a caller's own templates join them
    /// through `CreateJobTemplate`, which is where that growth gets its cap.
    pub job_templates: BTreeMap<String, JobTemplateRecord>,
    /// The account's API endpoint URL per region, keyed by the region of the request
    /// that first asked for it in `DEFAULT` mode. At most [`MAX_ENDPOINTS`] entries.
    /// Only `DescribeEndpoints` in `DEFAULT` mode adds one; nothing ever rewrites one,
    /// which is what makes the URL a caller reads back stable.
    pub endpoints: BTreeMap<String, String>,
    /// `ClientRequestToken` -> the job it created, so `CreateJob` is idempotent for
    /// [`IDEMPOTENCY_WINDOW`]. One entry per stored job at most, so it is bounded by
    /// [`MAX_JOBS`].
    pub idempotency: BTreeMap<String, IdempotencyEntry>,
    /// The jobs still owed their exit from `SUBMITTED`, ordered by the instant it comes
    /// due. [`Self::settle`] drains the already-due prefix, which is why settling never
    /// walks the job set.
    pending: BTreeSet<(SystemTime, String)>,
    /// Monotonic suffix source for a minted job id; never reset, so no id repeats.
    job_sequence: u64,
    /// How long after `CreateJob` a job is due to leave `SUBMITTED`.
    ///
    /// Defaults to [`JOB_SETTLES_AFTER`]. It is a field rather than a constant so a test
    /// can pin the span it needs — a long one to hold a job in `SUBMITTED` for the whole
    /// case, a short one to observe the transition — instead of racing the real clock.
    /// Nothing on a request path writes it.
    pub job_settles_after: Duration,
}

impl Default for ComAmazonawsMediaconvertMediaconvertState {
    /// A store holding the built-in `Default` queue, as every MediaConvert account does
    /// before any call is made, and the built-in system job templates, which an account
    /// likewise has without creating them. Seeded here, at state construction, so no
    /// request ever has to create them and no reporting operation writes to put them
    /// there.
    fn default() -> Self {
        let now = SystemTime::now();
        let mut queues = BTreeMap::new();
        queues.insert(
            DEFAULT_QUEUE_NAME.to_string(),
            QueueRecord {
                name: DEFAULT_QUEUE_NAME.to_string(),
                description: None,
                pricing_plan: PRICING_PLAN_ON_DEMAND.to_string(),
                status: QUEUE_STATUS_ACTIVE.to_string(),
                queue_type: QUEUE_TYPE_SYSTEM.to_string(),
                concurrent_jobs: None,
                maximum_concurrent_feeds: None,
                reservation_plan: None,
                created_at: now,
                last_updated: now,
                tags: BTreeMap::new(),
            },
        );
        Self {
            queues,
            jobs: BTreeMap::new(),
            // No policy until a caller puts one; an account carrying no policy is the
            // state `GetPolicy` reports as an absent `Policy`, so nothing is seeded.
            policy: None,
            job_templates: SYSTEM_JOB_TEMPLATES
                .iter()
                .map(|name| ((*name).to_string(), JobTemplateRecord::system(name, now)))
                .collect(),
            // No account endpoint exists until a `DescribeEndpoints` in `DEFAULT` mode
            // creates one, so `GET_ONLY` on a fresh account answers an empty list.
            endpoints: BTreeMap::new(),
            idempotency: BTreeMap::new(),
            pending: BTreeSet::new(),
            job_sequence: 0,
            job_settles_after: JOB_SETTLES_AFTER,
        }
    }
}

impl ComAmazonawsMediaconvertMediaconvertState {
    /// Materialise every job status change the clock had already made due at `now`.
    ///
    /// Bounded by the number of jobs actually settling, not by the size of the store:
    /// `pending` is ordered by due instant, so this drains its already-due prefix and
    /// stops at the first entry still in the future. It is the only write a reporting
    /// operation makes, and the change it applies was owed at `now` whether or not
    /// anybody called.
    pub fn settle(&mut self, now: SystemTime) {
        while let Some(entry) = self.pending.first().cloned() {
            let (due, id) = entry;
            if due > now {
                break;
            }
            self.pending.remove(&(due, id.clone()));
            if let Some(job) = self.jobs.get_mut(&id) {
                if job.status == JOB_STATUS_SUBMITTED {
                    job.status = JOB_STATUS_ERROR.to_string();
                    job.error_message = Some(NO_TRANSCODER_MESSAGE.to_string());
                    job.timing.start_time = Some(due);
                    job.timing.finish_time = Some(due);
                }
                job.settles_at = None;
            }
        }
    }

    /// Store a newly submitted job, refusing it once the store is at [`MAX_JOBS`].
    ///
    /// Owns the id, the `pending` entry and the idempotency entry together, so the three
    /// cannot be left inconsistent by a caller that forgets one.
    pub fn accept_job(&mut self, mut record: JobRecord, now: SystemTime) -> AcceptOutcome {
        if self.jobs.len() >= MAX_JOBS {
            return AcceptOutcome::AtCapacity;
        }
        let id = self.mint_job_id(now);
        let due = now + self.job_settles_after;
        record.id = id.clone();
        // SUBMITTED is not terminal, so the clock owes this job its exit from it.
        record.settles_at = Some(due);
        self.pending.insert((due, id.clone()));
        if let Some(token) = record.client_request_token.clone() {
            self.idempotency.insert(
                token,
                IdempotencyEntry {
                    job_id: id.clone(),
                    recorded_at: now,
                },
            );
        }
        self.jobs.insert(id.clone(), record);
        AcceptOutcome::Stored(id)
    }

    /// The job a `ClientRequestToken` already created, while the token is still inside
    /// the window its documentation bounds it to ([`IDEMPOTENCY_WINDOW`]). Past the
    /// window the token names nothing and `CreateJob` goes on to create a new job.
    pub fn job_for_token(&self, token: &str, now: SystemTime) -> Option<&JobRecord> {
        let entry = self.idempotency.get(token)?;
        let age = now.duration_since(entry.recorded_at).ok()?;
        if age > IDEMPOTENCY_WINDOW {
            return None;
        }
        self.jobs.get(&entry.job_id)
    }

    /// Cancel a stored job: "Permanently cancel a job. Once you have canceled a job, you
    /// can't start it again." Only a job still waiting in its queue can be cancelled.
    pub fn cancel_job(&mut self, id: &str, now: SystemTime) -> CancelOutcome {
        let Some(job) = self.jobs.get_mut(id) else {
            return CancelOutcome::NotFound;
        };
        if job.status != JOB_STATUS_SUBMITTED {
            return CancelOutcome::NotCancelable(job.status.clone());
        }
        job.status = JOB_STATUS_CANCELED.to_string();
        job.timing.finish_time = Some(now);
        // A cancelled job is terminal, so the clock owes it nothing further.
        let due = job.settles_at.take();
        if let Some(due) = due {
            self.pending.remove(&(due, id.to_string()));
        }
        CancelOutcome::Canceled
    }

    /// A fresh, unused job id.
    ///
    /// Shaped like a MediaConvert job id — the submit instant in epoch milliseconds, a
    /// hyphen, then a short suffix — and drawn from a stored sequence rather than a
    /// random source, so the id a caller is handed is the id that was persisted. The
    /// loop terminates: the sequence strictly increases and the store holds fewer than
    /// [`MAX_JOBS`] ids.
    fn mint_job_id(&mut self, now: SystemTime) -> String {
        let millis = now
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());
        loop {
            self.job_sequence += 1;
            let candidate = format!("{millis}-{}", base36(self.job_sequence));
            if !self.jobs.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The account endpoint already stored for `region`, or `None` when none has been
    /// created there yet. A pure read: `GET_ONLY` mode goes through this and writes
    /// nothing.
    pub fn endpoint(&self, region: &str) -> Option<&str> {
        self.endpoints.get(region).map(String::as_str)
    }

    /// The account endpoint for `region`, creating and persisting it on the first ask.
    ///
    /// This is the write `DescribeEndpoints`' `DEFAULT` mode documents: "return your
    /// endpoints if any exist, or to create an endpoint for you and return it if one
    /// doesn't already exist." An endpoint already stored is returned UNCHANGED, so the
    /// URL is stable for the life of the account. `None` once the store holds
    /// [`MAX_ENDPOINTS`] regions and this one is not among them.
    pub fn ensure_endpoint(&mut self, region: &str) -> Option<&str> {
        if !self.endpoints.contains_key(region) && self.endpoints.len() >= MAX_ENDPOINTS {
            return None;
        }
        Some(
            self.endpoints
                .entry(region.to_string())
                .or_insert_with(|| account_endpoint_url(region))
                .as_str(),
        )
    }

    /// Whether the store is at [`MAX_QUEUES`] and can hold no further queue.
    pub fn queues_at_capacity(&self) -> bool {
        self.queues.len() >= MAX_QUEUES
    }

    /// How many jobs in `queue_name` are `SUBMITTED` and how many are `PROGRESSING`.
    ///
    /// Derived on every read rather than stored, so the counts cannot drift from `jobs`;
    /// the walk is over at most [`MAX_JOBS`] records.
    pub fn job_counts(&self, queue_name: &str) -> (i32, i32) {
        let mut submitted = 0;
        let mut progressing = 0;
        for job in self.jobs.values().filter(|job| job.queue == queue_name) {
            match job.status.as_str() {
                JOB_STATUS_SUBMITTED => submitted += 1,
                JOB_STATUS_PROGRESSING => progressing += 1,
                _ => {}
            }
        }
        (submitted, progressing)
    }
}

/// A `u64` in lowercase base 36, left-padded to six digits — the alphabet and width of
/// the suffix in a MediaConvert job id. Total: every remainder indexes a fixed 36-byte
/// table, so there is nothing to unwrap.
fn base36(mut value: u64) -> String {
    const DIGITS: [char; 36] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f', 'g',
        'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x',
        'y', 'z',
    ];
    let mut out = ['0'; 6];
    for slot in out.iter_mut().rev() {
        *slot = DIGITS.get((value % 36) as usize).copied().unwrap_or('0');
        value /= 36;
    }
    out.iter().collect()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMediaconvertMediaconvertState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMediaconvertMediaconvertState>)
}
