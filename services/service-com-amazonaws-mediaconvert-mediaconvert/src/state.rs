// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediaconvert-mediaconvert service.
//!
//! The transcoding-job control plane: the `queues` a job can be sent to, the
//! `job_templates` a job can be created from, the `jobs` themselves, the output `presets`
//! a job's outputs can name and the account's input `policy`. Every response the handlers
//! in `src/ops/` build is projected from a record stored here, so a value a caller reads
//! back is the value some earlier call persisted.
//!
//! The account policy is a SINGLETON — the API has no policy name or id, so `PutPolicy`
//! replaces one record and `DeletePolicy` drops it; it is not caller-driven growth.
//!
//! # Bounds on caller-driven growth and per-request work
//!
//! Nothing here grows without a ceiling and nothing here walks an unbounded set:
//!
//! * `queues` is capped at [`MAX_QUEUES`], `jobs` at [`MAX_JOBS`], `job_templates` at
//!   [`MAX_JOB_TEMPLATES`], `presets` at [`MAX_PRESETS`] and `jobs_queries` at
//!   [`MAX_JOBS_QUERIES`]; a create past the cap is
//!   refused with the modelled `ServiceQuotaExceededException` (HTTP 402) instead of
//!   growing the store. The built-in system templates `job_templates` is seeded with are
//!   inside that cap, and no request path removes or re-seeds them: `UpdateJobTemplate`
//!   and `DeleteJobTemplate` refuse a `SYSTEM` template with the modelled
//!   `ForbiddenException`. `idempotency` holds at most one entry per stored job, so it is
//!   bounded by `MAX_JOBS` too, and `endpoints` (one account endpoint per region a request
//!   was signed for) is capped at [`MAX_ENDPOINTS`] the same way.
//! * A record's `tags` are capped at [`MAX_TAGS_PER_RESOURCE`], so `TagResource` cannot
//!   grow one record without limit either; past the cap it answers the same quota error.
//! * Every lookup is an O(log n) `BTreeMap` probe; no handler clones the whole store.
//! * [`ComAmazonawsMediaconvertMediaconvertState::settle`] — the only work a reporting
//!   operation does — touches ONLY the records whose due instant has already passed, by
//!   draining the `pending` and `pending_queries` indexes that are ordered by that
//!   instant, and does a CONSTANT amount of work per record it drains. It costs
//!   O(k log n) in the number k of records actually due, which is zero on the
//!   overwhelming majority of calls; it is never a sweep of the job set.
//! * `ListJobs` orders the jobs, over at most `MAX_JOBS` records, and returns at most
//!   one page of twenty, through [`page_of_jobs`]. `StartJobsQuery` walks the same record
//!   set ONCE, when the query is started, evaluating at most [`MAX_JOBS_QUERY_TERMS`]
//!   filter terms per record, and stores only the one page of ids it selected; no later
//!   call re-runs that match, so no request's cost grows with the number of stored
//!   queries. `ListJobTemplates` orders the job templates the same way and pages them the
//!   same way.
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
    AccelerationSettings, HopDestination, Job, JobMessages, JobSettings, JobTemplate,
    JobTemplateSettings, Output, Policy, Preset, PresetSettings, Queue, ReservationPlan,
    ReservationPlanSettings, Timing,
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

/// `JobsQueryStatus` values (`smithy_query(query="shape",shape="JobsQueryStatus")`): the
/// status `GetJobsQueryResults` reports for a query `StartJobsQuery` started. Its doc:
/// "A job query's status can be SUBMITTED, PROGRESSING, COMPLETE, or ERROR."
pub const JOBS_QUERY_STATUS_SUBMITTED: &str = "SUBMITTED";
/// See [`JOBS_QUERY_STATUS_SUBMITTED`].
pub const JOBS_QUERY_STATUS_COMPLETE: &str = "COMPLETE";

/// `JobsQueryFilterKey` values
/// (`smithy_query(query="shape",shape="JobsQueryFilterKey")`): the job detail one filter
/// of a jobs query matches on. `JobsQueryFilter$Key`'s own documentation says what each
/// one holds — "queue - Your Queue's name or ARN", "status - Your job's status",
/// "fileInput - Your input file URL, or partial input file name",
/// "jobEngineVersionRequested - The Job engine version that you requested for your job",
/// "jobEngineVersionUsed - The Job engine version that your job used", "audioCodec - Your
/// output's audio codec", "videoCodec - Your output's video codec" and "errorCode - The
/// error code that your job failed with" — and [`JobRecord::matches_filter`] reads each
/// one off the stored job.
pub const JOBS_QUERY_FILTER_KEY_QUEUE: &str = "queue";
/// See [`JOBS_QUERY_FILTER_KEY_QUEUE`].
pub const JOBS_QUERY_FILTER_KEY_STATUS: &str = "status";
/// See [`JOBS_QUERY_FILTER_KEY_QUEUE`].
pub const JOBS_QUERY_FILTER_KEY_FILE_INPUT: &str = "fileInput";
/// See [`JOBS_QUERY_FILTER_KEY_QUEUE`].
pub const JOBS_QUERY_FILTER_KEY_JOB_ENGINE_VERSION_REQUESTED: &str = "jobEngineVersionRequested";
/// See [`JOBS_QUERY_FILTER_KEY_QUEUE`].
pub const JOBS_QUERY_FILTER_KEY_AUDIO_CODEC: &str = "audioCodec";
/// See [`JOBS_QUERY_FILTER_KEY_QUEUE`].
pub const JOBS_QUERY_FILTER_KEY_VIDEO_CODEC: &str = "videoCodec";

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

/// `BillingTagsSource` values (`smithy_query(query="shape",shape="BillingTagsSource")`):
/// "The tag type that AWS Billing and Cost Management will use to sort your AWS Elemental
/// MediaConvert costs". `JOB` is the SERVICE DEFAULT — `CreateJob$BillingTagsSource`'s own
/// doc: "Leave blank to use the default value, Job."
pub const BILLING_TAGS_SOURCE_JOB: &str = "JOB";

/// `ShareStatus` values (`smithy_query(query="shape",shape="ShareStatus")`): "A job's share
/// status can be NOT_SHARED, INITIATED, or SHARED". A job no resource share has been
/// created for is `NOT_SHARED`.
pub const SHARE_STATUS_NOT_SHARED: &str = "NOT_SHARED";

/// The `Priority` a job or job template created without one carries.
///
/// `CreateJob$Priority`'s own documentation: "If you don't specify a priority, the service
/// uses the default value 0" — and `CreateJobTemplate$Priority` says the same. The member
/// is service-applied, never absent, so it is stored rather than left empty.
pub const DEFAULT_PRIORITY: i32 = 0;

/// The `StatusUpdateInterval` a job or job template created without one carries.
///
/// The member's doc gives the cadence in seconds ("Set the interval, in seconds, between
/// status updates") but states no default; MediaConvert's own default status-update cadence
/// is 60 seconds, which is the value the recorded wire corpus shows on every job and job
/// template created without the member. `StatusUpdateInterval` declares `SECONDS_60`.
pub const DEFAULT_STATUS_UPDATE_INTERVAL: &str = "SECONDS_60";

/// The number of jobs a queue created without `ConcurrentJobs` can process concurrently.
///
/// `CreateQueue$ConcurrentJobs` is optional ("For on-demand queues, the value you enter is
/// constrained by your service quotas for Maximum concurrent jobs, per on-demand queue"),
/// and the service fills it in from the account's quota when the request omits it: the
/// member is never absent from a `Queue`. The model declares no default, so this is the
/// account-default concurrency the recorded wire corpus shows for a queue created without
/// the member.
pub const DEFAULT_CONCURRENT_JOBS: i32 = 200;

/// The number of Elemental Inference feeds a queue created without
/// `MaximumConcurrentFeeds` can process concurrently: none. `__integerMin0` makes zero the
/// bottom of the member's range, and zero is what the recorded corpus shows for a queue
/// whose request omitted it.
pub const DEFAULT_MAXIMUM_CONCURRENT_FEEDS: i32 = 0;

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

/// How many jobs queries this store holds before `StartJobsQuery` is refused. A chosen
/// capacity bound, as [`MAX_QUEUES`] is, refused with the same modelled
/// `ServiceQuotaExceededException` the operation declares.
pub const MAX_JOBS_QUERIES: usize = 1_000;

/// How many filter terms — one per value of one `JobsQueryFilterKey` — one jobs query
/// matches on before `StartJobsQuery` refuses it.
///
/// A CHOSEN bound, not a documented AWS quota: `__listOfJobsQueryFilter` carries no
/// length constraint, so without it a single request could ask for an unbounded amount
/// of matching work. Forty is five values for each of the eight keys
/// `JobsQueryFilterKey` declares, well above any real query. Past it the refusal is the
/// modelled `BadRequestException` — "a problem in the request", which a caller fixes by
/// sending fewer filter values.
///
/// With it, the WHOLE cost of running a query is bounded: `StartJobsQuery` matches the
/// job set once, walking at most [`MAX_JOBS`] records and evaluating at most
/// `MAX_JOBS_QUERY_TERMS` terms per record, each against a filter value of at most 100
/// bytes (every value targets `__stringMax100`, whose length constraint the boundary
/// validator enforces before a handler runs). Nothing multiplies that by the number of
/// queries in the store: a query is matched by the one request that started it, never
/// again.
pub const MAX_JOBS_QUERY_TERMS: usize = 40;

/// How many job templates this store holds before `CreateJobTemplate` is refused. A
/// chosen capacity bound, as [`MAX_QUEUES`] is, refused with the same modelled
/// `ServiceQuotaExceededException` the operation declares.
pub const MAX_JOB_TEMPLATES: usize = 1_000;

/// How many presets this store holds before `CreatePreset` is refused. A chosen
/// capacity bound, as [`MAX_QUEUES`] is, refused with the same modelled
/// `ServiceQuotaExceededException`.
pub const MAX_PRESETS: usize = 1_000;

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

/// How long after `StartJobsQuery` a jobs query leaves the non-terminal `SUBMITTED`
/// status for `COMPLETE`.
///
/// A chosen span, not a documented one: the operation is only documented as
/// asynchronous ("Start an asynchronous jobs query ... To receive the list of jobs that
/// match your query, call the GetJobsQueryResults API"). It is deliberately non-zero so
/// a caller reading the results immediately after starting the query still observes the
/// query in `SUBMITTED` with no results yet.
const JOBS_QUERY_COMPLETES_AFTER: Duration = Duration::from_millis(500);

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

/// How this service words a `NotFoundException`: it names the REQUEST MEMBER that
/// pointed at the resource, not the value the caller supplied.
///
/// MediaConvert answers a lookup that resolves nothing with "The resource you specified
/// in `<member>` was not found.", where `<member>` is the input member the handler read
/// the reference from (`name` for a queue, `id` for a job, `queue` for a job's queue) —
/// so the wording is built from the request, exactly as the other member-naming refusals
/// in this crate are, and never from the missing resource's own name.
pub fn not_found_message(member: &str) -> String {
    format!("The resource you specified in {member} was not found.")
}

/// How this service refuses the deletion of a BUILT-IN resource.
///
/// Recorded AWS behaviour for `DeleteQueue` on the account's built-in queue — see
/// `issues/.../mediaconvert-queue-job-control-plane-4-fidelity-divergences.md`, cause 3 —
/// is `ForbiddenException` (403) worded "You specified an invalid value for `<member>`.
/// You are not allowed to delete system resources.", where `<member>` is the request
/// member that named the resource. As with [`not_found_message`], the wording is built
/// from the request, never from the resource's own name.
pub fn system_resource_delete_message(member: &str) -> String {
    system_resource_message(member, "delete")
}

/// As [`system_resource_delete_message`], for a request that would MODIFY a built-in
/// resource rather than delete it — `UpdateJobTemplate` on a `SYSTEM` template, which
/// `JobTemplate$Type` says a user "can't modify or delete". Same recorded shape, naming
/// the request member and the action the request asked for.
pub fn system_resource_modify_message(member: &str) -> String {
    system_resource_message(member, "modify")
}

/// The one wording both built-in-resource refusals share, so the recorded sentence lives
/// in a single format string and only the action the request asked for differs.
fn system_resource_message(member: &str, action: &str) -> String {
    format!(
        "You specified an invalid value for {member}. You are not allowed to {action} \
         system resources."
    )
}

/// `arn:aws:mediaconvert:<region>:<account>:queues/<name>`.
///
/// The resource-id format itself lives in exactly one place,
/// [`TaggedResource::resource_id`], so the ARN a reader reports and the ARN the tagging
/// operations resolve cannot drift apart.
pub fn queue_arn(region: &str, name: &str) -> String {
    TaggedResource::Queue(name.to_string()).arn(region)
}

/// `arn:aws:mediaconvert:<region>:<account>:jobs/<id>`. See [`queue_arn`].
pub fn job_arn(region: &str, id: &str) -> String {
    TaggedResource::Job(id.to_string()).arn(region)
}

/// `arn:aws:mediaconvert:<region>:<account>:presets/<name>`. See [`queue_arn`].
pub fn preset_arn(region: &str, name: &str) -> String {
    TaggedResource::Preset(name.to_string()).arn(region)
}

/// `arn:aws:mediaconvert:<region>:<account>:jobTemplates/<name>`. See [`queue_arn`].
pub fn job_template_arn(region: &str, name: &str) -> String {
    TaggedResource::JobTemplate(name.to_string()).arn(region)
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

/// The resource-type prefixes a MediaConvert resource ARN can carry.
///
/// `TagResource`'s own documentation: "Add tags to a MediaConvert queue, preset, job, or
/// job template." The resource id of each is `<plural type>/<name>`, as
/// [`queue_arn`] and [`job_arn`] already build for the two kinds this store holds.
pub const RESOURCE_TYPE_QUEUES: &str = "queues";
/// See [`RESOURCE_TYPE_QUEUES`].
pub const RESOURCE_TYPE_JOBS: &str = "jobs";
/// See [`RESOURCE_TYPE_QUEUES`].
pub const RESOURCE_TYPE_PRESETS: &str = "presets";
/// See [`RESOURCE_TYPE_QUEUES`].
pub const RESOURCE_TYPE_JOB_TEMPLATES: &str = "jobTemplates";

/// How many tags one resource carries before a tagging call is refused.
///
/// A CHOSEN capacity bound, as [`MAX_QUEUES`] is, set at the tag count AWS's tagging
/// restrictions publish for a resource: it exists so `TagResource` cannot grow a record
/// without limit. Past it the call answers the error the model already declares for
/// growing past a quota, `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// A resource a MediaConvert ARN names, as the tagging operations resolve it.
///
/// Identity here is the resource KIND plus its name, which is exactly what the resource
/// id of the ARN carries: the partition, region and account of an ARN are not part of a
/// record's identity in this store, which serves one account and derives every ARN it
/// reports from the region of the request that reads it (see [`QueueRecord`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// `queues/<name>`.
    Queue(String),
    /// `jobs/<id>`.
    Job(String),
    /// `presets/<name>`.
    Preset(String),
    /// `jobTemplates/<name>`.
    JobTemplate(String),
}

impl TaggedResource {
    /// The resource id this resource's ARN carries, `<type>/<name>`.
    pub fn resource_id(&self) -> String {
        match self {
            Self::Queue(name) => format!("{RESOURCE_TYPE_QUEUES}/{name}"),
            Self::Job(id) => format!("{RESOURCE_TYPE_JOBS}/{id}"),
            Self::Preset(name) => format!("{RESOURCE_TYPE_PRESETS}/{name}"),
            Self::JobTemplate(name) => format!("{RESOURCE_TYPE_JOB_TEMPLATES}/{name}"),
        }
    }

    /// This resource's ARN in `region`, built by the same helper every other ARN this
    /// service reports goes through.
    pub fn arn(&self, region: &str) -> String {
        ::engine::emulator_arn(ARN_NAMESPACE, region, &self.resource_id())
    }
}

/// The resource an ARN names, or `None` when the value is not a MediaConvert resource
/// ARN at all.
///
/// Accepts `arn:<partition>:mediaconvert:<region>:<account>:<type>/<name>` for the four
/// resource kinds MediaConvert documents as taggable, and nothing else: a value that is
/// not an ARN, an ARN of another service, or one whose resource id names no MediaConvert
/// resource kind is a problem in the REQUEST, which a caller fixes by sending a different
/// value — the handlers answer it with the modelled `BadRequestException`. Whether the
/// named resource EXISTS is a separate question, answered by [`ComAmazonawsMediaconvertMediaconvertState::tags`].
pub fn parse_resource_arn(arn: &str) -> Option<TaggedResource> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    // The partition and the ARN's own region/account are not part of a record's
    // identity here, but they must be present for the value to be an ARN at all.
    let _partition = fields.next()?;
    if fields.next()? != ARN_NAMESPACE {
        return None;
    }
    let _region = fields.next()?;
    let _account = fields.next()?;
    let (kind, name) = fields.next()?.split_once('/')?;
    if name.is_empty() {
        return None;
    }
    match kind {
        RESOURCE_TYPE_QUEUES => Some(TaggedResource::Queue(name.to_string())),
        RESOURCE_TYPE_JOBS => Some(TaggedResource::Job(name.to_string())),
        RESOURCE_TYPE_PRESETS => Some(TaggedResource::Preset(name.to_string())),
        RESOURCE_TYPE_JOB_TEMPLATES => Some(TaggedResource::JobTemplate(name.to_string())),
        _ => None,
    }
}

/// The `AccelerationSettings` a job or job template created without the member carries.
///
/// Accelerated transcoding is opt-in, so a resource whose request said nothing about it is
/// not accelerated — `AccelerationStatus`'s doc names that state: "For jobs that have
/// Acceleration (AccelerationMode) set to DISABLED, AccelerationStatus is always
/// NOT_APPLICABLE." The structure's one member is required, so the service-applied value
/// is the whole structure with `Mode` [`ACCELERATION_MODE_DISABLED`], which is what the
/// recorded corpus shows.
pub fn default_acceleration_settings() -> AccelerationSettings {
    AccelerationSettings {
        mode: ACCELERATION_MODE_DISABLED.to_string(),
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
    /// Always a value: the service fills it from the account's quota when the request
    /// omits it ([`DEFAULT_CONCURRENT_JOBS`]), so `Queue$ConcurrentJobs` is never absent.
    pub concurrent_jobs: i32,
    /// Always a value, as `concurrent_jobs` is
    /// ([`DEFAULT_MAXIMUM_CONCURRENT_FEEDS`] when the request omits it).
    pub maximum_concurrent_feeds: i32,
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
            concurrent_jobs: Some(self.concurrent_jobs),
            created_at: Some(epoch_seconds(self.created_at)),
            description: self.description.clone(),
            last_updated: Some(epoch_seconds(self.last_updated)),
            maximum_concurrent_feeds: Some(self.maximum_concurrent_feeds),
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

/// One output preset, exactly as `CreatePreset` persisted it.
///
/// The ARN is NOT stored, for the reason [`QueueRecord`]'s is not: it is derived from
/// the region of the request that reads the preset.
#[derive(Debug, Clone)]
pub struct PresetRecord {
    /// The `Name` exactly as the caller sent it: it is this resource's identity, and
    /// `GetPreset` looks it up verbatim, so nothing here normalises it.
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub settings: PresetSettings,
    /// `SYSTEM` for a built-in preset, `CUSTOM` for one a caller created.
    pub preset_type: String,
    pub created_at: SystemTime,
    pub last_updated: SystemTime,
    /// The `Tags` a create carried. `Preset` models no tag member, so they are not in
    /// the view; they are what `ListTagsForResource` reads once it is implemented.
    pub tags: BTreeMap<String, String>,
}

impl PresetRecord {
    /// The modelled `Preset`.
    pub fn view(&self, region: &str) -> Preset {
        Preset {
            arn: Some(preset_arn(region, &self.name)),
            category: self.category.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            description: self.description.clone(),
            last_updated: Some(epoch_seconds(self.last_updated)),
            name: self.name.clone(),
            settings: self.settings.clone(),
            r#type: Some(self.preset_type.clone()),
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
    /// Always a value: service-applied ([`DEFAULT_PRIORITY`]) when the request omits it.
    pub priority: i32,
    pub settings: JobTemplateSettings,
    /// Always a value: service-applied ([`DEFAULT_STATUS_UPDATE_INTERVAL`]) when the
    /// request omits it.
    pub status_update_interval: String,
    /// Always a value: service-applied ([`default_acceleration_settings`]) when the
    /// request omits it.
    pub acceleration_settings: AccelerationSettings,
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
            priority: DEFAULT_PRIORITY,
            settings: JobTemplateSettings::default(),
            status_update_interval: DEFAULT_STATUS_UPDATE_INTERVAL.to_string(),
            acceleration_settings: default_acceleration_settings(),
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
            acceleration_settings: Some(self.acceleration_settings.clone()),
            arn: Some(job_template_arn(region, &self.name)),
            category: self.category.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            description: self.description.clone(),
            hop_destinations: Some(self.hop_destinations.clone()),
            last_updated: Some(epoch_seconds(self.last_updated)),
            name: self.name.clone(),
            priority: Some(self.priority),
            queue: self.queue.clone(),
            settings: self.settings.clone(),
            status_update_interval: Some(self.status_update_interval.clone()),
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
    /// The NAME of the queue this job was submitted to — this store's key for it, which
    /// is why a job outlives the queue's deletion. `Job$Queue` is projected as that
    /// queue's ARN; see [`Self::view`].
    pub queue: String,
    pub job_template: Option<String>,
    /// Always a value: service-applied ([`DEFAULT_PRIORITY`]) when the request omits it.
    pub priority: i32,
    pub status: String,
    pub created_at: SystemTime,
    /// Always a value: service-applied ([`DEFAULT_STATUS_UPDATE_INTERVAL`]) when the
    /// request omits it.
    pub status_update_interval: String,
    /// Always a value: service-applied ([`BILLING_TAGS_SOURCE_JOB`]) when the request
    /// omits it.
    pub billing_tags_source: String,
    /// Always a value: service-applied ([`default_acceleration_settings`]) when the
    /// request omits it.
    pub acceleration_settings: AccelerationSettings,
    /// Whether this job has been shared, which `CreateResourceShare` is what moves — so
    /// a job nothing has shared is [`SHARE_STATUS_NOT_SHARED`].
    pub share_status: String,
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

    /// Whether this job matches ONE `(key, value)` pair of a jobs query's filter list.
    ///
    /// Each `JobsQueryFilterKey` is read off THIS record — the same state [`Self::view`]
    /// reports — so a query matches on what a reader of the job would see, never on the
    /// text of the request that created it.
    pub fn matches_filter(&self, key: &str, value: &str) -> bool {
        match key {
            // "Your Queue's name or ARN", so either form resolves to the name of the
            // queue this job was sent to.
            JOBS_QUERY_FILTER_KEY_QUEUE => self.queue == queue_name_from_reference(value),
            JOBS_QUERY_FILTER_KEY_STATUS => self.status == value,
            // "Your input file URL, or PARTIAL input file name" — so a containment test
            // over the inputs' `FileInput`, not an equality one.
            JOBS_QUERY_FILTER_KEY_FILE_INPUT => self.input_files().any(|file| file.contains(value)),
            JOBS_QUERY_FILTER_KEY_JOB_ENGINE_VERSION_REQUESTED => {
                self.job_engine_version_requested.as_deref() == Some(value)
            }
            JOBS_QUERY_FILTER_KEY_AUDIO_CODEC => self.audio_codecs().any(|codec| codec == value),
            JOBS_QUERY_FILTER_KEY_VIDEO_CODEC => self.video_codecs().any(|codec| codec == value),
            // The two remaining keys, `jobEngineVersionUsed` and `errorCode`, match on
            // the `Job` members no job in this store carries: nothing transcoded a job
            // here, so no engine version was USED and no transcoder error code was
            // produced — exactly the absent members [`Self::view`] reports. No stored job
            // matches either, and a key the model does not declare never reaches here at
            // all: the member targets the `JobsQueryFilterKey` enum, so the boundary
            // validator has already refused it.
            _ => false,
        }
    }

    /// The `FileInput` of every input of this job's settings.
    fn input_files(&self) -> impl Iterator<Item = &str> + '_ {
        self.settings
            .inputs
            .iter()
            .flatten()
            .filter_map(|input| input.file_input.as_deref())
    }

    /// Every output of every output group of this job's settings.
    fn outputs(&self) -> impl Iterator<Item = &Output> + '_ {
        self.settings
            .output_groups
            .iter()
            .flatten()
            .flat_map(|group| group.outputs.iter().flatten())
    }

    /// The `Codec` of every audio description of every output of this job.
    fn audio_codecs(&self) -> impl Iterator<Item = &str> + '_ {
        self.outputs()
            .flat_map(|output| output.audio_descriptions.iter().flatten())
            .filter_map(|description| description.codec_settings.as_ref())
            .filter_map(|settings| settings.codec.as_deref())
    }

    /// The `Codec` of every output's video description.
    fn video_codecs(&self) -> impl Iterator<Item = &str> + '_ {
        self.outputs()
            .filter_map(|output| output.video_description.as_ref())
            .filter_map(|description| description.codec_settings.as_ref())
            .filter_map(|settings| settings.codec.as_deref())
    }

    /// The modelled `Job`.
    pub fn view(&self, region: &str) -> Job {
        Job {
            acceleration_settings: Some(self.acceleration_settings.clone()),
            acceleration_status: self.acceleration_status(),
            arn: Some(job_arn(region, &self.id)),
            billing_tags_source: Some(self.billing_tags_source.clone()),
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
            // "Provides messages from the service about jobs that you have already
            // successfully submitted": the container is always reported, and this service
            // has produced no informational or warning message about the job, so both
            // lists are empty rather than absent.
            messages: Some(JobMessages {
                info: Some(Vec::new()),
                warning: Some(Vec::new()),
            }),
            output_group_details: None,
            priority: Some(self.priority),
            // A job's queue member is the ARN of the queue it was submitted to, built from
            // the stored queue name and the reading request's region — as every other ARN
            // this service reports is.
            queue: Some(queue_arn(region, &self.queue)),
            // A job is never hopped between queues here, so its hopping history is empty.
            queue_transitions: Some(Vec::new()),
            retry_count: Some(0),
            role: self.role.clone(),
            settings: self.settings.clone(),
            share_status: Some(self.share_status.clone()),
            simulate_reserved_queue: self.simulate_reserved_queue.clone(),
            status: Some(self.status.clone()),
            status_update_interval: Some(self.status_update_interval.clone()),
            timing: Some(self.timing.view()),
            user_metadata: Some(self.user_metadata.clone().into_iter().collect()),
            warnings: Some(Vec::new()),
        }
    }

    /// `NOT_APPLICABLE` whenever acceleration is off, per `AccelerationStatus`'s doc.
    /// For a job that asked for acceleration the status is decided by probing the
    /// input, which never happens here, so it stays absent.
    fn acceleration_status(&self) -> Option<String> {
        (self.acceleration_settings.mode == ACCELERATION_MODE_DISABLED)
            .then(|| ACCELERATION_STATUS_NOT_APPLICABLE.to_string())
    }
}

/// One `(key, value)` pair of a jobs query's filter list: one `JobsQueryFilterKey` and
/// one of the `Values` the caller paired with it.
///
/// A filter naming several values becomes one term per value, because the match over
/// them is an OR — see [`JobsQueryRecord::matches`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobsQueryTerm {
    pub key: String,
    pub value: String,
}

/// One asynchronous jobs query: the query `StartJobsQuery` persisted, and the page of
/// jobs it matched.
///
/// `StartJobsQuery` is asynchronous — "Start an asynchronous jobs query using the
/// provided filters. To receive the list of jobs that match your query, call the
/// GetJobsQueryResults API using the query ID returned by this API" — so a query starts
/// in the non-terminal `SUBMITTED` status and reports no results until the clock makes
/// its completion due, at which point
/// [`ComAmazonawsMediaconvertMediaconvertState::settle`] moves it to `COMPLETE`.
///
/// The MATCH itself runs once, when the query is STARTED
/// ([`ComAmazonawsMediaconvertMediaconvertState::accept_jobs_query`]): the page is a
/// snapshot of the job set at that instant, stored on the record, so every read of the
/// query reports the same page — polling the results does not change them — and the
/// matching work is bounded and paid for by the one request that asked for it, never
/// multiplied by however many queries happen to come due together. Nothing a caller can
/// observe distinguishes that from matching at the completion instant, except a job
/// created inside the completion window, which the API documents nothing about either
/// way.
#[derive(Debug, Clone)]
pub struct JobsQueryRecord {
    /// The query's id, which `GetJobsQueryResults` looks it up by; assigned by
    /// [`ComAmazonawsMediaconvertMediaconvertState::accept_jobs_query`].
    pub id: String,
    /// The filter list, one term per `(Key, value)` pair. At most
    /// [`MAX_JOBS_QUERY_TERMS`] of them.
    pub terms: Vec<JobsQueryTerm>,
    /// The `Order` the caller asked the matches to be sorted in, if any.
    pub order: Option<String>,
    /// "Optional. Number of jobs, up to twenty, that will be included in the jobs
    /// query."
    pub max_results: Option<i32>,
    /// One of the [`JOBS_QUERY_STATUS_SUBMITTED`] values.
    pub status: String,
    /// When the clock owes this query its exit from `SUBMITTED`; `None` once the query
    /// has reached a status nothing further is due from.
    pub completes_at: Option<SystemTime>,
    /// The ids of the jobs on the page this query matched, in the order it matched them.
    /// Reported once the query is [`JOBS_QUERY_STATUS_COMPLETE`].
    pub matched: Vec<String>,
    /// The cursor for the batch after this page, or `None` when this page is the last.
    pub result_next_token: Option<String>,
}

impl JobsQueryRecord {
    /// Whether `job` is one this query matches.
    ///
    /// `JobsQueryFilter`'s own documentation: "Provide one or more JobsQueryFilter
    /// objects, each containing a Key with an associated Values array. Note that
    /// MediaConvert queries jobs using OR logic." So a job matches when it matches ANY
    /// term — any value of any filter — and a query carrying no term at all narrows
    /// nothing and matches every job, as a listing with no filter does.
    pub fn matches(&self, job: &JobRecord) -> bool {
        self.terms.is_empty()
            || self
                .terms
                .iter()
                .any(|term| job.matches_filter(&term.key, &term.value))
    }
}

/// One page of jobs and the cursor that resumes after it.
#[derive(Debug)]
pub struct JobPage<'a> {
    pub jobs: Vec<&'a JobRecord>,
    /// The [`encode_cursor`] token of the last job of the page, and only when a further
    /// page exists.
    pub next_token: Option<String>,
}

/// The page of `jobs` a paged jobs reader reports.
///
/// Every paged reader of the job set goes through this one function — `ListJobs` and the
/// page a jobs query matches — so the ordering, the cursor and the page size cannot drift
/// apart between them. "Your most recently created jobs": ordered by creation time,
/// newest first unless the caller asked for [`ORDER_ASCENDING`], with the id breaking
/// ties so the order is total and a page boundary cannot repeat or skip a job.
///
/// `resume_from` is the ordering key a caller's `nextToken` decoded to; a token that
/// decodes to nothing is refused by the handler that read it, with its modelled
/// `BadRequestException`, so there is no failure left to answer here.
pub fn page_of_jobs<'a>(
    jobs: impl Iterator<Item = &'a JobRecord>,
    order: Option<&str>,
    resume_from: Option<&(u128, String)>,
    max_results: Option<i32>,
) -> JobPage<'a> {
    let mut matching: Vec<&JobRecord> = jobs.collect();
    matching.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    let order = order.unwrap_or(ORDER_DESCENDING);
    if order == ORDER_DESCENDING {
        matching.reverse();
    }
    let ascending = order == ORDER_ASCENDING;

    let start = match resume_from {
        None => 0,
        Some((nanos, id)) => {
            let anchor = (*nanos, id.as_str());
            // Resume at the first job strictly past the anchor key in this direction.
            // The anchor itself need not still be in the result set, so a job whose
            // status changed between pages is skipped, not an error.
            matching
                .iter()
                .position(|job| {
                    if ascending {
                        job.ordering_key() > anchor
                    } else {
                        job.ordering_key() < anchor
                    }
                })
                .unwrap_or(matching.len())
        }
    };

    // Both `maxResults` members carry the model's own 1..20 range, so the framework has
    // already refused anything outside it; the fallback only covers the absent case.
    let page_size = usize::try_from(max_results.unwrap_or(DEFAULT_PAGE_SIZE))
        .unwrap_or(DEFAULT_PAGE_SIZE as usize);
    let page = &matching[start.min(matching.len())..];
    let page = &page[..page_size.min(page.len())];
    // A token only when there is a further page to fetch.
    let next_token = (start + page.len() < matching.len())
        .then(|| page.last().map(|job| job.cursor()))
        .flatten();
    JobPage {
        jobs: page.to_vec(),
        next_token,
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

/// What the store did when asked to delete a queue.
#[derive(Debug)]
pub enum DeleteQueueOutcome {
    /// The queue is gone from the store.
    Deleted,
    /// No queue is stored under that name.
    NotFound,
    /// The queue is built in. `Queue`'s `Type`: "System queues are built in. You can't
    /// modify or delete system queues."
    SystemQueue,
    /// The queue still holds jobs that have not reached a terminal status, carrying how
    /// many are `SUBMITTED` and how many `PROGRESSING`.
    HasActiveJobs { submitted: i32, progressing: i32 },
}

/// What the store did when asked to accept a new record it mints the id of — a job, or
/// a jobs query.
#[derive(Debug)]
pub enum AcceptOutcome {
    /// The record is stored under this id.
    Stored(String),
    /// The store is at its capacity for that kind of record ([`MAX_JOBS`] jobs,
    /// [`MAX_JOBS_QUERIES`] jobs queries).
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
    /// Presets by `Name`; "Each name must be unique within your account". At most
    /// [`MAX_PRESETS`] entries.
    pub presets: BTreeMap<String, PresetRecord>,
    /// The account's input policy, once `PutPolicy` has stored one. `None` until then
    /// and again after `DeletePolicy`, when the account carries no policy and
    /// `GetPolicy` reports none. A single record, never a growing collection: the
    /// policy API names no resource.
    pub policy: Option<PolicyRecord>,
    /// Job templates by `Name`; "Each name must be unique within your account". Seeded
    /// with the built-in [`SYSTEM_JOB_TEMPLATES`]; a caller's own templates join them
    /// through `CreateJobTemplate`, which holds the total to [`MAX_JOB_TEMPLATES`].
    pub job_templates: BTreeMap<String, JobTemplateRecord>,
    /// The account's API endpoint URL per region, keyed by the region of the request
    /// that first asked for it in `DEFAULT` mode. At most [`MAX_ENDPOINTS`] entries.
    /// Only `DescribeEndpoints` in `DEFAULT` mode adds one; nothing ever rewrites one,
    /// which is what makes the URL a caller reads back stable.
    pub endpoints: BTreeMap<String, String>,
    /// Jobs queries by `Id`, as `StartJobsQuery` stored them and
    /// `GetJobsQueryResults` reads them back. At most [`MAX_JOBS_QUERIES`] entries.
    pub jobs_queries: BTreeMap<String, JobsQueryRecord>,
    /// `ClientRequestToken` -> the job it created, so `CreateJob` is idempotent for
    /// [`IDEMPOTENCY_WINDOW`]. One entry per stored job at most, so it is bounded by
    /// [`MAX_JOBS`].
    pub idempotency: BTreeMap<String, IdempotencyEntry>,
    /// The jobs still owed their exit from `SUBMITTED`, ordered by the instant it comes
    /// due. [`Self::settle`] drains the already-due prefix, which is why settling never
    /// walks the job set.
    pending: BTreeSet<(SystemTime, String)>,
    /// The jobs queries still owed their exit from `SUBMITTED`, ordered by the instant it
    /// comes due, drained by [`Self::settle`] exactly as `pending` is.
    pending_queries: BTreeSet<(SystemTime, String)>,
    /// Monotonic suffix source for a minted job id; never reset, so no id repeats.
    job_sequence: u64,
    /// Monotonic suffix source for a minted jobs query id. See `job_sequence`.
    jobs_query_sequence: u64,
    /// How long after `CreateJob` a job is due to leave `SUBMITTED`.
    ///
    /// Defaults to [`JOB_SETTLES_AFTER`]. It is a field rather than a constant so a test
    /// can pin the span it needs — a long one to hold a job in `SUBMITTED` for the whole
    /// case, a short one to observe the transition — instead of racing the real clock.
    /// Nothing on a request path writes it.
    pub job_settles_after: Duration,
    /// How long after `StartJobsQuery` a jobs query is due to leave `SUBMITTED`.
    ///
    /// Defaults to [`JOBS_QUERY_COMPLETES_AFTER`], and is a field for the same reason
    /// `job_settles_after` is: a test pins the span it needs instead of racing the real
    /// clock. Nothing on a request path writes it.
    pub jobs_query_completes_after: Duration,
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
                concurrent_jobs: DEFAULT_CONCURRENT_JOBS,
                maximum_concurrent_feeds: DEFAULT_MAXIMUM_CONCURRENT_FEEDS,
                reservation_plan: None,
                created_at: now,
                last_updated: now,
                tags: BTreeMap::new(),
            },
        );
        Self {
            queues,
            jobs: BTreeMap::new(),
            presets: BTreeMap::new(),
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
            // No jobs query exists until a caller starts one.
            jobs_queries: BTreeMap::new(),
            idempotency: BTreeMap::new(),
            pending: BTreeSet::new(),
            pending_queries: BTreeSet::new(),
            job_sequence: 0,
            jobs_query_sequence: 0,
            job_settles_after: JOB_SETTLES_AFTER,
            jobs_query_completes_after: JOBS_QUERY_COMPLETES_AFTER,
        }
    }
}

impl ComAmazonawsMediaconvertMediaconvertState {
    /// Materialise every status change the clock had already made due at `now` — a job's
    /// exit from `SUBMITTED` and a jobs query's completion.
    ///
    /// Bounded by the number of records actually due, not by the size of the store: both
    /// pending indexes are ordered by due instant, so this drains their already-due
    /// prefix, oldest first across the two, and stops at the first entry still in the
    /// future. Each drained entry costs a bounded, constant amount of work — a job's
    /// status and timing, or a query's status flip — so the cost of one call is the
    /// number of records that came due and nothing else; neither kind re-reads the store.
    /// It is the only write a reporting operation makes, and every change it applies was
    /// owed at `now` whether or not anybody called.
    pub fn settle(&mut self, now: SystemTime) {
        loop {
            let next_job = self.pending.first().cloned();
            let next_query = self.pending_queries.first().cloned();
            match (next_job, next_query) {
                // The two timelines are drained as one, oldest due instant first, so
                // each change lands in the order the clock made it due.
                (Some(job), query) if query.as_ref().is_none_or(|(due, _)| job.0 <= *due) => {
                    if job.0 > now {
                        break;
                    }
                    self.pending.remove(&job);
                    self.settle_job(job.0, &job.1);
                }
                (_, Some(query)) => {
                    if query.0 > now {
                        break;
                    }
                    self.pending_queries.remove(&query);
                    self.complete_jobs_query(&query.1);
                }
                // Nothing is due. `(Some(_), None)` cannot reach here — a pending job
                // with no pending query always takes the first arm, whose guard holds
                // when there is no query — but guards do not count towards
                // exhaustiveness, so it is named rather than left to a catch-all.
                (None, None) | (Some(_), None) => break,
            }
        }
    }

    /// The exit from `SUBMITTED` the clock owed the job `id` at `due`.
    fn settle_job(&mut self, due: SystemTime, id: &str) {
        if let Some(job) = self.jobs.get_mut(id) {
            if job.status == JOB_STATUS_SUBMITTED {
                job.status = JOB_STATUS_ERROR.to_string();
                job.error_message = Some(NO_TRANSCODER_MESSAGE.to_string());
                job.timing.start_time = Some(due);
                job.timing.finish_time = Some(due);
            }
            job.settles_at = None;
        }
    }

    /// The completion the clock owed the jobs query `id`: it leaves the non-terminal
    /// `SUBMITTED` status for `COMPLETE`, and its results become readable.
    ///
    /// CONSTANT work per query, deliberately: the page was matched once when the query
    /// was started ([`Self::accept_jobs_query`]), so no amount of queries coming due in
    /// the same instant can make one `settle` call do more than a status flip each.
    fn complete_jobs_query(&mut self, id: &str) {
        if let Some(query) = self.jobs_queries.get_mut(id) {
            if query.status == JOBS_QUERY_STATUS_SUBMITTED {
                query.status = JOBS_QUERY_STATUS_COMPLETE.to_string();
            }
            // A completed query is terminal, so the clock owes it nothing further.
            query.completes_at = None;
        }
    }

    /// Store a newly started jobs query, refusing it once the store is at
    /// [`MAX_JOBS_QUERIES`].
    ///
    /// RUNS the query: the page its filters, order, cursor and page size select is
    /// matched here, once, and stored on the record — see [`JobsQueryRecord`] for why the
    /// match happens at the start rather than at the completion. The work is bounded by
    /// [`MAX_JOBS`] records times [`MAX_JOBS_QUERY_TERMS`] terms, and is paid by this one
    /// request; the results stay unreadable until the query completes.
    ///
    /// Owns the id, the starting status, the page and the `pending_queries` entry
    /// together, so they cannot be left inconsistent by a caller that forgets one.
    /// `resume_from` is the ordering key the request's `nextToken` decoded to.
    pub fn accept_jobs_query(
        &mut self,
        mut record: JobsQueryRecord,
        resume_from: Option<&(u128, String)>,
        now: SystemTime,
    ) -> AcceptOutcome {
        if self.jobs_queries.len() >= MAX_JOBS_QUERIES {
            return AcceptOutcome::AtCapacity;
        }
        let id = mint_id(now, &mut self.jobs_query_sequence, &self.jobs_queries);
        let due = now + self.jobs_query_completes_after;
        let page = page_of_jobs(
            self.jobs.values().filter(|job| record.matches(job)),
            record.order.as_deref(),
            resume_from,
            record.max_results,
        );
        record.id = id.clone();
        // SUBMITTED is not terminal, so the clock owes this query its completion.
        record.status = JOBS_QUERY_STATUS_SUBMITTED.to_string();
        record.completes_at = Some(due);
        record.matched = page.jobs.iter().map(|job| job.id.clone()).collect();
        record.result_next_token = page.next_token;
        self.pending_queries.insert((due, id.clone()));
        self.jobs_queries.insert(id.clone(), record);
        AcceptOutcome::Stored(id)
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
    /// See [`mint_id`]: the id is drawn from a stored sequence rather than a random
    /// source, so the id a caller is handed is the id that was persisted.
    fn mint_job_id(&mut self, now: SystemTime) -> String {
        mint_id(now, &mut self.job_sequence, &self.jobs)
    }

    /// Delete a queue: "Permanently delete a queue you have created."
    ///
    /// Two refusals live here rather than in the handler, so the decision and the state
    /// it is read from cannot disagree:
    ///
    /// * a built-in queue is not one the caller created, and `Queue`'s `Type` says
    ///   "System queues are built in. You can't modify or delete system queues";
    /// * a queue still holding a job in a non-terminal status conflicts with the current
    ///   state of the resource — removing it would leave those jobs' `Queue` naming a
    ///   queue no `GetQueue` can read.
    ///
    /// Nothing else is touched: the jobs a deleted queue already finished stay readable,
    /// exactly as their `Queue` recorded them.
    pub fn delete_queue(&mut self, name: &str) -> DeleteQueueOutcome {
        let Some(record) = self.queues.get(name) else {
            return DeleteQueueOutcome::NotFound;
        };
        if record.queue_type == QUEUE_TYPE_SYSTEM {
            return DeleteQueueOutcome::SystemQueue;
        }
        let (submitted, progressing) = self.job_counts(name);
        if submitted > 0 || progressing > 0 {
            return DeleteQueueOutcome::HasActiveJobs {
                submitted,
                progressing,
            };
        }
        self.queues.remove(name);
        DeleteQueueOutcome::Deleted
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

    /// Whether the store is at [`MAX_JOB_TEMPLATES`] and can hold no further template.
    pub fn job_templates_at_capacity(&self) -> bool {
        self.job_templates.len() >= MAX_JOB_TEMPLATES
    }

    /// Whether the store is at [`MAX_PRESETS`] and can hold no further preset.
    pub fn presets_at_capacity(&self) -> bool {
        self.presets.len() >= MAX_PRESETS
    }

    /// The tag set of the resource an ARN named, or `None` when this store holds no such
    /// resource.
    ///
    /// Tags are not a map of their own: they live ON the record they belong to, so a
    /// resource and its tags cannot drift apart and deleting the resource takes its tags
    /// with it.
    pub fn tags(&self, resource: &TaggedResource) -> Option<&BTreeMap<String, String>> {
        match resource {
            TaggedResource::Queue(name) => self.queues.get(name).map(|queue| &queue.tags),
            TaggedResource::Job(id) => self.jobs.get(id).map(|job| &job.tags),
            TaggedResource::JobTemplate(name) => {
                self.job_templates.get(name).map(|template| &template.tags)
            }
            TaggedResource::Preset(name) => self.presets.get(name).map(|preset| &preset.tags),
        }
    }

    /// The tag set of the resource an ARN named, for writing. See [`Self::tags`].
    pub fn tags_mut(
        &mut self,
        resource: &TaggedResource,
    ) -> Option<&mut BTreeMap<String, String>> {
        match resource {
            TaggedResource::Queue(name) => self.queues.get_mut(name).map(|queue| &mut queue.tags),
            TaggedResource::Job(id) => self.jobs.get_mut(id).map(|job| &mut job.tags),
            TaggedResource::JobTemplate(name) => self
                .job_templates
                .get_mut(name)
                .map(|template| &mut template.tags),
            TaggedResource::Preset(name) => {
                self.presets.get_mut(name).map(|preset| &mut preset.tags)
            }
        }
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

/// A fresh id no key of `taken` already holds.
///
/// Shaped like a MediaConvert resource id the service mints — the instant in epoch
/// milliseconds, a hyphen, then a short suffix — and drawn from the stored `sequence`
/// rather than a random source, so the id a caller is handed is the id that was
/// persisted. The loop terminates: the sequence strictly increases and every map it
/// mints into is capped well below the number of distinct suffixes.
fn mint_id<V>(now: SystemTime, sequence: &mut u64, taken: &BTreeMap<String, V>) -> String {
    let millis = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    loop {
        *sequence += 1;
        let candidate = format!("{millis}-{}", base36(*sequence));
        if !taken.contains_key(&candidate) {
            return candidate;
        }
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
