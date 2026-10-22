// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesisanalytics-kinesisanalytics-20150814 service.
//!
//! One record per Kinesis Analytics (v1, SQL) application, keyed by its
//! `ApplicationName`. Every implemented operation computes its answer from these
//! records — nothing is canned and nothing is echoed back without being stored.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    ApplicationSummary, DestinationSchema, InputProcessingConfiguration, KinesisFirehoseInput,
    KinesisFirehoseOutput, KinesisStreamsInput, KinesisStreamsOutput, LambdaOutput,
    S3ReferenceDataSource, SourceSchema,
};

/// How long a status transition the clock owes stays transient.
///
/// `StartApplication` and `StopApplication` answer before the application has
/// reached its terminal status, so a caller that reads straight back observes the
/// `STARTING` / `STOPPING` the real service reports. The span is never zero, and
/// nothing sleeps for it: the due instant is stored on the record and
/// [`Application::settle`] materialises the move once the clock has passed it.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// `InputParallelism.Count` for an input whose request omits it.
///
/// It happens to coincide with [`MIN_INPUT_PARALLELISM_COUNT`] today: one
/// in-application stream is both the documented default and the least the modelled
/// range allows. They are separate constants so a change to the default is not
/// silently applied to the bound, or the other way round.
pub const DEFAULT_INPUT_PARALLELISM_COUNT: i32 = 1;

/// The fewest in-application streams one input may feed: the lower bound of
/// `InputParallelismCount`'s modelled `1..64` range.
pub const MIN_INPUT_PARALLELISM_COUNT: i32 = 1;

/// The most in-application streams one input may feed: the upper bound of
/// `InputParallelismCount`'s modelled `1..64` range.
pub const MAX_INPUT_PARALLELISM_COUNT: i32 = 64;

/// Status of an application: the closed set the model declares
/// (`smithy_query shape=ApplicationStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicationStatus {
    /// Set by `DeleteApplication` once it models the asynchronous delete; the
    /// implemented one removes the record outright, so nothing reports this yet.
    Deleting,
    Starting,
    Stopping,
    Ready,
    Running,
    /// Set by the configuration-changing operations (`UpdateApplication`, the
    /// `AddApplication*` / `DeleteApplication*` sub-resource operations) once they
    /// are implemented; all of them still answer 501, so nothing reports this yet.
    Updating,
}

impl ApplicationStatus {
    /// The value this status takes on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deleting => "DELETING",
            Self::Starting => "STARTING",
            Self::Stopping => "STOPPING",
            Self::Ready => "READY",
            Self::Running => "RUNNING",
            Self::Updating => "UPDATING",
        }
    }
}

/// A status transition the clock owes: at `due` the record moves to `target`.
#[derive(Debug, Clone, Copy)]
pub struct PendingTransition {
    pub due: Instant,
    pub target: ApplicationStatus,
}

/// One configured application input, with the `InputId` the service assigned it.
#[derive(Debug, Clone)]
pub struct InputRecord {
    pub input_id: String,
    pub name_prefix: String,
    pub kinesis_streams_input: Option<KinesisStreamsInput>,
    pub kinesis_firehose_input: Option<KinesisFirehoseInput>,
    pub input_processing_configuration: Option<InputProcessingConfiguration>,
    pub input_schema: SourceSchema,
    /// `InputParallelism.Count`: how many in-application streams this input feeds.
    pub input_parallelism_count: i32,
    /// Where the application was last told to start reading, by `StartApplication`.
    pub starting_position: Option<String>,
}

impl InputRecord {
    /// The in-application stream names this input creates.
    ///
    /// Per the `NamePrefix` doc (`smithy_query query=doc shape=Input
    /// member=NamePrefix`): a prefix `MyInApplicationStream` yields
    /// `MyInApplicationStream_001`, `MyInApplicationStream_002`, ... one per
    /// `InputParallelism` count.
    ///
    /// The count is clamped to the modelled `1..64` range before it is expanded, so
    /// a stored record can never make this allocate without bound — whatever put the
    /// value there.
    pub fn in_app_stream_names(&self) -> Vec<String> {
        let count = self
            .input_parallelism_count
            .clamp(MIN_INPUT_PARALLELISM_COUNT, MAX_INPUT_PARALLELISM_COUNT);
        (1..=count)
            .map(|n| format!("{}_{n:03}", self.name_prefix))
            .collect()
    }
}

/// One configured application output, with the `OutputId` the service assigned it.
#[derive(Debug, Clone)]
pub struct OutputRecord {
    pub output_id: String,
    pub name: String,
    pub kinesis_streams_output: Option<KinesisStreamsOutput>,
    pub kinesis_firehose_output: Option<KinesisFirehoseOutput>,
    pub lambda_output: Option<LambdaOutput>,
    pub destination_schema: DestinationSchema,
}

/// One CloudWatch log stream the application writes configuration errors to.
#[derive(Debug, Clone)]
pub struct CloudWatchLoggingOptionRecord {
    pub cloud_watch_logging_option_id: String,
    pub log_stream_arn: String,
    pub role_arn: String,
}

/// One reference data source, with the `ReferenceId` the service assigned it.
///
/// No implemented operation fills this yet (`AddApplicationReferenceDataSource`
/// and `UpdateApplication` are still 501); the field exists so that slice can be
/// added without reshaping the store, and `DescribeApplication` already reports
/// whatever it holds.
#[derive(Debug, Clone)]
pub struct ReferenceDataSourceRecord {
    pub reference_id: String,
    pub table_name: String,
    pub s3_reference: Option<S3ReferenceDataSource>,
    pub reference_schema: Option<SourceSchema>,
}

/// A stored Kinesis Analytics application.
#[derive(Debug, Clone)]
pub struct Application {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub status: ApplicationStatus,
    /// Set while a status move the clock owes has not come due yet.
    pub pending: Option<PendingTransition>,
    /// `ApplicationVersionId`: a real counter on the record, starting at 1 and
    /// bumped by the configuration-changing operations (modelled range
    /// `1..999999999`).
    pub version_id: i64,
    pub create_timestamp_millis: i64,
    pub last_update_timestamp_millis: i64,
    pub application_code: Option<String>,
    pub inputs: Vec<InputRecord>,
    pub outputs: Vec<OutputRecord>,
    pub reference_data_sources: Vec<ReferenceDataSourceRecord>,
    pub cloud_watch_logging_options: Vec<CloudWatchLoggingOptionRecord>,
    /// The application's tags, by key. A tag whose request carried no `Value` keeps
    /// `None`, so a later tagging slice can tell "no value" from "empty value".
    pub tags: BTreeMap<String, Option<String>>,
    /// Source of the server-assigned sub-resource ids on this application.
    pub next_id: u64,
}

impl Application {
    /// Mint the next server-assigned sub-resource id (`InputId`, `OutputId`,
    /// `ReferenceId`, `CloudWatchLoggingOptionId`). Stays inside `Id`'s modelled
    /// `1..50` length and `^[a-zA-Z0-9_.-]+$` pattern.
    pub fn mint_id(&mut self) -> String {
        self.next_id += 1;
        self.next_id.to_string()
    }

    /// Start a status move the clock owes: report `transient` now and `target`
    /// once [`TRANSITION_SPAN`] has elapsed.
    pub fn begin_transition(
        &mut self,
        transient: ApplicationStatus,
        target: ApplicationStatus,
        now: Instant,
    ) {
        self.status = transient;
        self.pending = Some(PendingTransition {
            due: now + TRANSITION_SPAN,
            target,
        });
    }

    /// Materialise a status move the clock has ALREADY made due.
    ///
    /// A reporting operation may do this: the move was owed at this instant
    /// whether or not anybody called.
    pub fn settle(&mut self, now: Instant) {
        let Some(pending) = self.pending else {
            return;
        };
        if now >= pending.due {
            self.status = pending.target;
            self.pending = None;
        }
    }

    /// This application's `ApplicationSummary`, read off the record.
    pub fn summary(&self) -> ApplicationSummary {
        ApplicationSummary {
            application_name: self.name.clone(),
            application_arn: self.arn.clone(),
            application_status: self.status.as_str().to_string(),
        }
    }
}

/// Per-service state for com-amazonaws-kinesisanalytics-kinesisanalytics-20150814. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisanalyticsKinesisanalytics20150814State {
    /// Applications by `ApplicationName`. A `BTreeMap` so `ListApplications`
    /// pages deterministically in name order.
    pub applications: BTreeMap<String, Application>,
}

impl ComAmazonawsKinesisanalyticsKinesisanalytics20150814State {
    /// Materialise every status move the clock has already made due, so an
    /// operation never reports a status the clock has moved on from.
    pub fn settle_due(&mut self, now: Instant) {
        for application in self.applications.values_mut() {
            application.settle(now);
        }
    }
}

/// Epoch milliseconds for the current instant, the unit the stored timestamps use.
pub fn now_epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since_epoch| i64::try_from(since_epoch.as_millis()).ok())
        .unwrap_or_default()
}

/// The epoch milliseconds a wire `Timestamp` names, rounded to the nearest
/// millisecond.
///
/// awsJson1_1 carries a timestamp as fractional epoch seconds and a JSON number
/// is parsed through `f64`, so the sub-second part arrives within a few hundred
/// nanoseconds of the value that was sent. Rounding to the nearest millisecond
/// recovers exactly the value the service handed out, which is what lets
/// `DeleteApplication` compare the supplied `CreateTimestamp` for equality
/// instead of against a tolerance.
///
/// Total for every value a request can carry: the seconds a `f64` timestamp
/// saturates to are multiplied with `saturating_mul`, so an absurd
/// `CreateTimestamp` yields a far-away millisecond count (which matches no stored
/// record) rather than overflowing.
pub fn epoch_millis(timestamp: &::aws_smithy_types::DateTime) -> i64 {
    timestamp
        .secs()
        .saturating_mul(1000)
        .saturating_add(i64::from(
            (timestamp.subsec_nanos() + 500_000) / 1_000_000,
        ))
}

/// A stored epoch-millisecond timestamp as the modelled `Timestamp` type.
pub fn date_time(epoch_millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(epoch_millis)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKinesisanalyticsKinesisanalytics20150814State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKinesisanalyticsKinesisanalytics20150814State>)
}
