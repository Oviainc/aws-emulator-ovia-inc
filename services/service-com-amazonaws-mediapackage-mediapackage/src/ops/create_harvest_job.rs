// SPDX-License-Identifier: CC0-1.0

//! @op CreateHarvestJob
//! Submits a HarvestJob: it resolves `OriginEndpointId` against the stored origin
//! endpoints, mints the job's ARN, and persists the record `DescribeHarvestJob` and
//! `ListHarvestJobs` then report. The job's `ChannelId` is read off the STORED endpoint
//! (the request carries none), so it is computed from state rather than echoed.
//!
//! The submitted job starts `IN_PROGRESS` and leaves it only when the clock makes its
//! harvest due — see [`crate::state::HARVEST_SETTLE_AFTER`] for the span, and
//! [`crate::state::harvest_export`] for what the harvest then WRITES (the top-level
//! manifest, into the destination bucket) and which modelled terminal status the outcome
//! of that write decides.
//!
//! ## What this handler decides about the harvest window, the endpoint and the destination
//!
//! A submission is accepted only when it can be carried out, and every check happens
//! before the store is touched, so a refused submission persists nothing:
//!
//! * the WINDOW is an interval of media, so `EndTime` must name an instant LATER than
//!   `StartTime`. The two are modelled as plain `__string` with no `@pattern`, no
//!   `@timestampFormat` and no relation between them
//!   (`smithy_query(query="operation",operation="CreateHarvestJob",…)`), so this handler
//!   reads them as ISO-8601 instants itself — text that is not an instant is refused as
//!   that member's own failure — and stores them as SENT, because the bounds the caller
//!   gets back are the caller's own text;
//! * the ENDPOINT has to hold content a harvest can export: a harvest reads already
//!   published content back out of the origin endpoint's STARTOVER (time-shift) window,
//!   so an endpoint whose startover playback is disabled retains nothing to harvest and
//!   the submission is refused ([`crate::state::OriginEndpointRecord::startover_enabled`],
//!   read off the stored endpoint);
//! * the DESTINATION is where the harvest's own output goes, so it is pre-flighted before
//!   the job exists ([`crate::state::harvest_destination`]): the ROLE is the identity
//!   MediaPackage writes as, so the value has to BE an IAM role ARN
//!   ([`crate::state::IAM_ROLE_ARN`]) and has to name a role this service could act as —
//!   one the account's IAM records hold, whose trust policy grants
//!   `mediapackage.amazonaws.com` `sts:AssumeRole` ([`crate::state::harvest_role_refusal`])
//!   — and the BUCKET has to be a name Amazon S3 holds a bucket under and a bucket this
//!   account HOLDS, which is asked of Amazon S3 itself through its own front door.
//!
//! All of them answer the modelled `UnprocessableEntityException` (422) keyed by the member
//! the caller has to fix. Whether the harvest window lies INSIDE the endpoint's startover
//! period is NOT decided here: nothing published states that bound for this service in a
//! form this emulator can apply, so guessing it would refuse submissions AWS accepts.
//!
//! They are decided in the order listed, which is the order a submission failing several of
//! them is told to fix them in: a precondition read off the STORED endpoint the request acts
//! on is reported before a refusal about the destination the service would have to write to,
//! because the destination is not what such a caller has to fix; and of the destination, the
//! role before the bucket, since a submission the service could not write as cannot be
//! written anywhere. The destination verdicts are computed against the OTHER services' own
//! records (the account's IAM store, the account's object store) BEFORE this service's guard
//! is taken — so no two orders of the same two locks exist, and the object store's front
//! door never waits on this service's lock — and HELD until the endpoint has been resolved
//! and its precondition decided, exactly as `UpdateOriginEndpoint` holds the same role
//! verdict until it has resolved the endpoint its request addresses.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateHarvestJob::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateHarvestJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")
//! ```
//!
//! Creates a new HarvestJob record.
//!
//! ## Input
//!
//! * `EndTime` (__string) **required** — The end of the time-window which will be harvested
//! * `Id` (__string) **required** — The ID of the HarvestJob. The ID must be unique within
//!   the region and it cannot be changed after the HarvestJob is submitted
//! * `OriginEndpointId` (__string) **required** — The ID of the OriginEndpoint that the
//!   HarvestJob will harvest from. This cannot be changed after the HarvestJob is submitted.
//! * `S3Destination` (S3Destination) **required** — Configuration parameters for where in an
//!   S3 bucket to place the harvested content
//! * `StartTime` (__string) **required** — The start of the time-window which will be
//!   harvested
//!
//! ## Output
//!
//! * `Arn` (__string) — The Amazon Resource Name (ARN) assigned to the HarvestJob.
//! * `ChannelId` (__string) — The ID of the Channel that the HarvestJob will harvest from.
//! * `CreatedAt` (__string) — The date and time the HarvestJob was submitted.
//! * `EndTime` (__string) — The end of the time-window which will be harvested.
//! * `Id` (__string) — The ID of the HarvestJob. The ID must be unique within the region and
//!   it cannot be changed after the HarvestJob is submitted.
//! * `OriginEndpointId` (__string) — The ID of the OriginEndpoint that the HarvestJob will
//!   harvest from. This cannot be changed after the HarvestJob is submitted.
//! * `S3Destination` (S3Destination) — Configuration parameters for where in an S3 bucket to
//!   place the harvested content
//! * `StartTime` (__string) — The start of the time-window which will be harvested.
//! * `Status` (Status, one of: FAILED | IN_PROGRESS | SUCCEEDED) — The current status of the
//!   HarvestJob. Consider setting up a CloudWatch Event to listen for HarvestJobs as they
//!   succeed or fail. In the event of failure, the CloudWatch Event will include an
//!   explanation of why the HarvestJob failed.
//!
//! ## Errors
//!
//! * `ForbiddenException` (HTTP 403) — The client is not authorized to access the requested
//!   resource.
//! * `InternalServerErrorException` (HTTP 500) — An unexpected error occurred.
//! * `NotFoundException` (HTTP 404) — The requested resource does not exist.
//! * `ServiceUnavailableException` (HTTP 503) — An unexpected error occurred.
//! * `TooManyRequestsException` (HTTP 429) — The client has exceeded their resource or
//!   throttling limits.
//! * `UnprocessableEntityException` (HTTP 422) — The parameters sent in the request are not
//!   valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateHarvestJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-mediapackage-mediapackage.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateHarvestJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.mediapackage#CreateHarvestJob")]
#[derive(Debug)]
pub enum CreateHarvestJobErrorModeled {
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    NotFoundException(NotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
    TooManyRequestsException(TooManyRequestsException),
    UnprocessableEntityException(UnprocessableEntityException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_harvest_job::CreateHarvestJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateHarvestJobError = crate::scaffolded::Scaffolded<CreateHarvestJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateHarvestJobErrorModeled> for CreateHarvestJobError {
    fn from(modeled: CreateHarvestJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateHarvestJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateHarvestJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateHarvestJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateHarvestJob<Eng>
{
    type Input = CreateHarvestJobInput;
    type Output = CreateHarvestJobOutput;
    type Error = CreateHarvestJobError;

    const NAME: &'static str = "CreateHarvestJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_HARVEST_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_HARVEST_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateHarvestJobErrorModeled::InternalServerErrorException(
                InternalServerErrorException {
                    message: Some("the mediapackage state store is unavailable".into()),
                },
            )
        })?;

        // Bound what the caller may make this record cost, before anything is stored: the
        // store copies these strings verbatim, so one request must not decide how much
        // memory its record occupies. Each refusal is this service's own envelope, keyed
        // by the member path the wire names.
        // The id this job is stored under is the `{Id}` label of its own reads and the
        // last segment of its ARN, so it is judged by the identifier rule all three
        // creates share, before anything else about the submission.
        crate::state::check_created_id(&input.id)?;
        for (member, value) in [
            ("originEndpointId", &input.origin_endpoint_id),
            ("startTime", &input.start_time),
            ("endTime", &input.end_time),
            ("s3Destination.bucketName", &input.s3_destination.bucket_name),
            ("s3Destination.manifestKey", &input.s3_destination.manifest_key),
            ("s3Destination.roleArn", &input.s3_destination.role_arn),
        ] {
            crate::state::bound_chars(member, value, crate::state::MAX_STRING_CHARS)?;
        }

        // The window is an interval of media, so its end has to be LATER than its start:
        // an inverted or empty window names no content to harvest. `StartTime` and
        // `EndTime` are modelled as `__string`, so this handler is what reads them as
        // instants — text that is not an instant at all is refused as that member's own
        // failure, which is a different answer from "your window is inverted". Decided
        // before the store is touched, so a refused submission persists nothing.
        let start = crate::state::parse_instant(&input.start_time)
            .ok_or_else(|| crate::state::instant_invalid("startTime"))?;
        let end = crate::state::parse_instant(&input.end_time)
            .ok_or_else(|| crate::state::instant_invalid("endTime"))?;
        if end <= start {
            return Err(crate::state::harvest_window_invalid().into());
        }

        // MediaPackage writes the harvested content to the caller's bucket AS the role the
        // destination names, so a submission it could never carry out is refused instead of
        // producing a job id for a harvest that cannot happen. The role has to BE an IAM
        // role ARN — a value that is not one names no role to assume, which is a value in
        // the request and nothing to do with any stored resource, so it is judged here with
        // the request's other members. The bucket is judged the same way for the same
        // reason: a value outside Amazon S3's published naming rules names no bucket
        // anywhere, so no provisioning could make such a submission servable.
        crate::state::check_arn_member(
            crate::state::HARVEST_ROLE_MEMBER,
            &input.s3_destination.role_arn,
            crate::state::IAM_ROLE_ARN,
        )?;
        crate::state::harvest_destination::check_bucket_name(&input.s3_destination.bucket_name)?;
        // Whether the service could ACT AS that role is decided against the account's IAM
        // records — a different store in the same engine, and no IAM handler reads this one;
        // its guard is taken and released BEFORE this service's, so no two orders of the
        // same two locks exist. The ANSWER is held and reported below, after the endpoint
        // the submission harvests has been resolved and its own precondition decided, so a
        // submission failing both is told to fix the endpoint rather than the destination.
        let iam = crate::state::iam_state(&*self.engine).ok_or_else(|| {
            CreateHarvestJobErrorModeled::InternalServerErrorException(
                InternalServerErrorException {
                    message: Some("the IAM store is unavailable".into()),
                },
            )
        })?;
        let role_refusal =
            crate::state::harvest_role_refusal(&input.s3_destination.role_arn, &iam.lock());

        // MediaPackage is regional: this job belongs to the Region this request is served
        // in, the endpoint it names resolves only among that Region's records, and the
        // destination is resolved in that Region's object store.
        let region = crate::state::request_region(ctx.parts());
        // Whether the account HOLDS the destination bucket is Amazon S3's own answer, asked
        // through its front door with no guard held, and held like the role's verdict above.
        // It is not asked at all for a submission the role already refuses: the role is
        // reported first, so nothing is resolved for a destination the service could not
        // write as.
        let bucket_refusal = if role_refusal.is_some() {
            None
        } else {
            crate::state::harvest_destination::bucket_refusal(
                ctx,
                &region,
                &input.s3_destination.bucket_name,
            )
            .await
        };

        let now = std::time::SystemTime::now();
        // The clock, not this call, carries a submitted job's harvest out and moves its
        // status: materialise first what was already due, so the duplicate-id rule below
        // and the response are decided against the jobs as the clock has left them. It is
        // done BEFORE this service's guard is taken, because carrying a harvest out writes
        // to another service's store through its own front door.
        crate::state::harvest_export::settle_due_harvests(&state, ctx, now).await;

        let mut state = state.lock();

        // A HarvestJob harvests ONE origin endpoint: an `OriginEndpointId` naming no
        // stored endpoint OF THIS REGION has nothing to harvest, so the job is not
        // created. This is a value in the REQUEST the caller has to fix — not a resource
        // the request addressed — so it is `UnprocessableEntityException` (422) keyed by
        // the member, exactly as `CreateOriginEndpoint` answers an unresolvable
        // `channelId`.
        let Some(endpoint) = state.origin_endpoint(&region, &input.origin_endpoint_id) else {
            return Err(crate::state::member_not_found("originEndpointId").into());
        };
        // A harvest exports content the endpoint has ALREADY published, which an endpoint
        // keeps only for as long as its startover (time-shift) window retains it: an
        // endpoint with startover playback disabled retains nothing, so there is nothing
        // for the job to harvest and the submission is refused — keyed by the member that
        // names the endpoint, decided against the stored record, and before the store is
        // written, so nothing is persisted and no later read reports a phantom job.
        if !endpoint.startover_enabled() {
            return Err(crate::state::startover_required("originEndpointId").into());
        }
        // The endpoint being harvested holds up its end, so the destination is now the thing
        // the caller has to fix, and the verdicts read off the other services' records above
        // are reported — the role first, then the bucket.
        if let Some(refusal) = role_refusal {
            return Err(refusal.into());
        }
        if let Some(refusal) = bucket_refusal {
            return Err(refusal.into());
        }
        // The channel harvested is the one the STORED endpoint packages; the request does
        // not carry it and the response must not invent it.
        let channel_id = endpoint.channel_id.clone();

        // "The ID must be unique within the region and it cannot be changed after the
        // HarvestJob is submitted": a second submit on an id already in use in this
        // Region is refused, never an overwrite.
        if state.has_harvest_job(&region, &input.id) {
            return Err(crate::state::resource_already_exists(
                crate::state::HARVEST_JOB_KIND,
                &input.id,
            )
            .into());
        }

        // The store lives for the life of the process and only a submit adds to it, so it
        // is capped by entry count — across every Region, because the memory the process
        // holds is the sum of them; past the cap the resource limit is reported.
        if state.harvest_jobs.len() >= crate::state::MAX_HARVEST_JOBS {
            return Err(crate::state::resource_limit_reached(
                crate::state::HARVEST_JOB_KIND,
                crate::state::MAX_HARVEST_JOBS,
            )
            .into());
        }

        let record = crate::state::HarvestJobRecord {
            id: input.id.clone(),
            region: region.clone(),
            arn: crate::state::harvest_job_arn(&region, &input.id),
            channel_id,
            origin_endpoint_id: input.origin_endpoint_id.clone(),
            s3_destination: input.s3_destination.clone(),
            start_time: input.start_time.clone(),
            end_time: input.end_time.clone(),
            created_at: crate::state::now_iso8601(),
            status: crate::state::HARVEST_IN_PROGRESS.to_string(),
            // The harvest the clock owes this job, stored as the instant it falls due;
            // nothing sleeps for it and no read brings it forward. Which terminal status
            // it reaches is decided when it runs, by whether its output was written —
            // nothing here fixes the outcome in advance.
            next_status_due: Some(now + crate::state::HARVEST_SETTLE_AFTER),
        };
        // `insert` + the read back below: the record returned is always the STORED one,
        // and the id is free (refused as a duplicate above), so nothing is replaced.
        let id = input.id.clone();
        state.insert_harvest_job(record);
        let stored = state
            .harvest_job(&region, &id)
            .expect("the record just inserted under this region and id");

        // Rendered from the STORED record, so this response and every later
        // DescribeHarvestJob/ListHarvestJobs are the same bytes.
        let job = stored.to_harvest_job();
        Ok(CreateHarvestJobOutput {
            arn: job.arn,
            channel_id: job.channel_id,
            created_at: job.created_at,
            end_time: job.end_time,
            id: job.id,
            origin_endpoint_id: job.origin_endpoint_id,
            s3_destination: job.s3_destination,
            start_time: job.start_time,
            status: job.status,
        })
    }
}
