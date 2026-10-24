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
//!   growing the store, and a create whose `Name` the store already holds stores nothing
//!   at all (see [`CreateOutcome`]), so a client repeating a create cannot grow it
//!   either. The built-in system templates `job_templates` is seeded with are
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
//! * `job_engine_versions` is NOT caller-driven at all: the account's Job engine version
//!   catalogue is seeded once, at state construction, with exactly
//!   [`JOB_ENGINE_VERSION_COUNT`] records, and no request path adds, removes or rewrites
//!   one — so what `ListVersions` reports is stable across reads.
//! * `ListJobs` orders the jobs, over at most `MAX_JOBS` records, and returns at most
//!   one page of twenty, through [`page_of_jobs`]. `SearchJobs` goes through the same
//!   function, over the same bounded record set, with the input-file test
//!   ([`JobRecord::matches_input_file`]) narrowing it further. `StartJobsQuery` walks the same record
//!   set ONCE, when the query is started, evaluating at most [`MAX_JOBS_QUERY_TERMS`]
//!   filter terms per record, and stores only the one page of ids it selected; no later
//!   call re-runs that match, so no request's cost grows with the number of stored
//!   queries. `ListJobTemplates` orders the job templates the same way and pages them the
//!   same way, and `ListQueues` orders and pages the queues through [`page_of_queues`],
//!   over at most [`MAX_QUEUES`] records.
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
    AccelerationSettings, HopDestination, Job, JobEngineVersion, JobMessages, JobSettings,
    JobTemplate, JobTemplateSettings, Output, Policy, Preset, PresetSettings, Queue,
    ReservationPlan, ReservationPlanSettings, Timing,
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

/// The `InputPolicy` decision that forbids an input location
/// (`smithy_query(query="shape",shape="InputPolicy")`, whose generated value set is
/// [`crate::ops::types::INPUT_POLICY_VALUES`]).
pub const INPUT_POLICY_DISALLOWED: &str = "DISALLOWED";
/// The other `InputPolicy` decision: the one at least one input location must carry.
/// See [`INPUT_POLICY_DISALLOWED`].
pub const INPUT_POLICY_ALLOWED: &str = "ALLOWED";

/// Every input location the modelled `Policy` carries a decision for, in the Smithy
/// member spelling (`smithy_query(query="shape",shape="Policy")`): `S3Inputs`,
/// `HttpInputs`, `HttpsInputs`. [`PolicyRecord::allows_no_input`] and
/// [`policy_allows_no_input_message`] both read the set from here, so the condition that
/// is checked and the locations the refusal names cannot drift apart.
pub const INPUT_LOCATION_MEMBERS: [&str; 3] = ["S3Inputs", "HttpInputs", "HttpsInputs"];

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

/// The region-wide ceiling `ListQueues$TotalConcurrentJobs` reports: "The maximum number
/// of jobs that MediaConvert can process at one time, across all of your on-demand queues
/// in the current AWS Region."
///
/// A CHOSEN account quota, as [`MAX_QUEUES`] is: the model declares no value for it, and
/// it is a DIFFERENT quota from the per-queue one [`DEFAULT_CONCURRENT_JOBS`] comes from
/// (`CreateQueue$ConcurrentJobs`: "constrained by your service quotas for Maximum
/// concurrent jobs, per on-demand queue"). It is deliberately above that per-queue value
/// so the built-in `Default` queue's allocation leaves the account headroom to allocate,
/// which is what `UnallocatedConcurrentJobs` reports (see
/// [`ComAmazonawsMediaconvertMediaconvertState::concurrency_allocation`]).
pub const TOTAL_CONCURRENT_JOBS: i32 = 1_000;

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

/// How many Job engine versions the account's catalogue carries.
///
/// A CHOSEN count: `ListVersions` ("Retrieve a JSON array of all available Job engine
/// versions and the date they expire") documents that an account has a catalogue of them
/// and the model carries no list of its entries, so the SIZE of the seeded catalogue is
/// the emulator's own, as [`SYSTEM_JOB_TEMPLATES`] is for job templates. Four is well
/// inside one page of the operation's `__integerMin1Max20` `MaxResults`.
pub const JOB_ENGINE_VERSION_COUNT: usize = 4;

/// How far apart two consecutive Job engine versions of that catalogue are released.
///
/// A CHOSEN span, not a documented one: `JobEngineVersion$Version` says only that the
/// versions "represent periodically grouped MediaConvert releases" and "are in a
/// YYYY-MM-DD format" — that they are periodic, never what the period is. Ninety days
/// puts consecutive releases on distinct calendar dates, which is what makes the
/// `YYYY-MM-DD` label of each entry unique.
pub const JOB_ENGINE_VERSION_INTERVAL: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// How long after its release a Job engine version expires.
///
/// A CHOSEN span, not a documented one: `JobEngineVersion$ExpirationDate` ("The date that
/// this Job engine version expires. Requests to create jobs with an expired version result
/// in a regular job") documents THAT a version expires, never how long it lasts. A year,
/// which is longer than the whole seeded catalogue spans
/// (`JOB_ENGINE_VERSION_COUNT - 1` intervals), so every entry of a freshly constructed
/// account's catalogue is still available.
pub const JOB_ENGINE_VERSION_LIFETIME: Duration = Duration::from_secs(365 * 24 * 60 * 60);

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

/// How this service refuses a `Create*` whose `Name` a BUILT-IN resource already holds.
///
/// A create whose name is already taken by a resource the CALLER created is idempotent
/// (see [`CreateOutcome`]), so the only name a create is refused for is one no `Create*`
/// call ever minted — which is the modelled `ConflictException`. As with
/// [`not_found_message`], the wording names the request member the name came from, never
/// the value supplied.
pub fn system_resource_name_message(member: &str) -> String {
    format!(
        "{} That name belongs to a system resource.",
        invalid_value_sentence(member)
    )
}

/// The sentence EVERY built-in-resource refusal opens with, so the recorded wording lives
/// in one format string and only the clause that says what was wrong with the request
/// differs.
fn invalid_value_sentence(member: &str) -> String {
    format!("You specified an invalid value for {member}.")
}

/// The wording the two refusals of an ACTION on a built-in resource share (deleting one
/// and modifying one), built on [`invalid_value_sentence`]; only the action the request
/// asked for differs.
fn system_resource_message(member: &str, action: &str) -> String {
    format!(
        "{} You are not allowed to {action} system resources.",
        invalid_value_sentence(member)
    )
}

/// How this service refuses a `PutPolicy` that allows no input location at all.
///
/// AWS states the constraint — a policy requires at least one input location to be set
/// to `ALLOWED` — so the refusal names that constraint and the locations it covers,
/// taken from [`INPUT_LOCATION_MEMBERS`] rather than a second hand-written list. What
/// the API requires is the 400/`BadRequestException` pair plus a message naming the
/// constraint; this wording is this emulator's rendering of it.
pub fn policy_allows_no_input_message() -> String {
    format!(
        "You set your input policy to {disallowed} for all input locations: {all}. Policies \
         require at least one input location to be set to {allowed}. Set at least one of \
         {any} to {allowed} and resubmit your policy.",
        disallowed = INPUT_POLICY_DISALLOWED,
        allowed = INPUT_POLICY_ALLOWED,
        all = input_locations_joined("and"),
        any = input_locations_joined("or"),
    )
}

/// [`INPUT_LOCATION_MEMBERS`] as an English list — "A, B, <conjunction> C" — so the
/// refusal above reads as a sentence without hard-coding the member names twice.
fn input_locations_joined(conjunction: &str) -> String {
    match INPUT_LOCATION_MEMBERS.split_last() {
        Some((last, leading)) if !leading.is_empty() => {
            format!("{}, {conjunction} {last}", leading.join(", "))
        }
        _ => INPUT_LOCATION_MEMBERS.join(", "),
    }
}

/// How this service refuses a request that pairs a reservation plan with a queue whose
/// pricing plan is on-demand.
///
/// `ReservationPlanSettings` is "required for reserved queues and not applicable to
/// on-demand queues", so both the create and the update that carry one for an on-demand
/// queue state the same refusal; the sentence lives here so the two cannot drift.
pub fn reservation_plan_not_applicable_message() -> String {
    format!(
        "ReservationPlanSettings does not apply when PricingPlan is {}.",
        PRICING_PLAN_ON_DEMAND
    )
}

/// As [`reservation_plan_not_applicable_message`], for a reserved queue created without
/// the plan it requires.
pub fn reservation_plan_required_message() -> String {
    format!(
        "ReservationPlanSettings is required when PricingPlan is {}.",
        PRICING_PLAN_RESERVED
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
    resource_name_from_reference(RESOURCE_TYPE_QUEUES, reference)
}

/// The job template name a caller named, whether they gave the name or the template's
/// ARN.
///
/// `CreateJob$JobTemplate` ("when you create a job, you can either specify a job template
/// or specify the transcoding settings individually") is a reference to a job template
/// resource, so it accepts the same two forms every resource reference in this service
/// does; the resource id in a MediaConvert job template ARN is `jobTemplates/<name>`.
pub fn job_template_name_from_reference(reference: &str) -> &str {
    resource_name_from_reference(RESOURCE_TYPE_JOB_TEMPLATES, reference)
}

/// The resource name inside a reference that is either a bare name or the ARN of a
/// MediaConvert resource of `resource_type`.
///
/// One function for every kind of reference this service resolves, because the resource id
/// of a MediaConvert ARN is `<plural type>/<name>` for all of them (see
/// [`TaggedResource::resource_id`]); a value that is not an ARN is the name itself.
fn resource_name_from_reference<'a>(resource_type: &str, reference: &'a str) -> &'a str {
    let separator = format!(":{resource_type}/");
    match reference.rsplit_once(separator.as_str()) {
        Some((prefix, name)) if prefix.starts_with("arn:") => name,
        _ => reference,
    }
}

/// The queue reference of every hop destination that names one, in request order.
///
/// `HopDestination$Queue` is "Optional unless the job is submitted on the default queue.
/// When you set up a job to use queue hopping, you can specify a destination queue" — so a
/// hop destination that names no queue references nothing, and one that DOES name a queue
/// references a queue resource exactly as `CreateJob$Queue` does.
pub fn hop_destination_queues(hops: &[HopDestination]) -> impl Iterator<Item = &str> + '_ {
    hops.iter().filter_map(|hop| hop.queue.as_deref())
}

/// What a submitted job's settings graph is missing for the job to describe a transcode at
/// all.
///
/// `JobSettings$Inputs` is "Use Inputs to define source file used in the transcode job" and
/// `JobSettings$OutputGroups` is "Contains one group of settings for each set of outputs":
/// a job with no input names nothing to read and a job with no output group names nothing
/// to write, so in either case there is no transcode for the service to run. MediaConvert
/// validates the settings graph when the job is SUBMITTED — the error envelope's
/// `settingsValidationErrorsJsonBlob` member exists to report exactly that — and refuses
/// such a request with `BadRequestException` (400) rather than storing a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobSettingsProblem {
    /// Nothing supplies an input, so the job names no source.
    NoInputs,
    /// Nothing supplies an output group, so the job names no outputs.
    NoOutputGroups,
}

impl JobSettingsProblem {
    /// The refusal's wording: the settings member that must carry an entry and what it
    /// describes. Built from the request's own members, as every member-naming refusal in
    /// this crate is (see [`not_found_message`]), never from a recorded sentence.
    pub fn message(self) -> String {
        match self {
            Self::NoInputs => {
                "Your job settings must specify at least one entry in Inputs: use Inputs to \
                 define the source file used in the transcode job."
            }
            Self::NoOutputGroups => {
                "Your job settings must specify at least one entry in OutputGroups: use \
                 OutputGroups to define each set of outputs the transcode produces, or \
                 create the job from a job template that supplies them."
            }
        }
        .to_string()
    }
}

/// What is missing from the settings a `CreateJob` request submits, or `None` when they
/// describe a transcode.
///
/// `template` is the stored settings of the job template the request referenced, when it
/// referenced one: a job template is "a pre-made set of encoding instructions that you can
/// use to quickly create a job", so a job that carries only its inputs and takes its
/// outputs from its template describes a transcode just as fully as one that spells both
/// out. The EFFECTIVE graph — what the request carries plus what the template supplies — is
/// therefore what is tested here. An absent list and an empty list are the same thing: a
/// list with no entries supplies nothing.
pub fn job_settings_problem(
    settings: &JobSettings,
    template: Option<&JobTemplateSettings>,
) -> Option<JobSettingsProblem> {
    let inputs = settings.inputs.iter().flatten().count()
        + template.map_or(0, |template| template.inputs.iter().flatten().count());
    if inputs == 0 {
        return Some(JobSettingsProblem::NoInputs);
    }
    let output_groups = settings.output_groups.iter().flatten().count()
        + template.map_or(0, |template| template.output_groups.iter().flatten().count());
    if output_groups == 0 {
        return Some(JobSettingsProblem::NoOutputGroups);
    }
    None
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

    /// The pair a paged listing orders this queue on: its creation instant and its name,
    /// which is unique within the account ("Each name must be unique within your
    /// account"), so the pair is a total order.
    pub fn ordering_key(&self) -> (u128, &str) {
        (epoch_nanos(self.created_at), self.name.as_str())
    }

    /// This queue's [`encode_cursor`] token.
    pub fn cursor(&self) -> String {
        encode_cursor(self.created_at, &self.name)
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
            JOBS_QUERY_FILTER_KEY_FILE_INPUT => self.matches_input_file(value),
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

    /// Whether any input of this job names a file whose `FileInput` CONTAINS `needle`.
    ///
    /// Both jobs readers that filter on an input file take this one test — the jobs
    /// query's `fileInput` filter key ("Your input file URL, or partial input file name")
    /// and `SearchJobs`'s `InputFile` ("Provide your input file URL or your partial input
    /// file name") — so a whole URL and any part of one both match, and the two cannot
    /// drift apart.
    pub fn matches_input_file(&self, needle: &str) -> bool {
        self.input_files().any(|file| file.contains(needle))
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

/// Which key a queue listing is ordered on, as `listBy` chooses it. `QueueListBy`'s doc:
/// "you can choose to list them alphabetically by NAME or chronologically by
/// CREATION_DATE. If you don't specify, the service will list them by creation date."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueSortKey {
    /// Alphabetically by `Name`. Names are unique within the account, so this key alone
    /// is a total order.
    Name,
    /// Chronologically by `CreatedAt`, with the name breaking ties.
    CreationDate,
}

impl QueueSortKey {
    /// The key the `listBy` a request carried names, or the documented default
    /// ([`LIST_BY_CREATION_DATE`]) when it carried none. `QueueListBy` declares only the
    /// two values, and the boundary validator has already refused anything else, so a
    /// value that is not [`LIST_BY_NAME`] is the creation-date key.
    pub fn from_list_by(list_by: Option<&str>) -> Self {
        if list_by.unwrap_or(LIST_BY_CREATION_DATE) == LIST_BY_NAME {
            Self::Name
        } else {
            Self::CreationDate
        }
    }

    /// Where one queue's [`QueueRecord::ordering_key`] falls against another's on this
    /// key.
    fn compare(self, left: (u128, &str), right: (u128, &str)) -> ::std::cmp::Ordering {
        match self {
            Self::Name => left.1.cmp(right.1),
            Self::CreationDate => left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)),
        }
    }
}

/// One page of queues and the cursor that resumes after it.
#[derive(Debug)]
pub struct QueuePage<'a> {
    pub queues: Vec<&'a QueueRecord>,
    /// The [`encode_cursor`] token of the last queue of the page, and only when a further
    /// page exists.
    pub next_token: Option<String>,
}

/// The page of `queues` `ListQueues` reports, ordered on the key `list_by` names in the
/// direction `order` names.
///
/// The queue listing's own [`page_of_jobs`]: ONE place that owns the ordering, the cursor
/// and the page size, so a later paged queue reader cannot drift from this one. A queue
/// listing runs UP its key — alphabetically from the start of the alphabet,
/// chronologically from the oldest, which is the account's built-in `Default` queue —
/// unless the caller asks for [`ORDER_DESCENDING`]; `Order`'s doc says only "Default
/// varies by resource".
///
/// `resume_from` is the ordering key a caller's `nextToken` decoded to; a token that
/// decodes to nothing is refused by the handler that read it, with its modelled
/// `BadRequestException`, so there is no failure left to answer here.
pub fn page_of_queues<'a>(
    queues: impl Iterator<Item = &'a QueueRecord>,
    list_by: Option<&str>,
    order: Option<&str>,
    resume_from: Option<&(u128, String)>,
    max_results: Option<i32>,
) -> QueuePage<'a> {
    let sort_key = QueueSortKey::from_list_by(list_by);
    let mut matching: Vec<&QueueRecord> = queues.collect();
    matching.sort_by(|left, right| sort_key.compare(left.ordering_key(), right.ordering_key()));
    let ascending = order.unwrap_or(ORDER_ASCENDING) != ORDER_DESCENDING;
    if !ascending {
        matching.reverse();
    }

    let start = match resume_from {
        None => 0,
        Some((nanos, name)) => {
            let anchor = (*nanos, name.as_str());
            // Resume at the first queue strictly past the anchor key in this direction.
            // The anchor itself need not still be stored, so a queue deleted between two
            // pages is skipped rather than failing the token.
            matching
                .iter()
                .position(|queue| {
                    let side = sort_key.compare(queue.ordering_key(), anchor);
                    if ascending {
                        side == ::std::cmp::Ordering::Greater
                    } else {
                        side == ::std::cmp::Ordering::Less
                    }
                })
                .unwrap_or(matching.len())
        }
    };

    // `maxResults` carries the model's own 1..20 range, so the framework has already
    // refused anything outside it; the fallback only covers the absent case.
    let page_size = usize::try_from(max_results.unwrap_or(DEFAULT_PAGE_SIZE))
        .unwrap_or(DEFAULT_PAGE_SIZE as usize);
    let page = &matching[start.min(matching.len())..];
    let page = &page[..page_size.min(page.len())];
    // A token only when there is a further page to fetch.
    let next_token = (start + page.len() < matching.len())
        .then(|| page.last().map(|queue| queue.cursor()))
        .flatten();
    QueuePage {
        queues: page.to_vec(),
        next_token,
    }
}

/// One Job engine version of the account's catalogue: "periodically grouped MediaConvert
/// releases", each with "the date that this Job engine version expires".
///
/// Both instants are stored, so the `version` label and the `ExpirationDate` a caller
/// reads are the ones the catalogue was built with rather than values recomputed per
/// request. Nothing on a request path writes a record of this kind.
#[derive(Debug, Clone)]
pub struct JobEngineVersionRecord {
    /// The release date in the `YYYY-MM-DD` form `JobEngineVersion$Version` documents, and
    /// this catalogue's key.
    pub version: String,
    /// When the version was released — the instant `version` labels, and half of the
    /// ordering key a paged listing resumes on.
    pub released_at: SystemTime,
    /// When it expires, as `ExpirationDate` reports it.
    pub expires_at: SystemTime,
}

impl JobEngineVersionRecord {
    /// The catalogue entry for a version released at `released_at`: its label is that
    /// date, and it expires a [`JOB_ENGINE_VERSION_LIFETIME`] later.
    ///
    /// `checked_add` rather than `+`, so an instant near the end of the representable
    /// range cannot panic; it falls back to the release instant, which simply makes the
    /// entry already expired.
    pub fn released(released_at: SystemTime) -> Self {
        Self {
            version: job_engine_version_label(released_at),
            released_at,
            expires_at: released_at
                .checked_add(JOB_ENGINE_VERSION_LIFETIME)
                .unwrap_or(released_at),
        }
    }

    /// Whether this version is still one of the "available Job engine versions"
    /// `ListVersions` reports at `now` — an expired one is past the date it expires.
    pub fn is_available_at(&self, now: SystemTime) -> bool {
        self.expires_at > now
    }

    /// The modelled `JobEngineVersion`.
    pub fn view(&self) -> JobEngineVersion {
        JobEngineVersion {
            expiration_date: Some(epoch_seconds(self.expires_at)),
            version: Some(self.version.clone()),
        }
    }

    /// The pair a paged listing orders this version on: its release instant and its label,
    /// which is this catalogue's key, so the pair is a total order.
    pub fn ordering_key(&self) -> (u128, &str) {
        (epoch_nanos(self.released_at), self.version.as_str())
    }

    /// This version's [`encode_cursor`] token.
    pub fn cursor(&self) -> String {
        encode_cursor(self.released_at, &self.version)
    }
}

/// A Job engine version's label: the release date in the `YYYY-MM-DD` form
/// `JobEngineVersion$Version` documents ("Job engine versions are in a YYYY-MM-DD
/// format").
///
/// The calendar conversion is the LIBRARY's, never hand-rolled date arithmetic:
/// `DateTime::fmt(Format::DateTime)` renders an instant as RFC 3339 in UTC
/// (`YYYY-MM-DDThh:mm:ssZ`), whose first ten ASCII characters are exactly that calendar
/// date. A rendering that somehow came back shorter is taken whole, so there is nothing
/// to unwrap.
fn job_engine_version_label(at: SystemTime) -> String {
    let rendered = epoch_seconds(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    rendered.get(..10).unwrap_or(&rendered).to_string()
}

/// One page of Job engine versions and the cursor that resumes after it.
#[derive(Debug)]
pub struct JobEngineVersionPage<'a> {
    pub versions: Vec<&'a JobEngineVersionRecord>,
    /// The [`encode_cursor`] token of the last version of the page, and only when a
    /// further page exists.
    pub next_token: Option<String>,
}

/// The page of Job engine versions `ListVersions` reports.
///
/// The version catalogue's own [`page_of_queues`]: ONE place that owns the ordering, the
/// cursor and the page size. `ListVersions` models neither a sort key nor an `Order`, so
/// the one ordering is CHOSEN — chronological by release, oldest first, with the label
/// breaking ties so the order is total and a page boundary cannot repeat or skip an entry.
///
/// `resume_from` is the ordering key a caller's `nextToken` decoded to; a token that
/// decodes to nothing is refused by the handler that read it, with its modelled
/// `BadRequestException`, so there is no failure left to answer here.
pub fn page_of_versions<'a>(
    versions: impl Iterator<Item = &'a JobEngineVersionRecord>,
    resume_from: Option<&(u128, String)>,
    max_results: Option<i32>,
) -> JobEngineVersionPage<'a> {
    let mut matching: Vec<&JobEngineVersionRecord> = versions.collect();
    matching.sort_by(|left, right| left.ordering_key().cmp(&right.ordering_key()));

    let start = match resume_from {
        None => 0,
        Some((nanos, version)) => {
            let anchor = (*nanos, version.as_str());
            // Resume at the first version strictly past the anchor key. The anchor itself
            // need not still be listed, so a version that expired between two pages is
            // skipped rather than failing the token.
            matching
                .iter()
                .position(|record| record.ordering_key() > anchor)
                .unwrap_or(matching.len())
        }
    };

    // `maxResults` carries the model's own 1..20 range, so the framework has already
    // refused anything outside it; the fallback only covers the absent case.
    let page_size = usize::try_from(max_results.unwrap_or(DEFAULT_PAGE_SIZE))
        .unwrap_or(DEFAULT_PAGE_SIZE as usize);
    let page = &matching[start.min(matching.len())..];
    let page = &page[..page_size.min(page.len())];
    // A token only when there is a further page to fetch.
    let next_token = (start + page.len() < matching.len())
        .then(|| page.last().map(|record| record.cursor()))
        .flatten();
    JobEngineVersionPage {
        versions: page.to_vec(),
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

    /// Whether this policy leaves NO input location allowed.
    ///
    /// A location the policy does not state carries no decision — it is allowed, which
    /// is also what an account with no policy at all allows — so the policy allows
    /// nothing only when EVERY input location the `Policy` shape models
    /// ([`INPUT_LOCATION_MEMBERS`]) is explicitly `DISALLOWED`. This is the condition
    /// AWS states a policy must not meet: "Policies require at least one input location
    /// to be set to ALLOWED."
    pub fn allows_no_input(&self) -> bool {
        self.decisions()
            .iter()
            .all(|decision| decision.as_deref() == Some(INPUT_POLICY_DISALLOWED))
    }

    /// This record's decisions, one per member of [`INPUT_LOCATION_MEMBERS`] and in that
    /// order. The array's length IS the member list's, so an input location added to one
    /// and not the other does not compile.
    fn decisions(&self) -> [&Option<String>; INPUT_LOCATION_MEMBERS.len()] {
        [&self.s3_inputs, &self.http_inputs, &self.https_inputs]
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
    /// The job was ALREADY `CANCELED`, so the request asked for the state it is in and
    /// nothing was changed: the repeat is a no-op success, not a conflict.
    AlreadyCanceled,
    /// No job is stored under that id.
    NotFound,
    /// The job has left `SUBMITTED` for a status other than `CANCELED`, carrying the
    /// status it is in now.
    NotCancelable(String),
}

/// What the store did when asked to delete a queue.
#[derive(Debug)]
pub enum DeleteQueueOutcome {
    /// The queue is gone from the store.
    Deleted,
    /// No queue was stored under that name, so the store is already in the state the
    /// request asked for and nothing was removed: the repeat is a no-op success. The
    /// READ paths are unaffected — `GetQueue` on that name is still the modelled 404.
    AlreadyAbsent,
    /// The queue is built in. `Queue`'s `Type`: "System queues are built in. You can't
    /// modify or delete system queues."
    SystemQueue,
    /// The queue still holds jobs that have not reached a terminal status, carrying how
    /// many are `SUBMITTED` and how many `PROGRESSING`.
    HasActiveJobs { submitted: i32, progressing: i32 },
}

/// What the store did when asked to create a resource whose `Name` is its identity — a
/// queue, a job template or a preset.
///
/// A create whose `Name` is already stored does NOT conflict: MediaConvert answers the
/// repeat with the operation's success status and the resource that is already there, so
/// a provisioning path that runs twice gets the resource back rather than a hard
/// failure. The stored record is handed back UNCHANGED — a repeat create never edits the
/// resource it finds, so a later read sees what the FIRST create persisted.
#[derive(Debug)]
pub enum CreateOutcome<V> {
    /// Nothing held that name; the record is now stored, and this is its projection.
    Created(V),
    /// A resource the caller could have created already holds that name. This is the
    /// projection of the record that was ALREADY stored, which the create left as it was.
    Existing(V),
    /// A BUILT-IN ([`TYPE_SYSTEM`]) resource holds that name. No `Create*` call ever
    /// produced it, so there is no create for a repeat to be idempotent with, and the
    /// name genuinely conflicts with the current state of the store.
    SystemResource,
    /// The store is at its capacity for that kind of record ([`MAX_QUEUES`] queues,
    /// [`MAX_JOB_TEMPLATES`] job templates, [`MAX_PRESETS`] presets).
    AtCapacity,
}

/// A record stored under the `Name` that is its identity.
///
/// Implemented by the three records a `Create*` operation mints — a queue, a job
/// template and a preset — so [`create_named`] can own the one rule they share: the
/// existence probe, the idempotent return and the insert, in a single map lookup.
pub trait NamedRecord {
    /// The `Name` this record is stored under.
    fn record_name(&self) -> &str;

    /// Whether this is a BUILT-IN resource, which no caller created and no caller can
    /// re-create ([`TYPE_SYSTEM`]).
    fn is_system(&self) -> bool;
}

impl NamedRecord for QueueRecord {
    fn record_name(&self) -> &str {
        &self.name
    }

    fn is_system(&self) -> bool {
        self.queue_type == QUEUE_TYPE_SYSTEM
    }
}

impl NamedRecord for JobTemplateRecord {
    fn record_name(&self) -> &str {
        &self.name
    }

    fn is_system(&self) -> bool {
        self.template_type == TYPE_SYSTEM
    }
}

impl NamedRecord for PresetRecord {
    fn record_name(&self) -> &str {
        &self.name
    }

    fn is_system(&self) -> bool {
        self.preset_type == TYPE_SYSTEM
    }
}

/// Store `record` under its own name, idempotently.
///
/// ONE rule for every named resource this service creates, so no two create paths can
/// disagree about it:
///
/// * a name nothing holds stores the record and projects it — [`CreateOutcome::Created`];
/// * a name a caller-created record already holds stores NOTHING and projects the record
///   that is there — [`CreateOutcome::Existing`], the idempotent repeat;
/// * a name a built-in record holds is [`CreateOutcome::SystemResource`];
/// * a store already at `capacity` is [`CreateOutcome::AtCapacity`], so the idempotent
///   repeat above is reached even when the store is full — it adds nothing.
///
/// The probe and the insert happen under the one `&mut` borrow the caller holds the state
/// lock for, so two concurrent creates of the same name cannot both insert.
fn create_named<R: NamedRecord, V>(
    store: &mut BTreeMap<String, R>,
    capacity: usize,
    record: R,
    view: impl Fn(&R) -> V,
) -> CreateOutcome<V> {
    if let Some(existing) = store.get(record.record_name()) {
        if existing.is_system() {
            return CreateOutcome::SystemResource;
        }
        return CreateOutcome::Existing(view(existing));
    }
    if store.len() >= capacity {
        return CreateOutcome::AtCapacity;
    }
    let projection = view(&record);
    store.insert(record.record_name().to_string(), record);
    CreateOutcome::Created(projection)
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
    /// The account's Job engine version catalogue by `Version` label, as `ListVersions`
    /// reports it. Seeded with [`JOB_ENGINE_VERSION_COUNT`] entries when the account comes
    /// up — the service publishes these versions, no caller creates one — and never
    /// written by a request path, so the catalogue a caller reads is the one that was
    /// persisted. Keyed by the label, which is unique because consecutive releases are a
    /// [`JOB_ENGINE_VERSION_INTERVAL`] apart.
    pub job_engine_versions: BTreeMap<String, JobEngineVersionRecord>,
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
            // The service's own Job engine versions: the newest released now and the rest
            // one interval apart behind it, so every entry of a fresh account's catalogue
            // is inside its lifetime and none is dated in the future. `checked_sub` rather
            // than `-`, so an instant near the start of the representable range cannot
            // panic — it simply yields a shorter catalogue.
            job_engine_versions: (0..JOB_ENGINE_VERSION_COUNT)
                .filter_map(|index| {
                    let age = JOB_ENGINE_VERSION_INTERVAL
                        .checked_mul(u32::try_from(index).ok()?)?;
                    let record = JobEngineVersionRecord::released(now.checked_sub(age)?);
                    Some((record.version.clone(), record))
                })
                .collect(),
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

    /// Whether this store holds the queue a reference names, in either form a queue
    /// reference comes in ([`queue_name_from_reference`]).
    ///
    /// For a reference the caller has NOT already reduced to a name: the hop destinations
    /// of a job or of a job template, each of which may name its queue either way. A
    /// handler that resolved the name itself — because it STORES it, as `CreateJob` stores
    /// the name of the queue the job was submitted to — probes `queues` with that name.
    pub fn holds_queue(&self, reference: &str) -> bool {
        self.queues
            .contains_key(queue_name_from_reference(reference))
    }

    /// The stored settings of the job template a reference names, or `None` when this
    /// store holds no template under that name.
    ///
    /// The template is resolved from the reference exactly as a queue is
    /// ([`job_template_name_from_reference`]), so a caller may name it by name or by ARN.
    pub fn job_template_settings(&self, reference: &str) -> Option<&JobTemplateSettings> {
        self.job_templates
            .get(job_template_name_from_reference(reference))
            .map(|template| &template.settings)
    }

    /// Cancel a stored job: "Permanently cancel a job. Once you have canceled a job, you
    /// can't start it again." Only a job still waiting in its queue can be cancelled.
    pub fn cancel_job(&mut self, id: &str, now: SystemTime) -> CancelOutcome {
        let Some(job) = self.jobs.get_mut(id) else {
            return CancelOutcome::NotFound;
        };
        // A job already `CANCELED` is in the state the request asks for, so the repeat
        // changes nothing at all — not its status, not its finish time — and reports the
        // success the first cancel reported. Idempotent, so a teardown path that runs
        // twice does not have to tell "already cancelled" from a real failure.
        if job.status == JOB_STATUS_CANCELED {
            return CancelOutcome::AlreadyCanceled;
        }
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
    /// The delete is IDEMPOTENT on the name: a name the store does not hold is already in
    /// the state the request asks for, so it is reported as such rather than as a
    /// failure. The read paths are untouched — `GetQueue` on that name is still the
    /// modelled 404.
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
            return DeleteQueueOutcome::AlreadyAbsent;
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

    /// Store a queue `CreateQueue` asked for, idempotently on its `Name`.
    ///
    /// See [`create_named`] for the one rule every named create here follows; the two
    /// job counts the projection carries are read off `jobs`, so the queue a repeat
    /// create returns reports the counts it has NOW, exactly as `GetQueue` would.
    pub fn create_queue(&mut self, record: QueueRecord, region: &str) -> CreateOutcome<Queue> {
        let (submitted, progressing) = self.job_counts(record.record_name());
        create_named(&mut self.queues, MAX_QUEUES, record, |queue| {
            queue.view(region, submitted, progressing)
        })
    }

    /// Store a job template `CreateJobTemplate` asked for, idempotently on its `Name`.
    /// See [`create_named`].
    pub fn create_job_template(
        &mut self,
        record: JobTemplateRecord,
        region: &str,
    ) -> CreateOutcome<JobTemplate> {
        create_named(
            &mut self.job_templates,
            MAX_JOB_TEMPLATES,
            record,
            |template| template.view(region),
        )
    }

    /// Store a preset `CreatePreset` asked for, idempotently on its `Name`.
    /// See [`create_named`].
    pub fn create_preset(&mut self, record: PresetRecord, region: &str) -> CreateOutcome<Preset> {
        create_named(&mut self.presets, MAX_PRESETS, record, |preset| {
            preset.view(region)
        })
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

    /// `(TotalConcurrentJobs, UnallocatedConcurrentJobs)` as `ListQueues` reports them:
    /// the region-wide ceiling [`TOTAL_CONCURRENT_JOBS`], and how much of it no queue
    /// holds.
    ///
    /// `UnallocatedConcurrentJobs`: "The remaining number of concurrent jobs that are not
    /// associated with a queue and are available to allocate to a queue" — so it is the
    /// ceiling less what the stored queues have allocated, derived on every read rather
    /// than stored so it cannot drift from `queues`. Only `ON_DEMAND` queues count,
    /// because the ceiling itself is "across all of your on-demand queues": a `RESERVED`
    /// queue's slots are bought by its reservation plan, not taken from this pool. Summed
    /// in `i64` and clamped into the ceiling, so an account whose queues have allocated
    /// more than the pool reports nothing left to allocate rather than a negative count.
    pub fn concurrency_allocation(&self) -> (i32, i32) {
        let allocated: i64 = self
            .queues
            .values()
            .filter(|queue| queue.pricing_plan == PRICING_PLAN_ON_DEMAND)
            .map(|queue| i64::from(queue.concurrent_jobs))
            .sum();
        let ceiling = i64::from(TOTAL_CONCURRENT_JOBS);
        let unallocated = (ceiling - allocated).clamp(0, ceiling);
        (
            TOTAL_CONCURRENT_JOBS,
            i32::try_from(unallocated).unwrap_or(0),
        )
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
