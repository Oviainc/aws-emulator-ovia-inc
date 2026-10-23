// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3control-awss3controlservicev20180820 service.
//!
//! Besides the state struct itself this module holds the derivations every
//! handler shares — the access point ARN, alias, network origin and
//! `ListAccessPoints` page token, the Multi-Region Access Point alias and the
//! asynchronous request token ARN, the Storage Lens configuration ARN, and the
//! refusals the resource lifecycles answer — so the handlers under `src/ops/`
//! compute each one way instead of once per handler.

use crate::ops::types::{
    AccessPoint, AsyncOperation, AsyncRequestParameters, AsyncResponseDetails, CreateJobInput,
    CreateMultiRegionAccessPointInput2, DeleteMultiRegionAccessPointInput2, JobDescriptor,
    JobManifest, JobManifestGenerator, JobOperation, JobReport,
    MultiRegionAccessPointRegionalResponse, MultiRegionAccessPointReport,
    MultiRegionAccessPointsAsyncResponse, PublicAccessBlockConfiguration, RegionReport, S3Tag,
    StorageLensConfiguration, StorageLensTag, Tag, VpcConfiguration, ASYNC_OPERATION_NAME_VALUES,
    JOB_STATUS_VALUES, MULTI_REGION_ACCESS_POINT_STATUS_VALUES, NETWORK_ORIGIN_VALUES,
};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One stored access point: everything `GetAccessPoint`, `ListAccessPoints` and
/// `GetAccessPointPolicy` report, recorded by `CreateAccessPoint`.
#[derive(Debug, Clone)]
pub struct AccessPointRecord {
    pub name: String,
    /// The account that owns the access point — the first half of the key, kept
    /// on the record so a record passed on its own still names its owner.
    pub account_id: String,
    pub bucket: String,
    pub bucket_account_id: Option<String>,
    pub arn: String,
    pub alias: String,
    /// One of [`NETWORK_ORIGIN_VALUES`] — `VPC` when the access point is
    /// VPC-restricted, otherwise `Internet`.
    pub network_origin: String,
    pub vpc_configuration: Option<VpcConfiguration>,
    pub public_access_block_configuration: Option<PublicAccessBlockConfiguration>,
    pub creation_date: ::aws_smithy_types::DateTime,
    /// The tags `CreateAccessPoint` was given. Recorded here because they are
    /// part of the resource; the operations that report them
    /// (`ListTagsForResource`, `TagResource`, `UntagResource`) are still stubs
    /// with their own gap issues, and will read this field rather than a second
    /// copy.
    pub tags: Vec<Tag>,
    /// The document the last `PutAccessPointPolicy` stored, byte for byte;
    /// `None` until one is put, which is what `GetAccessPointPolicy` reports as
    /// `NoSuchAccessPointPolicy`.
    pub policy: Option<String>,
}

impl AccessPointRecord {
    /// The record as the `AccessPoint` summary shape `ListAccessPoints` returns.
    pub fn summary(&self) -> AccessPoint {
        AccessPoint {
            name: self.name.clone(),
            network_origin: self.network_origin.clone(),
            vpc_configuration: self.vpc_configuration.clone(),
            bucket: self.bucket.clone(),
            access_point_arn: Some(self.arn.clone()),
            alias: Some(self.alias.clone()),
            bucket_account_id: self.bucket_account_id.clone(),
            // S3 on Outposts / FSx data sources are not emulated, so a record
            // created here is attached to no data source.
            data_source_id: None,
            data_source_type: None,
        }
    }
}

/// How many access points the store holds before `CreateAccessPoint` refuses
/// with `TooManyAccessPoints`.
///
/// S3 Control publishes an adjustable quota on access points per account per
/// Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is set far above any workflow an emulator
/// serves, and it is what stops the state growing without end: every record it
/// holds is released by `DeleteAccessPoint`, which drops that access point's
/// policy with it, so together with [`MAX_POLICY_BYTES`] the bytes this store
/// can hold are bounded.
pub const ACCESS_POINT_QUOTA: usize = 1_000;

/// The largest policy document `PutAccessPointPolicy` accepts and retains, in
/// bytes.
///
/// Amazon S3 documents a 20 KB maximum for a bucket or access point policy, so a
/// document past it is one the real service would not store either. It is also
/// what bounds the bytes a single record can hold.
pub const MAX_POLICY_BYTES: usize = 20 * 1024;

/// How a stored access point is keyed: the owning account id and the access
/// point name, which is the pair a name is unique within.
pub type AccessPointKey = (String, String);

/// How many accounts' `PublicAccessBlock` configurations the store holds before
/// `PutPublicAccessBlock` refuses to record one for a FURTHER account.
///
/// This is the emulator's OWN bound on its store, not a claim about any limit
/// AWS publishes: every account has exactly one such configuration, and
/// `AccountId` is constrained only to `^\d{12}$`, which bounds the shape of a
/// key and not how many of them a caller can name. Without this bound a caller
/// could grow this process's state without end by varying `AccountId`. It is
/// checked only when a request would ADD an account's configuration, so every
/// account already in the store stays writable, and `DeletePublicAccessBlock`
/// releases a slot.
pub const PUBLIC_ACCESS_BLOCK_QUOTA: usize = 1_000;

/// One stored S3 Storage Lens configuration: the document the last
/// `PutStorageLensConfiguration` stored, and the tag set that configuration
/// carries.
#[derive(Debug, Clone)]
pub struct StorageLensConfigurationRecord {
    /// The configuration document as the last put left it — stored WHOLE, never
    /// merged with the document before it, with `StorageLensArn` set to the ARN
    /// this service assigns (that member is documented read-only).
    pub configuration: StorageLensConfiguration,
    /// The configuration's tag set.
    ///
    /// Held BESIDE the document rather than inside it, because the two are
    /// separate resources on the wire: `GetStorageLensConfiguration` reports no
    /// tags at all, and `GetStorageLensConfigurationTagging` /
    /// `PutStorageLensConfigurationTagging` are what report and replace them.
    pub tags: Vec<StorageLensTag>,
}

/// How a stored S3 Storage Lens configuration is keyed: the owning account id
/// and the configuration id, which is the pair an id is unique within.
pub type StorageLensConfigurationKey = (String, String);

/// How many S3 Storage Lens configurations the store holds before
/// `PutStorageLensConfiguration` refuses to add another.
///
/// S3 Storage Lens limits how many configurations an account may hold in a
/// Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is set far above any workflow an
/// emulator serves, and together with [`MAX_STORAGE_LENS_TAGS`] it is what
/// stops this part of the state growing without end. (The record a put adds is
/// released by `DeleteStorageLensConfiguration`, which is still a scaffold with
/// its own gap issue; until it is written the bound is the only limit.)
pub const STORAGE_LENS_CONFIGURATION_QUOTA: usize = 1_000;

/// The largest tag set an S3 Storage Lens configuration carries.
///
/// The model documents it on both members that carry a tag set
/// (`PutStorageLensConfigurationRequest$Tags` and
/// `PutStorageLensConfigurationTaggingRequest$Tags`): "You can set up to a
/// maximum of 50 tags."
pub const MAX_STORAGE_LENS_TAGS: usize = 50;

// ---------------------------------------------------------------------------
// S3 Batch Operations jobs
//
// `CreateJob` records a job here and every job operation reads the one record
// it stored: `DescribeJob` reports it, the job tagging operations read and
// replace its tag set, and `UpdateJobPriority` / `UpdateJobStatus` move its
// members (those four are still scaffolds with their own gap issues).
// ---------------------------------------------------------------------------

/// `JobStatus` of a job the service has accepted and not yet prepared — the
/// status a `CreateJob` that needs no confirmation leaves the job in.
pub const JOB_STATUS_NEW: &str = "New";

/// `JobStatus` of a job that has been prepared and is waiting for its tasks to
/// be run.
pub const JOB_STATUS_READY: &str = "Ready";

/// `JobStatus` of a job that is waiting for confirmation before it runs, which
/// is what `JobDescriptor$SuspendedCause` documents a created job enters when
/// confirmation is required: "When you create the job, it enters the Suspended
/// state to await confirmation before running."
pub const JOB_STATUS_SUSPENDED: &str = "Suspended";

/// The `SuspendedCause` of a job held in [`JOB_STATUS_SUSPENDED`] by its own
/// `ConfirmationRequired`, from the sentence above: it is awaiting
/// confirmation. `SuspendedCause` is a plain string in the model (len 1..1024),
/// so the model names no catalog of causes.
pub const SUSPENDED_AWAITING_CONFIRMATION: &str = "AwaitingConfirmation";

/// How long a created job takes to leave [`JOB_STATUS_NEW`].
///
/// A span chosen by this emulator (AWS documents no duration for it), inside
/// the 250–750 ms band this project fixes for a transition the clock owes. It
/// is never zero, so a caller that describes the job straight after creating it
/// still sees `New`.
pub const JOB_PREPARATION: Duration = Duration::from_millis(500);

/// How many jobs the store holds before `CreateJob` refuses another.
///
/// S3 Batch Operations publishes an adjustable quota on jobs, and this emulator
/// models neither Service Quotas nor an account's adjustments — so this is the
/// emulator's OWN bound on the store rather than a claim about any account's
/// limit. It is set far above any workflow an emulator serves, and it is what
/// stops the job store growing without end: every member of a job record is
/// length-bounded by the model, and a repeated `CreateJob` carrying one
/// `ClientRequestToken` adds no record at all.
pub const JOB_QUOTA: usize = 1_000;

/// How a stored job is keyed: the owning account id and the job id, which is
/// the pair a job id is unique within.
pub type JobKey = (String, String);

/// One stored S3 Batch Operations job: every member `DescribeJob` reports,
/// recorded by `CreateJob`.
#[derive(Debug, Clone)]
pub struct JobRecord {
    /// The service-assigned job id, which is also the second half of the key.
    pub job_id: String,
    /// The account that created the job — the first half of the key, kept on
    /// the record so a record passed on its own still names its owner.
    pub account_id: String,
    /// The job's ARN, assigned by this service (see [`job_arn`]).
    pub arn: String,
    /// The job's current status: one of [`JOB_STATUS_VALUES`].
    pub status: String,
    pub confirmation_required: Option<bool>,
    pub description: Option<String>,
    /// The manifest the create named, if it named one rather than a generator.
    pub manifest: Option<JobManifest>,
    /// The manifest generator the create named, if it named one rather than a
    /// manifest.
    pub manifest_generator: Option<JobManifestGenerator>,
    pub operation: JobOperation,
    pub priority: i32,
    pub report: JobReport,
    pub role_arn: String,
    pub creation_time: ::aws_smithy_types::DateTime,
    pub suspended_date: Option<::aws_smithy_types::DateTime>,
    pub suspended_cause: Option<String>,
    /// The reason carried by the last `UpdateJobStatus`; `None` until one
    /// arrives, which is what `DescribeJob` reports as an absent member.
    pub status_update_reason: Option<String>,
    /// When the job reached a terminal status; `None` while it has not.
    pub termination_date: Option<::aws_smithy_types::DateTime>,
    /// The job's tag set, as `CreateJob` was given it. The job tagging
    /// operations (`GetJobTagging`, `PutJobTagging`, `DeleteJobTagging`) are
    /// still scaffolds with their own gap issues, and will read this field
    /// rather than a second copy.
    pub tags: Vec<S3Tag>,
    /// The `ClientRequestToken` the create carried, which is what makes a
    /// repeated create idempotent rather than a second job.
    pub client_request_token: String,
    /// A digest of the create parameters BESIDE the token (see
    /// [`create_job_fingerprint`]), so a repeat of the same token can be told
    /// from a different request reusing it.
    pub request_fingerprint: String,
    /// When the transition out of [`JOB_STATUS_NEW`] falls due; `None` once it
    /// has been materialised (or when the job was never in `New`). Stored as an
    /// instant rather than slept for.
    pub status_due_at: Option<SystemTime>,
}

impl JobRecord {
    /// The record as the `JobDescriptor` shape `DescribeJob` returns.
    ///
    /// `ProgressSummary` and `FailureReasons` are absent: this emulator runs no
    /// batch tasks, so there is no task count or failure to report — and a
    /// synthesised one would be a claim about work nothing did.
    pub fn descriptor(&self) -> JobDescriptor {
        JobDescriptor {
            job_id: Some(self.job_id.clone()),
            confirmation_required: self.confirmation_required,
            description: self.description.clone(),
            job_arn: Some(self.arn.clone()),
            status: Some(self.status.clone()),
            manifest: self.manifest.clone(),
            operation: Some(self.operation.clone()),
            priority: self.priority,
            progress_summary: None,
            status_update_reason: self.status_update_reason.clone(),
            failure_reasons: None,
            report: Some(self.report.clone()),
            creation_time: Some(self.creation_time),
            termination_date: self.termination_date,
            role_arn: Some(self.role_arn.clone()),
            suspended_date: self.suspended_date,
            suspended_cause: self.suspended_cause.clone(),
            manifest_generator: self.manifest_generator.clone(),
            // A generated manifest is described only once a generator has
            // actually written one, and nothing in this emulator does.
            generated_manifest_descriptor: None,
        }
    }

    /// Apply the transition the clock already owed this job at `now`: a job
    /// leaves [`JOB_STATUS_NEW`] for [`JOB_STATUS_READY`] once its preparation
    /// span has passed.
    ///
    /// This materialises a change that was due at the same instant whether or
    /// not anybody called — it is not a move made because the job was read.
    fn materialize_due(&mut self, now: SystemTime) {
        if self.status != JOB_STATUS_NEW {
            return;
        }
        if self.status_due_at.is_some_and(|due| due <= now) {
            self.status = job_status(JOB_STATUS_READY);
            self.status_due_at = None;
        }
    }
}

/// The spelling of a job status that goes on the wire: the one the generated
/// [`JOB_STATUS_VALUES`] carries (from the model's `JobStatus` enum), found by
/// VALUE rather than by position, so neither a reordering of the generated list
/// nor a typo in a constant above can change what is returned.
pub fn job_status(wanted: &'static str) -> String {
    modelled_value(JOB_STATUS_VALUES, wanted)
}

// ---------------------------------------------------------------------------
// Multi-Region Access Points, and the asynchronous requests that manage them.
// ---------------------------------------------------------------------------

/// How long a Multi-Region Access Point stays in the transient status an
/// asynchronous request put it in before the clock owes it the settled outcome:
/// `CREATING` becomes `READY`, and `DELETING` releases the record.
///
/// `CreateMultiRegionAccessPoint` and `DeleteMultiRegionAccessPoint` are
/// documented as asynchronous ("you might receive a response before the command
/// has completed"), and neither the model nor the documentation publishes how
/// long the propagation takes — on the real service it is minutes. This span is
/// therefore the emulator's OWN: long enough that a caller which reads straight
/// after the call still observes the transient status, short enough that a
/// request settles within one test, and owed by the clock alone, so it completes
/// whether or not anybody calls again.
pub const MULTI_REGION_ACCESS_POINT_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The `MultiRegionAccessPointStatus` a create request leaves behind until
/// [`MULTI_REGION_ACCESS_POINT_SETTLE_AFTER`] has elapsed — "a temporary state
/// that exists while the request is propagating and being completed", per the
/// member's documentation.
pub const STATUS_CREATING: &str = "CREATING";

/// The `MultiRegionAccessPointStatus` a created Multi-Region Access Point
/// reaches once the clock owes it.
pub const STATUS_READY: &str = "READY";

/// The `MultiRegionAccessPointStatus` a delete request leaves behind until
/// [`MULTI_REGION_ACCESS_POINT_SETTLE_AFTER`] has elapsed, after which the
/// record is released.
pub const STATUS_DELETING: &str = "DELETING";

/// The `AsyncRequestStatus` an asynchronous request carries until the clock owes
/// its work.
///
/// `AsyncRequestStatus` is an unconstrained string in the model, so these two
/// values are this service's vocabulary for the two states a Multi-Region Access
/// Point request can be in rather than a modelled enumeration.
pub const ASYNC_REQUEST_IN_PROGRESS: &str = "INPROGRESS";

/// The `AsyncRequestStatus` an asynchronous request carries once its work is
/// due. See [`ASYNC_REQUEST_IN_PROGRESS`].
pub const ASYNC_REQUEST_SUCCEEDED: &str = "SUCCEEDED";

/// `wanted` as the generated enum list `values` spells it.
///
/// The spelling that goes on the wire is the model's, found by VALUE rather than
/// by position: neither a reordering of the generated list nor a typo in the
/// constant above can change what is returned.
fn modelled_value(values: &[&'static str], wanted: &'static str) -> String {
    values
        .iter()
        .copied()
        .find(|value| *value == wanted)
        .unwrap_or(wanted)
        .to_string()
}

/// The ARN of a job: the `job/<job-id>` resource in this service's ARN
/// namespace (`s3`, per the model's `awsService.arnNamespace`), which is the
/// form `JobDescriptor$JobArn` states
/// (`^arn:[^:]+:s3:[a-zA-Z0-9\-]+:\d{12}:job\/`).
///
/// The account segment comes from [`::emulator_server::emulator_arn`], the
/// single source of truth for the canonical emulator account.
pub fn job_arn(region: &str, job_id: &str) -> String {
    ::emulator_server::emulator_arn("s3", region, &format!("job/{job_id}"))
}

/// A digest of everything a `CreateJob` asked for EXCEPT its
/// `ClientRequestToken` and the account it names, which is what lets a repeat
/// of one token be answered with the job it already created while a DIFFERENT
/// request reusing that token is refused.
///
/// The members are rendered with `Debug` — the one total, structural rendering
/// every modelled shape in this service derives — and hashed with SHA-256, so
/// two creates agree here exactly when they asked for the same job. The input
/// is destructured rather than field-accessed, so a member the model adds to
/// `CreateJobRequest` is a compile error here rather than a difference two
/// creates could disagree on unnoticed.
pub fn create_job_fingerprint(input: &CreateJobInput) -> String {
    use sha2::Digest;

    let CreateJobInput {
        // Not part of the fingerprint: the token is what this digest is
        // compared ACROSS, and the account id is already the scope the token is
        // looked up in.
        account_id: _,
        client_request_token: _,
        confirmation_required,
        operation,
        report,
        manifest,
        description,
        priority,
        role_arn,
        tags,
        manifest_generator,
    } = input;
    let rendered = format!(
        "{operation:?}\u{1f}{report:?}\u{1f}{manifest:?}\u{1f}{manifest_generator:?}\u{1f}\
         {description:?}\u{1f}{priority:?}\u{1f}{role_arn:?}\u{1f}{confirmation_required:?}\u{1f}\
         {tags:?}"
    );
    hex::encode(sha2::Sha256::digest(rendered.as_bytes()))
}

/// A fresh job id, in the shape S3 Batch Operations gives one: a version-4
/// UUID, `8-4-4-4-12` lowercase hex with the version and variant bits RFC 4122
/// §4.4 fixes — 36 characters, which is the maximum the modelled `JobId` (len
/// 5..36, pattern `^[a-zA-Z0-9\-\_]+$`) admits.
///
/// The 128 bits come from three sources hashed together with SHA-256: the
/// OS-seeded randomness a `RandomState` draws, the system clock in nanoseconds,
/// and `sequence` — a counter the store hands out, so two creates in the same
/// nanosecond cannot collide. A job id names a resource; a credential would
/// come from the credential store instead, never from here.
fn new_job_uuid(sequence: u64) -> String {
    use sha2::Digest;
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = sha2::Sha256::new();
    hasher.update(RandomState::new().hash_one(sequence).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(sequence.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version nibble is 4, and the variant's two top bits
    // are `10`.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let rendered = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[0..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..32]
    )
}

/// One of [`STATUS_CREATING`], [`STATUS_READY`] or [`STATUS_DELETING`], spelled
/// as the model's `MultiRegionAccessPointStatus` enum carries it.
pub fn multi_region_access_point_status(wanted: &'static str) -> String {
    modelled_value(MULTI_REGION_ACCESS_POINT_STATUS_VALUES, wanted)
}

/// An `AsyncOperationName`, spelled as the model's enum carries it.
pub fn async_operation_name(wanted: &'static str) -> String {
    modelled_value(ASYNC_OPERATION_NAME_VALUES, wanted)
}

/// One stored Multi-Region Access Point: everything `GetMultiRegionAccessPoint`
/// reports, recorded by `CreateMultiRegionAccessPoint`.
#[derive(Debug, Clone)]
pub struct MultiRegionAccessPointRecord {
    pub name: String,
    /// The account that owns it — the first half of the key, kept on the record
    /// so a record passed on its own still names its owner.
    pub account_id: String,
    pub alias: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub public_access_block: Option<PublicAccessBlockConfiguration>,
    /// The Regions and buckets the create request associated, in the order it
    /// listed them.
    pub regions: Vec<RegionReport>,
    /// One of [`MULTI_REGION_ACCESS_POINT_STATUS_VALUES`].
    pub status: String,
    /// When the clock owes `status` its settled outcome, for the transient
    /// statuses an asynchronous request leaves behind; `None` once nothing is
    /// owed.
    pub status_due_at: Option<SystemTime>,
}

impl MultiRegionAccessPointRecord {
    /// The record as the `MultiRegionAccessPointReport` shape
    /// `GetMultiRegionAccessPoint` returns.
    pub fn report(&self) -> MultiRegionAccessPointReport {
        MultiRegionAccessPointReport {
            name: Some(self.name.clone()),
            alias: Some(self.alias.clone()),
            created_at: Some(self.created_at),
            public_access_block: self.public_access_block.clone(),
            status: Some(self.status.clone()),
            regions: Some(self.regions.clone()),
        }
    }

    /// Whether the clock has already released this record: a delete request's
    /// span has elapsed, so the Multi-Region Access Point is gone whether or not
    /// anybody called.
    fn removal_due(&self, now: SystemTime) -> bool {
        self.status == STATUS_DELETING && self.status_due_at.is_some_and(|due| now >= due)
    }

    /// Move `status` on if the clock already owes it: a creating access point
    /// reaches `READY`. A record whose removal is due is dropped by
    /// [`ComAmazonawsS3controlAwss3controlservicev20180820State::settle_multi_region_access_points`]
    /// rather than here.
    fn settle(&mut self, now: SystemTime) {
        if self.status == STATUS_CREATING && self.status_due_at.is_some_and(|due| now >= due) {
            self.status = multi_region_access_point_status(STATUS_READY);
            self.status_due_at = None;
        }
    }
}

/// One asynchronous Multi-Region Access Point request, as
/// `DescribeMultiRegionAccessPointOperation` reports it.
#[derive(Debug, Clone)]
pub struct AsyncRequestRecord {
    /// The token the create or delete call handed its caller — this record's
    /// identity, and the second half of its key.
    pub request_token_arn: String,
    /// The account the request was made for.
    pub account_id: String,
    /// One of [`ASYNC_OPERATION_NAME_VALUES`].
    pub operation: String,
    pub creation_time: ::aws_smithy_types::DateTime,
    /// The parameters the call submitted, which the model reports back verbatim.
    pub request_parameters: AsyncRequestParameters,
    /// The Regions the request touches, in request order; each is reported with
    /// its own status.
    pub regions: Vec<String>,
    /// When the clock owes this request's work — the same instant the Multi-Region
    /// Access Point's transient status settles at.
    pub due_at: SystemTime,
}

impl AsyncRequestRecord {
    /// The record as the `AsyncOperation` shape, with the status the clock owes
    /// at `now`.
    pub fn operation_report(&self, now: SystemTime) -> AsyncOperation {
        let status = if now >= self.due_at {
            ASYNC_REQUEST_SUCCEEDED
        } else {
            ASYNC_REQUEST_IN_PROGRESS
        };
        AsyncOperation {
            creation_time: Some(self.creation_time),
            operation: Some(self.operation.clone()),
            request_token_arn: Some(self.request_token_arn.clone()),
            request_parameters: Some(self.request_parameters.clone()),
            request_status: Some(status.to_string()),
            response_details: Some(AsyncResponseDetails {
                multi_region_access_point_details: Some(MultiRegionAccessPointsAsyncResponse {
                    regions: Some(
                        self.regions
                            .iter()
                            .map(|region| MultiRegionAccessPointRegionalResponse {
                                name: Some(region.clone()),
                                request_status: Some(status.to_string()),
                            })
                            .collect(),
                    ),
                }),
                // Only a request that failed carries error details, and a
                // request this service accepted is one it completes: the
                // refusals this lifecycle has are answered to the create or
                // delete call itself, not recorded as an async failure.
                error_details: None,
            }),
        }
    }

    /// The pair this record is ordered by when the store evicts its oldest
    /// retained request: when it was made, to the nanosecond.
    fn made_at(&self) -> (i64, u32) {
        (self.creation_time.secs(), self.creation_time.subsec_nanos())
    }
}

/// How a stored Multi-Region Access Point is keyed: the owning account id and
/// the name, which is the pair a name is unique within.
pub type MultiRegionAccessPointKey = (String, String);

/// How a stored asynchronous request is keyed: the account it was made for and
/// its request token ARN.
pub type AsyncRequestKey = (String, String);

/// How many Multi-Region Access Points the store holds before
/// `CreateMultiRegionAccessPoint` refuses to add another.
///
/// S3 publishes an adjustable quota on Multi-Region Access Points per account,
/// and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is set far above any workflow an emulator
/// serves, and it is what stops this part of the state growing without end:
/// every record it holds is released by `DeleteMultiRegionAccessPoint`.
pub const MULTI_REGION_ACCESS_POINT_QUOTA: usize = 1_000;

/// How many asynchronous request records the store retains.
///
/// Unlike a Multi-Region Access Point, an asynchronous request is never deleted
/// by a caller, so retention is what bounds it: recording the
/// [`ASYNC_REQUESTS_RETAINED`]-th + 1 request releases the oldest one, exactly
/// as the real service forgets a request long after it completed. A caller that
/// then describes a forgotten token is answered the same not-found refusal as
/// one that invents a token.
pub const ASYNC_REQUESTS_RETAINED: usize = 1_000;

/// Per-service state for com-amazonaws-s3control-awss3controlservicev20180820. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsS3controlAwss3controlservicev20180820State {
    /// Access points by (account id, name).
    ///
    /// A `BTreeMap` rather than a `HashMap` so the one scan order every reader
    /// sees is sorted by that key: `ListAccessPoints` is `@paginated`, and a
    /// page token can only resume a scan whose order does not change between
    /// calls.
    ///
    /// BOUNDED: `CreateAccessPoint` refuses past [`ACCESS_POINT_QUOTA`] records
    /// and `PutAccessPointPolicy` refuses a document past
    /// [`MAX_POLICY_BYTES`], so neither the number of records nor the size of
    /// one is caller-chosen without limit. `DeleteAccessPoint` releases a
    /// record and its policy.
    pub access_points: BTreeMap<AccessPointKey, AccessPointRecord>,

    /// The account-level `PublicAccessBlock` configuration each account has set,
    /// by account id — what `PutPublicAccessBlock` records,
    /// `GetPublicAccessBlock` reports and `DeletePublicAccessBlock` removes.
    ///
    /// An account is a KEY here only once a configuration has been put for it:
    /// an ABSENT entry is what `GetPublicAccessBlock` answers
    /// `NoSuchPublicAccessBlockConfiguration` for, and it is a different state
    /// from a PRESENT configuration whose four settings are all `false`, which
    /// that read reports as a configuration.
    ///
    /// BOUNDED: one fixed-size record per account and at most
    /// [`PUBLIC_ACCESS_BLOCK_QUOTA`] accounts, released by
    /// `DeletePublicAccessBlock`.
    pub public_access_blocks: BTreeMap<String, PublicAccessBlockConfiguration>,

    /// S3 Storage Lens configurations by (account id, configuration id).
    ///
    /// A `BTreeMap` for the same reason as `access_points`:
    /// `ListStorageLensConfigurations` is `@paginated`, and a page token can
    /// only resume a scan whose order does not change between calls.
    ///
    /// BOUNDED: `PutStorageLensConfiguration` refuses a NEW configuration past
    /// [`STORAGE_LENS_CONFIGURATION_QUOTA`] records, and both puts that carry a
    /// tag set refuse one past [`MAX_STORAGE_LENS_TAGS`] tags; every member of
    /// the document itself is length-bounded by the model.
    pub storage_lens_configurations:
        BTreeMap<StorageLensConfigurationKey, StorageLensConfigurationRecord>,

    /// S3 Batch Operations jobs by (account id, job id).
    ///
    /// A `BTreeMap` for the same reason as `access_points`: `ListJobs` is
    /// `@paginated`, and a page token can only resume a scan whose order does
    /// not change between calls.
    ///
    /// BOUNDED: `CreateJob` refuses past [`JOB_QUOTA`] records, and a repeated
    /// create carrying one `ClientRequestToken` adds none; every member of a
    /// record is length-bounded by the model.
    pub jobs: BTreeMap<JobKey, JobRecord>,

    /// How many job ids this store has minted, so two ids are distinct even
    /// when the clock reports the same nanosecond (see [`new_job_uuid`]).
    minted_job_ids: u64,

    /// Multi-Region Access Points by (account id, name).
    ///
    /// A `BTreeMap` for the same reason as `access_points`:
    /// `ListMultiRegionAccessPoints` is `@paginated`, and a page token can only
    /// resume a scan whose order does not change between calls.
    ///
    /// BOUNDED: `CreateMultiRegionAccessPoint` refuses past
    /// [`MULTI_REGION_ACCESS_POINT_QUOTA`] records, and every member of a record
    /// is length-bounded by the model. `DeleteMultiRegionAccessPoint` releases a
    /// record once the clock owes its removal.
    pub multi_region_access_points:
        BTreeMap<MultiRegionAccessPointKey, MultiRegionAccessPointRecord>,

    /// The asynchronous Multi-Region Access Point requests
    /// `DescribeMultiRegionAccessPointOperation` reports, by (account id,
    /// request token ARN).
    ///
    /// BOUNDED by retention rather than by a caller's delete: see
    /// [`ASYNC_REQUESTS_RETAINED`] and [`Self::record_async_request`].
    pub async_requests: BTreeMap<AsyncRequestKey, AsyncRequestRecord>,
}

impl ComAmazonawsS3controlAwss3controlservicev20180820State {
    /// A fresh job id for a job this store is about to hold, over this store's
    /// own counter (see [`new_job_uuid`]).
    pub fn new_job_id(&mut self) -> String {
        self.minted_job_ids = self.minted_job_ids.wrapping_add(1);
        new_job_uuid(self.minted_job_ids)
    }

    /// Apply every job transition the clock already owed at `now`.
    ///
    /// Called by the operations that report or move a job, so what they answer
    /// is what the clock had already made due — never a move caused by the read
    /// itself. The scan is over the whole job store, which [`JOB_QUOTA`]
    /// bounds.
    pub fn materialize_due_jobs(&mut self, now: SystemTime) {
        for job in self.jobs.values_mut() {
            job.materialize_due(now);
        }
    }

    /// The job `job_id` names under `account_id`, or `None` when that account
    /// owns no such job.
    pub fn job(&self, account_id: &str, job_id: &str) -> Option<&JobRecord> {
        self.jobs
            .get(&(account_id.to_string(), job_id.to_string()))
    }

    /// The job this account already created with `client_request_token`, if
    /// any: the record a repeated create is answered from.
    pub fn job_by_client_request_token(
        &self,
        account_id: &str,
        client_request_token: &str,
    ) -> Option<&JobRecord> {
        self.jobs.values().find(|job| {
            job.account_id == account_id && job.client_request_token == client_request_token
        })
    }

    /// Materialise every Multi-Region Access Point transition the clock already
    /// owes at `now`: a creating access point reaches `READY`, and a deleting
    /// one is released.
    ///
    /// Each of these was owed at `now` whether or not anybody called, so a
    /// reporting operation runs it before reading: that is what makes a status a
    /// caller observes the status the clock says it is, rather than the one the
    /// last writer left behind.
    pub fn settle_multi_region_access_points(&mut self, now: SystemTime) {
        self.multi_region_access_points
            .retain(|_, record| !record.removal_due(now));
        for record in self.multi_region_access_points.values_mut() {
            record.settle(now);
        }
    }

    /// Retain `record`, releasing the oldest retained request first when the
    /// store is already holding [`ASYNC_REQUESTS_RETAINED`] of them, so the
    /// number of async request records a caller can accumulate is bounded.
    pub fn record_async_request(&mut self, record: AsyncRequestRecord) {
        while self.async_requests.len() >= ASYNC_REQUESTS_RETAINED {
            let oldest = self
                .async_requests
                .iter()
                .min_by_key(|(key, retained)| (retained.made_at(), (*key).clone()))
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            self.async_requests.remove(&oldest);
        }
        let key = (record.account_id.clone(), record.request_token_arn.clone());
        self.async_requests.insert(key, record);
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsS3controlAwss3controlservicev20180820State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsS3controlAwss3controlservicev20180820State>)
}

/// The region this request was signed for, which is the region the resources it
/// touches live in. Falls back to the AWS SDK default region when the request
/// carries no SigV4 credential scope (authentication may be off).
pub fn signing_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request is answered in — the region the emulator's own
/// generated credentials file names (`vendor/server/src/lib.rs`).
const DEFAULT_REGION: &str = "us-east-1";

/// The ARN of an access point: the `accesspoint/<name>` resource in this
/// service's ARN namespace (`s3`, per the model's `awsService.arnNamespace`).
///
/// The account segment comes from [`::emulator_server::emulator_arn`], the single
/// source of truth for the canonical emulator account — a generated emulator
/// never puts another account id in an ARN it emits.
pub fn access_point_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn("s3", region, &format!("accesspoint/{name}"))
}

/// The ARN of an S3 Storage Lens configuration.
///
/// `StorageLensConfiguration$StorageLensArn` documents the member as read-only
/// and states the format it follows:
/// `arn:aws:s3:<region>:<account-id>:storage-lens/<config-id>` — so the service
/// assigns it from the configuration's own identity rather than taking whatever
/// a request submitted. The account segment comes from
/// [`::emulator_server::emulator_arn`], the single source of truth for the
/// canonical emulator account.
pub fn storage_lens_configuration_arn(region: &str, config_id: &str) -> String {
    ::emulator_server::emulator_arn("s3", region, &format!("storage-lens/{config_id}"))
}

/// The suffix length AWS access point aliases carry between the name and the
/// `-s3alias` tail: a 26-character, service-assigned string.
const ALIAS_SUFFIX_LEN: usize = 26;

/// The tail every S3 access point alias ends in.
const ALIAS_TAIL: &str = "-s3alias";

/// `Alias`'s modelled maximum length (`com.amazonaws.s3control#Alias`, len 0..63).
const ALIAS_MAX_LEN: usize = 63;

/// The alias of an access point: `<name>-<26 service-assigned characters>-s3alias`,
/// the form S3 reports for an access point alias.
///
/// The middle segment is derived from the access point's identity (its owning
/// account and name) rather than drawn at random, so it is one value per access
/// point: stable across every later read, and different for a different access
/// point. The name segment is truncated when the whole alias would pass the
/// modelled 63-character maximum.
pub fn access_point_alias(account_id: &str, name: &str) -> String {
    use sha2::Digest;

    let digest = sha2::Sha256::digest(format!("{account_id}/{name}").as_bytes());
    let suffix: String = hex::encode(digest).chars().take(ALIAS_SUFFIX_LEN).collect();

    let room_for_name = ALIAS_MAX_LEN - ALIAS_TAIL.len() - ALIAS_SUFFIX_LEN - 1;
    let lowered = name.to_lowercase();
    let head: String = lowered.chars().take(room_for_name).collect();
    format!("{head}-{suffix}{ALIAS_TAIL}")
}

/// The network origin of an access point: `VPC` when the create call restricted
/// it to a VPC, otherwise `Internet` — the rule the `AccessPoint$NetworkOrigin`
/// documentation states.
pub fn network_origin(vpc_configuration: Option<&VpcConfiguration>) -> String {
    let vpc_restricted = vpc_configuration.is_some_and(|vpc| !vpc.vpc_id.is_empty());
    let wanted = if vpc_restricted { "VPC" } else { "Internet" };
    // The spelling that goes on the wire is the one the generated
    // NETWORK_ORIGIN_VALUES carries (from the model's `NetworkOrigin` enum),
    // found by VALUE rather than by position: neither a reordering of the
    // generated list nor a typo in the name above can change what is returned.
    NETWORK_ORIGIN_VALUES
        .iter()
        .copied()
        .find(|value| *value == wanted)
        .unwrap_or(wanted)
        .to_string()
}

/// Encode a `ListAccessPoints` page token: the opaque form of the last access
/// point name the previous page returned.
///
/// Encoded rather than passed through so a caller does not come to depend on a
/// token's contents, the way an AWS page token's contents are not part of its
/// contract. The encoding is reversible and is NOT a secret or an access check:
/// a token only names a position in the account's own scan.
pub fn encode_page_token(last_name: &str) -> String {
    hex::encode(last_name.as_bytes())
}

/// Decode a page token produced by [`encode_page_token`], or `None` when it was
/// not produced by it.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

// ---------------------------------------------------------------------------
// The derivations the Multi-Region Access Point lifecycle shares.
// ---------------------------------------------------------------------------

/// The tail every Multi-Region Access Point alias ends in, per the modelled
/// `MultiRegionAccessPointAlias` pattern `^[a-z][a-z0-9]*[.]mrap$`.
const MRAP_ALIAS_TAIL: &str = ".mrap";

/// How many characters of a Multi-Region Access Point alias precede
/// [`MRAP_ALIAS_TAIL`]. Well inside the modelled 63-character maximum, so an
/// alias never has to be truncated.
const MRAP_ALIAS_BODY_LEN: usize = 16;

/// The characters an alias body is drawn from: the first from the letters alone,
/// because the modelled pattern requires an alias to START with `[a-z]`, the
/// rest from letters and digits.
const MRAP_ALIAS_LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const MRAP_ALIAS_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The alias of a Multi-Region Access Point: a service-assigned string of the
/// form the modelled `MultiRegionAccessPointAlias` pattern admits, ending in
/// `.mrap`.
///
/// Unlike an access point's alias, a Multi-Region Access Point's carries no part
/// of the name — the model documents the two as distinct, and the pattern admits
/// no hyphen. It is derived from the access point's identity (its owning account
/// and name) rather than drawn at random, so it is one value per access point:
/// stable across every later read, and different for a different access point.
pub fn multi_region_access_point_alias(account_id: &str, name: &str) -> String {
    use sha2::Digest;

    let digest = sha2::Sha256::digest(format!("mrap/{account_id}/{name}").as_bytes());
    let mut alias = String::with_capacity(MRAP_ALIAS_BODY_LEN + MRAP_ALIAS_TAIL.len());
    for (position, byte) in digest.iter().take(MRAP_ALIAS_BODY_LEN).enumerate() {
        let alphabet = if position == 0 {
            MRAP_ALIAS_LETTERS
        } else {
            MRAP_ALIAS_ALPHABET
        };
        alias.push(alphabet[usize::from(*byte) % alphabet.len()] as char);
    }
    alias.push_str(MRAP_ALIAS_TAIL);
    alias
}

/// The `async-request/mrap/create` path segment, and the `create` half of a
/// request token ARN.
pub const ASYNC_VERB_CREATE: &str = "create";

/// The `delete` half of a request token ARN. See [`ASYNC_VERB_CREATE`].
pub const ASYNC_VERB_DELETE: &str = "delete";

/// How many hex characters of the digest a request token ARN carries.
const ASYNC_REQUEST_ID_LEN: usize = 32;

/// The ARN of an asynchronous Multi-Region Access Point request: the
/// `async-request/mrap/<verb>/<id>` resource in this service's ARN namespace
/// (`s3`, per the model's `awsService.arnNamespace`), which satisfies the
/// modelled `AsyncRequestTokenARN` pattern `^arn:.+$`.
///
/// The id is a digest of the request's own identity — the verb, the account, the
/// caller's `ClientToken` and the details it submitted — so one request has ONE
/// token: a byte-identical retry is handed the token the first call returned,
/// which is what an `@idempotencyToken` asks for ("guarantee that requests are
/// unique"), and two different requests never share a token. The account segment
/// comes from [`::emulator_server::emulator_arn`], the single source of truth
/// for the canonical emulator account.
pub fn async_request_token_arn(
    region: &str,
    verb: &str,
    account_id: &str,
    client_token: &str,
    details: &str,
) -> String {
    use sha2::Digest;

    let digest =
        sha2::Sha256::digest(format!("{verb}\n{account_id}\n{client_token}\n{details}").as_bytes());
    let id: String = hex::encode(digest)
        .chars()
        .take(ASYNC_REQUEST_ID_LEN)
        .collect();
    ::emulator_server::emulator_arn("s3", region, &format!("async-request/mrap/{verb}/{id}"))
}

/// A `CreateMultiRegionAccessPoint` request's details as one string, for
/// [`async_request_token_arn`]: every member the caller submitted, so two
/// requests that differ in any of them differ in their token.
pub fn create_details_fingerprint(details: &CreateMultiRegionAccessPointInput2) -> String {
    let mut fingerprint = format!("name={}", details.name);
    for region in &details.regions {
        fingerprint.push_str(&format!(
            "|bucket={}@{}",
            region.bucket,
            region.bucket_account_id.as_deref().unwrap_or_default()
        ));
    }
    if let Some(block) = &details.public_access_block {
        fingerprint.push_str(&format!(
            "|block={},{},{},{}",
            block.block_public_acls,
            block.ignore_public_acls,
            block.block_public_policy,
            block.restrict_public_buckets
        ));
    }
    fingerprint
}

/// A `DeleteMultiRegionAccessPoint` request's details as one string, for
/// [`async_request_token_arn`].
pub fn delete_details_fingerprint(details: &DeleteMultiRegionAccessPointInput2) -> String {
    format!("name={}", details.name)
}

// ---------------------------------------------------------------------------
// The refusals the access point lifecycle shares.
//
// S3 Control's model declares no errors for these operations, so each is built
// through `Scaffolded::service_failure`, which carries the wire `<Code>` and the
// HTTP status a client branches on. They live here, once, so four handlers
// cannot drift on a code.
// ---------------------------------------------------------------------------

/// `NoSuchAccessPoint` (HTTP 404): no access point of that name is owned by the
/// account the request names.
pub fn no_such_access_point<E>(name: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "NoSuchAccessPoint",
        404,
        format!("The specified access point does not exist: {name}"),
    )
}

/// `NoSuchAccessPointPolicy` (HTTP 404): the access point exists but carries no
/// policy, because none was ever put on it (or it was deleted).
pub fn no_such_access_point_policy<E>(name: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "NoSuchAccessPointPolicy",
        404,
        format!("The specified access point does not have a policy: {name}"),
    )
}

/// `AccessPointAlreadyOwnedByYou` (HTTP 409): the account already owns an access
/// point of that name, so creating it again would be a second resource under one
/// name.
pub fn access_point_already_owned_by_you<E>(name: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessPointAlreadyOwnedByYou",
        409,
        format!("An access point with the same name already exists: {name}"),
    )
}

/// `TooManyAccessPoints` (HTTP 400): the store already holds
/// [`ACCESS_POINT_QUOTA`] access points, so another record would grow it without
/// end. Released by `DeleteAccessPoint`.
pub fn too_many_access_points<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "TooManyAccessPoints",
        400,
        format!("This emulator holds the maximum of {ACCESS_POINT_QUOTA} access points."),
    )
}

/// `MalformedPolicy` (HTTP 400): the policy document is not one this service
/// will store — it is not JSON, or it is larger than [`MAX_POLICY_BYTES`].
pub fn malformed_policy<E>(why: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "MalformedPolicy",
        400,
        format!("The specified policy is not a policy this service can store: {why}"),
    )
}

/// `AccessDenied` (HTTP 403): the store already holds
/// [`PUBLIC_ACCESS_BLOCK_QUOTA`] accounts' configurations, so recording one for
/// a further account would grow this process's state without end.
/// `PutPublicAccessBlock` documents `AccessDenied` (403) as the refusal a
/// caller gets when the service will not let the account-level configuration be
/// written, and the message says which bound was reached.
pub fn public_access_block_store_full<E>(account_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessDenied",
        403,
        format!(
            "This emulator already holds the maximum of {PUBLIC_ACCESS_BLOCK_QUOTA} \
             PublicAccessBlock configurations and cannot add one for account {account_id}."
        ),
    )
}

/// `NoSuchConfiguration` (HTTP 404): no S3 Storage Lens configuration of that
/// id is owned by the account the request names.
///
/// S3 Control declares no error for the Storage Lens operations, so — exactly
/// as for the access point refusals above — the refusal is carried in the
/// S3-style XML `<Code>`/`<Message>` envelope with the code and status the real
/// service answers a missing configuration with.
pub fn no_such_storage_lens_configuration<E>(config_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "NoSuchConfiguration",
        404,
        format!("The specified configuration does not exist: {config_id}"),
    )
}

/// `InvalidRequestException` (HTTP 400): a `PutStorageLensConfiguration` whose
/// URI names one configuration id and whose document names another.
///
/// Both members are documented as the id OF the S3 Storage Lens configuration
/// (`PutStorageLensConfigurationRequest$ConfigId`, `StorageLensConfiguration$Id`),
/// so a request carrying two different ids names no one configuration. The code
/// and status are this service's own modelled `InvalidRequestException`
/// (`com.amazonaws.s3control#InvalidRequestException`, a 400 client error),
/// which this operation's model does not list.
pub fn storage_lens_configuration_id_mismatch<E>(
    config_id: &str,
    document_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "InvalidRequestException",
        400,
        format!(
            "ConfigId is {config_id} but the submitted StorageLensConfiguration's Id is \
             {document_id}; they must name the same configuration."
        ),
    )
}

/// `TooManyTagsException` (HTTP 400): the request carries more than
/// [`MAX_STORAGE_LENS_TAGS`] tags, the maximum the model documents for a
/// Storage Lens configuration's tag set.
///
/// The code and status are this service's own modelled `TooManyTagsException`
/// ("Amazon S3 throws this exception if you have too many tags in your tag
/// set", a 400 client error), which these operations' models do not list.
pub fn too_many_storage_lens_tags<E>(submitted: usize) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "TooManyTagsException",
        400,
        format!(
            "A Storage Lens configuration carries at most {MAX_STORAGE_LENS_TAGS} tags; \
             the request carries {submitted}."
        ),
    )
}

/// `TooManyConfigurations` (HTTP 400): the store already holds
/// [`STORAGE_LENS_CONFIGURATION_QUOTA`] Storage Lens configurations, so another
/// record would grow it without end. Replacing an existing configuration is
/// never refused by this bound: it adds no record.
pub fn too_many_storage_lens_configurations<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "TooManyConfigurations",
        400,
        format!(
            "This emulator holds the maximum of {STORAGE_LENS_CONFIGURATION_QUOTA} \
             Storage Lens configurations."
        ),
    )
}

/// `InvalidRequest` (HTTP 400): the account the request names owns no job of
/// that id.
///
/// The model binds `NotFoundException` to `DescribeJob`, but the recorded
/// behaviour of this service for a well-formed id that names no job is the
/// S3-style XML envelope with code `InvalidRequest` and HTTP 400 (example
/// message "Job not found"), so that is what the job-id surface answers —
/// uniformly, so `DescribeJob` and the job tagging operations cannot drift on
/// it.
pub fn job_not_found<E>(job_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "InvalidRequest",
        400,
        format!("Job not found: {job_id}"),
    )
}

/// `NoSuchMultiRegionAccessPoint` (HTTP 404): no Multi-Region Access Point of
/// that name is owned by the account the request names.
///
/// S3 Control declares no error for these operations, so — exactly as for the
/// access point refusals above — the refusal is carried in the S3-style XML
/// `<Code>`/`<Message>` envelope, with the `NoSuch<Resource>` code and the 404
/// this service answers a missing named resource with.
pub fn no_such_multi_region_access_point<E>(name: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "NoSuchMultiRegionAccessPoint",
        404,
        format!("The specified Multi-Region Access Point does not exist: {name}"),
    )
}

/// `MultiRegionAccessPointAlreadyOwnedByYou` (HTTP 409): the account already
/// owns a Multi-Region Access Point of that name — including one still being
/// created or deleted — so creating it again would be a second resource under
/// one name.
///
/// The code follows the one this service already answers for the same conflict
/// one resource down (`AccessPointAlreadyOwnedByYou`), and the 409 is the status
/// a name conflict carries.
pub fn multi_region_access_point_already_owned_by_you<E>(
    name: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "MultiRegionAccessPointAlreadyOwnedByYou",
        409,
        format!("A Multi-Region Access Point with the same name already exists: {name}"),
    )
}

/// `TooManyMultiRegionAccessPoints` (HTTP 400): the store already holds
/// [`MULTI_REGION_ACCESS_POINT_QUOTA`] Multi-Region Access Points, so another
/// record would grow it without end. Released by
/// `DeleteMultiRegionAccessPoint`.
pub fn too_many_multi_region_access_points<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "TooManyMultiRegionAccessPoints",
        400,
        format!(
            "This emulator holds the maximum of {MULTI_REGION_ACCESS_POINT_QUOTA} \
             Multi-Region Access Points."
        ),
    )
}

/// `NotFoundException` (HTTP 404): no asynchronous request with that token was
/// made by the account the request names, or it was made so long ago that the
/// store no longer retains it (see [`ASYNC_REQUESTS_RETAINED`]).
///
/// An asynchronous request is not an S3 resource with a `NoSuch<Resource>` code
/// of its own, so the code and status here are this service's own modelled
/// `NotFoundException` ("The requested resource could not be found", a 404),
/// which this operation's model does not list.
pub fn no_such_async_request<E>(request_token_arn: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "NotFoundException",
        404,
        format!("The specified request token names no asynchronous request: {request_token_arn}"),
    )
}

/// `InternalError` (HTTP 500): [`service_state`] answered `None`, so this
/// process has no shared store to read. Never answered with a fresh engine — a
/// private store would accept writes nobody else can see.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "InternalError",
        500,
        "The emulator's shared state is not reachable from this request.",
    )
}
