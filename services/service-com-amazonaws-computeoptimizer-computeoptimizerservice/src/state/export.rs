// SPDX-License-Identifier: CC0-1.0

//! The FILES a recommendations export job writes, and the write itself.
//!
//! Every `Export*Recommendations` operation of this service queues one asynchronous export:
//! "Recommendations are exported in a comma-separated values (.csv) file, and its metadata
//! in a JavaScript Object Notation (JSON) (.json) file, to an existing Amazon Simple Storage
//! Service (Amazon S3) bucket that you specify." The job record and its lifecycle live in
//! [`super`]; this module is the other half — WHAT the two objects hold, and the write that
//! puts them in the destination bucket once the job completes.
//!
//! The one object store in this emulator is the Amazon S3 service's own, so the export
//! objects are written there and are then read back with `GetObject` / `ListObjectsV2` like
//! any other object. That is also why a destination this emulator does not hold is REFUSED
//! at the export call ([`destination_bucket_rejection`]): "You must create the destination
//! Amazon S3 bucket for your recommendations export before you create the export job.
//! Compute Optimizer does not create the S3 bucket for you."
//!
//! What the files hold:
//!
//! * the export file — one HEADER row of the `Exportable*Field` names the request asked for
//!   ("The recommendations data to include in the export file"), then one row per
//!   recommendation the account's analysis holds for the job's resource type, and one per
//!   recommendation OPTION where the fields name an option's values (each
//!   `RecommendationOptions*` field names ONE option). Each cell is that field's value for
//!   that recommendation, computed from the stored analysis the same way the recommendation
//!   READER computes it, so an exported row and a `Get*Recommendations` response cannot
//!   disagree. A field the stored analysis records no value for is an empty cell.
//! * the metadata file — what the export job itself was: its id, the resource type and
//!   account(s) it covers, the file format and columns of the export file, how many rows it
//!   holds and where both objects were written.
//!
//! AWS publishes no column layout, row order or metadata schema for either file, so both are
//! the emulator's own; every value in them is nevertheless derived from the stored analysis
//! and from the request that asked for the export.
//!
//! ## The clock
//!
//! The objects appear when the job REACHES `Complete` ([`ExportJob::status_at`]), not when
//! the export call returns: a client that reads the destination while the job is still
//! `Queued` or `InProgress` finds nothing there, as it would against the real service. The
//! files are computed at the export call (so the export is of the analysis as it stood when
//! the job was created) and held with the job until the clock makes the completion due;
//! [`write_due_exports`] then performs exactly the writes the stored due instant has already
//! made due, and no operation ever sleeps for it. A client that polls
//! `DescribeRecommendationExportJobs` to `Complete` and then downloads the file is the
//! documented workflow, and [`drive_export_clock`] starts one short in-process task per job
//! so the objects also appear for a client that never calls this API again.
//!
//! ## Lock order
//!
//! This service's store is locked, the due objects are TAKEN out of it, and the lock is
//! released before the object store is locked: the two locks are never held at once, and the
//! Amazon S3 service never reaches into this one, so the two cannot deadlock.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use ::service_com_amazonaws_s3_amazons3 as s3;

use super::{
    look_back_period_in_days, parse_resource_arn, percentage_of, rds_finding_members,
    reason_codes_in, round_hundredths, AnalysedResource, AnalyzedAutoScalingGroup,
    ComAmazonawsComputeoptimizerComputeoptimizerserviceState, EffectivePreferences, ExportJob,
    GroupConfiguration, IdleResourceAnalysis, ObservedLicensedInstance, ANALYSIS_DELAY,
    CURRENCY_USD, IDLE_FALSE, IDLE_TRUE, RESOURCE_TYPE_AUTO_SCALING_GROUP,
    RESOURCE_TYPE_EBS_VOLUME, RESOURCE_TYPE_EC2_INSTANCE, RESOURCE_TYPE_ECS_SERVICE,
    RESOURCE_TYPE_IDLE, RESOURCE_TYPE_LAMBDA_FUNCTION, RESOURCE_TYPE_LICENSE,
    RESOURCE_TYPE_RDS_DB_INSTANCE, SAVINGS_ESTIMATION_SOURCE,
};
use crate::ops::types::{
    R_D_S_INSTANCE_FINDING_REASON_CODE_VALUES, R_D_S_STORAGE_FINDING_REASON_CODE_VALUES,
};

/// The one `FileFormat` the model declares: "The only export file format currently supported
/// is Csv" (`crate::ops::types::FILE_FORMAT_VALUES`).
pub const FILE_FORMAT_CSV: &str = "Csv";

/// How a cell holding several values separates them, so one exportable field is still one
/// column. AWS publishes no layout for the export file, so this is the emulator's own.
const LIST_SEPARATOR: &str = "|";

/// One row of an export file: the value of each exportable field the recommendation carries,
/// keyed by the field's modelled name (an `Exportable*Field` value). A field the analysis
/// records no value for has NO entry, and the cell under its column is empty.
pub type ExportRow = BTreeMap<String, String>;

/// One filter of any of this service's filter vocabularies.
///
/// Each `Export*Recommendations` operation carries its own filter shape and every one of them
/// is a name with the values it selects, so this is what lets one [`ExportPlan`] take the
/// filters of any of those surfaces ([`filter_pairs`]) instead of each handler restating the
/// same mapping.
pub trait ExportFilter {
    /// The `*FilterName` the filter carries, or `None` for one that names nothing.
    fn filter_name(&self) -> Option<&str>;
    /// The values it selects on; empty constrains nothing.
    fn filter_values(&self) -> &[String];
}

/// Implement [`ExportFilter`] for the modelled filter shapes, which all declare the same two
/// members.
macro_rules! export_filters {
    ($($shape:ty),+ $(,)?) => {
        $(impl ExportFilter for $shape {
            fn filter_name(&self) -> Option<&str> {
                self.name.as_deref()
            }

            fn filter_values(&self) -> &[String] {
                self.values.as_deref().unwrap_or_default()
            }
        })+
    };
}

export_filters!(
    crate::ops::types::Filter,
    crate::ops::types::EBSFilter,
    crate::ops::types::ECSServiceRecommendationFilter,
    crate::ops::types::IdleRecommendationFilter,
    crate::ops::types::LambdaFunctionRecommendationFilter,
    crate::ops::types::LicenseRecommendationFilter,
    crate::ops::types::RDSDBRecommendationFilter,
);

/// A request's filters as the (name, values) pairs an [`ExportPlan`] carries.
pub fn filter_pairs<F: ExportFilter>(filters: &[F]) -> Vec<(String, Vec<String>)> {
    filters
        .iter()
        .map(|filter| {
            (
                filter.filter_name().unwrap_or_default().to_string(),
                filter.filter_values().to_vec(),
            )
        })
        .collect()
}

/// Everything one export call settles about the job it creates: where the two objects go, and
/// what they hold. The job record and both files are derived from this, so the destination the
/// call answers, the destination the job reports and the objects that actually appear cannot
/// name different places.
#[derive(Debug, Clone)]
pub struct ExportPlan {
    /// The `JobId` minted for the job.
    pub job_id: String,
    /// The Region whose endpoint created it.
    pub region: String,
    /// The `ResourceType` whose recommendations are exported.
    pub resource_type: String,
    /// The destination bucket the request named, and the two object keys the export file and
    /// its metadata file are written under ([`super::export_destination_keys`]).
    pub bucket: String,
    /// See [`ExportPlan::bucket`].
    pub key: String,
    /// See [`ExportPlan::bucket`].
    pub metadata_key: String,
    /// The columns of the export file ([`export_columns`]).
    pub columns: Vec<String>,
    /// The `FileFormat` the export file is written in.
    pub file_format: String,
    /// The `accountIds` the request named, and whether it asked for the organization's member
    /// accounts — what the export covers, which the metadata file states.
    pub account_ids: Vec<String>,
    /// See [`ExportPlan::account_ids`].
    pub include_member_accounts: bool,
    /// The request's filters as (name, values) pairs — "an array of objects to specify a
    /// filter that exports a more specific set of ... recommendations" — which select which
    /// recommendations the export file holds ([`row_matches_filters`]).
    pub filters: Vec<(String, Vec<String>)>,
}

/// The two objects an export job has computed but not yet written: the export file and its
/// metadata file, held with the job until the clock makes the job's completion due.
#[derive(Debug, Clone)]
pub struct PendingExport {
    /// The export file's bytes.
    pub export_file: String,
    /// The metadata file's bytes.
    pub metadata_file: String,
}

/// One job's objects, taken out of the store because the clock has made the job's completion
/// due, with where they go — what [`write_due_exports`] then writes.
#[derive(Debug, Clone)]
pub struct DueExport {
    /// The job the objects belong to, which is what a failed write is recorded against.
    pub job_id: String,
    /// The destination bucket, and the keys the two objects are written under.
    pub bucket: String,
    /// See [`DueExport::bucket`].
    pub key: String,
    /// See [`DueExport::bucket`].
    pub metadata_key: String,
    /// The objects' bytes.
    pub pending: PendingExport,
}

/// Why a request's destination is not one this export can be written to, or `None` when it
/// is.
///
/// "You must create the destination Amazon S3 bucket for your recommendations export before
/// you create the export job. Compute Optimizer does not create the S3 bucket for you" — so a
/// request naming a bucket the object store does not hold is refused rather than accepted
/// into a job whose file could never appear.
pub fn destination_bucket_rejection<Eng: Send + Sync + 'static>(
    engine: &Eng,
    bucket: &str,
) -> Option<String> {
    let held = s3::state::service_state(engine)
        .is_some_and(|store| store.lock().bucket(bucket).is_some());
    (!held).then(|| {
        format!(
            "The value supplied for parameter s3DestinationConfig is not valid: the Amazon S3 \
             bucket {bucket} does not exist. You must create the destination Amazon S3 bucket \
             for your recommendations export before you create the export job."
        )
    })
}

/// The columns of an export file: the fields the request asked for, in the order it asked for
/// them, or — for a request that names none — every field the resource type's
/// `Exportable*Field` enum declares.
///
/// "The recommendations data to include in the export file": the request chooses which of its
/// resource type's fields the file holds, and a request that chooses none gets the whole set
/// rather than a file of no columns.
///
/// Only the fields that resource type's enum DECLARES, each once: the column count is then
/// bounded by the size of that enum, so neither the file this builds nor the work of building
/// it can be grown by a request that repeats a name or carries a long list of them. (The
/// request boundary already refuses a value the enum does not declare, so a name dropped here
/// is one that could not have arrived.)
pub fn export_columns(requested: Option<&[String]>, modelled: &[&str]) -> Vec<String> {
    let requested = requested.unwrap_or_default();
    if requested.is_empty() {
        return modelled.iter().map(|field| (*field).to_string()).collect();
    }
    let declared: HashSet<&str> = modelled.iter().copied().collect();
    let mut taken: HashSet<&str> = HashSet::with_capacity(modelled.len());
    let mut columns: Vec<String> = Vec::new();
    for field in requested {
        if let Some(field) = declared.get(field.as_str()) {
            if taken.insert(field) {
                columns.push((*field).to_string());
            }
        }
    }
    columns
}

/// The export file: the column header row, then one row per recommendation.
///
/// Comma-separated values as the `csv` crate writes them — quoting a cell that needs it, with
/// the CRLF record terminator RFC 4180 states — so the quoting rules are the codec's and not
/// re-derived here. A row holds no byte the codec can refuse, so the writer cannot fail; an
/// unexpected failure yields an EMPTY file rather than a truncated one a client could not tell
/// from a complete export, and is logged.
pub fn export_csv(columns: &[String], rows: &[ExportRow]) -> String {
    let mut writer = ::csv::WriterBuilder::new()
        .terminator(::csv::Terminator::CRLF)
        .from_writer(Vec::new());
    let mut write = || -> Result<(), ::csv::Error> {
        writer.write_record(columns)?;
        for row in rows {
            writer.write_record(
                columns
                    .iter()
                    .map(|column| row.get(column).map_or("", String::as_str)),
            )?;
        }
        writer.flush().map_err(::csv::Error::from)
    };
    if let Err(error) = write() {
        ::tracing::warn!(%error, "the recommendations export file could not be written");
        return String::new();
    }
    writer
        .into_inner()
        .map(|file| String::from_utf8_lossy(&file).into_owned())
        .unwrap_or_default()
}

/// The metadata file of the export `plan` describes, holding `rows` recommendation rows and
/// created at `created_at`: what the export job was, and where its two objects were written.
fn export_metadata(plan: &ExportPlan, rows: usize, created_at: f64) -> String {
    let metadata = ::serde_json::json!({
        "jobId": plan.job_id,
        "region": plan.region,
        "resourceType": plan.resource_type,
        "accountIds": if plan.account_ids.is_empty() {
            vec![::engine::emulator_account_id().to_string()]
        } else {
            plan.account_ids.clone()
        },
        "includeMemberAccounts": plan.include_member_accounts,
        "creationTimestamp": timestamp(created_at),
        "exportFile": {
            "fileFormat": plan.file_format,
            "columns": plan.columns,
            "rowCount": rows,
        },
        "destination": {
            "bucket": plan.bucket,
            "key": plan.key,
            "metadataKey": plan.metadata_key,
        },
    });
    ::serde_json::to_string_pretty(&metadata).unwrap_or_else(|_| metadata.to_string())
}

/// The export COLUMN one filter name selects on, for the surface that exports
/// `resource_type` — the member the recommendation reader of that surface resolves the same
/// name to.
///
/// Each `Export*Recommendations` operation carries its own filter vocabulary, and a name is
/// spelled for the member it selects on rather than for the exportable field, so the two are
/// mapped here explicitly: the ECS, Lambda and License surfaces name one reason code
/// (`FindingReasonCode`) while the field is the list `FindingReasonCodes`, the License surface
/// selects on the license's name, and the database surface selects on the instance and storage
/// findings, their reason codes and whether the database is idle. `None` is a name the surface
/// resolves to no member of its own recommendation, which therefore selects nothing — the rule
/// the readers apply (`FindingReasonCodes` on the Auto Scaling group surface, say, whose
/// recommendation declares no such member).
fn filter_column_of(resource_type: &str, name: &str) -> Option<&'static str> {
    match (resource_type, name) {
        // The `Filters` vocabulary, shared by the Auto Scaling group and instance surfaces.
        (_, "Finding") => Some("Finding"),
        (_, "InferredWorkloadTypes") => Some("InferredWorkloadTypes"),
        // `FindingReasonCodes` selects nothing on the Auto Scaling group surface: an
        // `AutoScalingGroupRecommendation` declares no finding reason codes.
        (RESOURCE_TYPE_AUTO_SCALING_GROUP, "FindingReasonCodes") => None,
        (_, "FindingReasonCodes" | "FindingReasonCode") => Some("FindingReasonCodes"),
        // The idle resource surface.
        (RESOURCE_TYPE_IDLE, "ResourceType") => Some("ResourceType"),
        // The commercial software license surface.
        (RESOURCE_TYPE_LICENSE, "LicenseName") => {
            Some("CurrentLicenseConfigurationLicenseName")
        }
        // The database surface, which reports a finding per side of the analysis.
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "InstanceFinding") => Some("InstanceFinding"),
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "StorageFinding") => Some("StorageFinding"),
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "InstanceFindingReasonCode") => {
            Some("InstanceFindingReasonCodes")
        }
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "StorageFindingReasonCode") => {
            Some("StorageFindingReasonCodes")
        }
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "Idle") => Some("Idle"),
        _ => None,
    }
}

/// Whether `row` satisfies every filter of the request that asked for the export.
///
/// A filter selects on the member its name names ([`filter_column_of`]) and the row matches
/// when that column holds any of the filter's values; `RecommendationSourceType` selects on
/// the resource type the job exports, which is the same for every row. Several filters are
/// ANDed and a filter carrying no values constrains nothing. A filter naming a member the
/// surface's own recommendation does not declare selects nothing, which is what the
/// recommendation readers of this service answer for the same filter sets.
fn row_matches_filters(
    row: &ExportRow,
    resource_type: &str,
    filters: &[(String, Vec<String>)],
) -> bool {
    filters.iter().all(|(name, values)| {
        if values.is_empty() {
            return true;
        }
        let held = if name == "RecommendationSourceType" {
            resource_type
        } else {
            match filter_column_of(resource_type, name) {
                Some(column) => row.get(column).map_or("", String::as_str),
                None => return false,
            }
        };
        held.split(LIST_SEPARATOR)
            .any(|item| !item.is_empty() && values.iter().any(|value| value == item))
    })
}

/// An instant as the export files state it: RFC 3339 in UTC, the form every timestamp this
/// service reports on the wire takes.
fn timestamp(at: f64) -> String {
    ::aws_smithy_types::DateTime::from_secs_f64(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// Record `value` for `field` in `row`, unless it is empty: a field the analysis records no
/// value for has no entry at all, so its cell is empty whatever column order a request asks
/// for.
fn put(row: &mut ExportRow, field: &str, value: impl Into<String>) {
    let value: String = value.into();
    if !value.is_empty() {
        row.insert(field.to_string(), value);
    }
}

/// [`put`] for a derived figure, at the two decimal places every figure this service reports
/// is rounded to.
fn put_number(row: &mut ExportRow, field: &str, value: f64) {
    put(row, field, round_hundredths(value).to_string());
}

/// [`put`] for a list-valued member, as one cell ([`LIST_SEPARATOR`]).
fn put_list<T: AsRef<str>>(row: &mut ExportRow, field: &str, values: &[T]) {
    let joined: Vec<&str> = values.iter().map(AsRef::as_ref).collect();
    put(row, field, joined.join(LIST_SEPARATOR));
}

/// The columns every resource type's export file shares, from the preferences the
/// recommendation was generated under: `EffectiveRecommendationPreferences*` and the
/// look-back period in days, which is the length of the period in force.
fn put_preferences(row: &mut ExportRow, preferences: &EffectivePreferences) {
    put_number(
        row,
        "LookbackPeriodInDays",
        look_back_period_in_days(&preferences.look_back_period),
    );
    put(
        row,
        "EffectiveRecommendationPreferencesLookBackPeriod",
        preferences.look_back_period.clone(),
    );
    put(
        row,
        "EffectiveRecommendationPreferencesInferredWorkloadTypes",
        preferences.inferred_workload_types.clone(),
    );
    put(
        row,
        "EffectiveRecommendationPreferencesEnhancedInfrastructureMetrics",
        preferences
            .enhanced_infrastructure_metrics
            .clone()
            .unwrap_or_default(),
    );
    // `savingsEstimationMode` is an `InstanceSavingsEstimationMode`, whose one member is the
    // `source` the money figures came from — which is what the reader reports for it.
    put(
        row,
        "EffectiveRecommendationPreferencesSavingsEstimationMode",
        SAVINGS_ESTIMATION_SOURCE,
    );
    let preferred: Vec<String> = preferences
        .preferred_resources
        .iter()
        .flat_map(|resource| resource.include_list.clone().unwrap_or_default())
        .collect();
    put_list(
        row,
        "EffectiveRecommendationPreferencesPreferredResources",
        &preferred,
    );
}

/// The exportable field one utilization metric lands under:
/// `UtilizationMetrics<MetricName><Statistic>`.
///
/// The field sets spell every metric name as its own enum does, bar the CPU series: the
/// `IdleMetricName` value is `CPU` while the field is `UtilizationMetricsCpuMaximum`.
fn utilization_field(prefix: &str, name: &str, statistic: &str) -> String {
    let name = if name == "CPU" { "Cpu" } else { name };
    format!("{prefix}{name}{statistic}")
}

/// The `Exportable*Field` an export file names a resource's own ARN under: each resource
/// type's enum declares the member its recommendation reports the ARN in
/// (`InstanceArn`, `VolumeArn`, `FunctionArn`, `ServiceArn`, `AutoScalingGroupArn`), and the
/// types whose recommendation reports it as `resourceArn` name it `ResourceArn`.
fn arn_field_of(resource_type: &str) -> &'static str {
    match resource_type {
        RESOURCE_TYPE_AUTO_SCALING_GROUP => "AutoScalingGroupArn",
        RESOURCE_TYPE_EC2_INSTANCE => "InstanceArn",
        RESOURCE_TYPE_EBS_VOLUME => "VolumeArn",
        RESOURCE_TYPE_LAMBDA_FUNCTION => "FunctionArn",
        RESOURCE_TYPE_ECS_SERVICE => "ServiceArn",
        _ => "ResourceArn",
    }
}

impl ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// Create the export job `plan` describes, with the two objects it writes when the clock
    /// makes it complete, and answer the record as stored.
    ///
    /// The files are computed HERE, from the analysis as it stands at `now`, so the export is
    /// of the recommendations the account held when it asked for the export rather than of
    /// whatever the analysis says when the job happens to complete.
    pub fn record_export_job(&mut self, plan: &ExportPlan, now: f64) -> ExportJob {
        let rows = self.export_rows(plan);
        let job = ExportJob {
            job_id: plan.job_id.clone(),
            region: plan.region.clone(),
            resource_type: plan.resource_type.clone(),
            bucket: plan.bucket.clone(),
            key: plan.key.clone(),
            metadata_key: plan.metadata_key.clone(),
            created_at: now,
            pending: Some(PendingExport {
                export_file: export_csv(&plan.columns, &rows),
                metadata_file: export_metadata(plan, rows.len(), now),
            }),
            failure_reason: None,
        };
        self.export_jobs.insert(job.clone());
        job
    }

    /// The recommendation rows one export file holds: those of the job's resource type that
    /// the account's analysis holds and the request's filters select, in analysis order.
    ///
    /// The rows are the ONE account this emulator presents
    /// ([`engine::emulator_account_id`]): `accountIds` and `includeMemberAccounts` select
    /// whose recommendations go into the file, and this emulator records no organization and
    /// holds no other account's analysis, so neither of them selects any row away or adds one
    /// — they are stated in the metadata file as what the export was asked for.
    ///
    /// Compute Optimizer reports recommendations for an ENROLLED account only, and only once
    /// the analysis its enrollment starts is due, so an export created before then holds the
    /// header row alone — exactly what the recommendation readers report at that moment.
    fn export_rows(&self, plan: &ExportPlan) -> Vec<ExportRow> {
        if !self.enrollment.is_opted_in() || !self.enrollment.analysis_due() {
            return Vec::new();
        }
        let last_refresh = self
            .enrollment
            .last_updated_timestamp
            .map(|written| written.as_secs_f64() + ANALYSIS_DELAY.as_secs_f64());
        let mut rows = match plan.resource_type.as_str() {
            RESOURCE_TYPE_AUTO_SCALING_GROUP => self.auto_scaling_group_export_rows(last_refresh),
            RESOURCE_TYPE_IDLE => self.idle_export_rows(&plan.region, last_refresh),
            RESOURCE_TYPE_LICENSE => self.license_export_rows(&plan.region, last_refresh),
            resource_type => {
                self.analysed_resource_export_rows(resource_type, &plan.region, last_refresh)
            }
        };
        rows.retain(|row| row_matches_filters(row, &plan.resource_type, &plan.filters));
        rows
    }

    /// The export rows of the Auto Scaling group analysis: one per recommendation option of
    /// every analysed group, since each `RecommendationOptions*` field names ONE option's
    /// value — and one row carrying the group's own columns for a group the analysis found no
    /// candidate for.
    fn auto_scaling_group_export_rows(&self, last_refresh: Option<f64>) -> Vec<ExportRow> {
        let mut rows: Vec<ExportRow> = Vec::new();
        for group in self.visible_auto_scaling_group_analyses() {
            let Some(parsed) = parse_resource_arn(&group.arn) else {
                continue;
            };
            let preferences = self.effective_preferences(&group.arn, parsed);
            // "Describes the CPU vendor and architecture for an instance or Auto Scaling group
            // recommendations": the architectures the reported options consist of.
            let mut architectures: Vec<String> = Vec::new();
            for candidate in &group.candidates {
                if !architectures.contains(&candidate.architecture) {
                    architectures.push(candidate.architecture.clone());
                }
            }
            if group.candidates.is_empty() {
                rows.push(auto_scaling_group_row(
                    group,
                    &preferences,
                    None,
                    &architectures,
                    last_refresh,
                ));
                continue;
            }
            for candidate in &group.candidates {
                rows.push(auto_scaling_group_row(
                    group,
                    &preferences,
                    Some(candidate),
                    &architectures,
                    last_refresh,
                ));
            }
        }
        rows
    }

    /// The export rows of the findings catalog, for a resource type whose recommendation is
    /// reported from it: one row per analysed resource, carrying the columns the catalog row
    /// records ([`analysed_resource_row`]).
    fn analysed_resource_export_rows(
        &self,
        resource_type: &str,
        region: &str,
        last_refresh: Option<f64>,
    ) -> Vec<ExportRow> {
        let account_id = ::engine::emulator_account_id();
        let by_type = self.analysed_resources_by_type(account_id);
        by_type
            .get(resource_type)
            .map(|resources| {
                resources
                    .iter()
                    .map(|resource| {
                        let arn = resource.arn_in(region);
                        let preferences =
                            self.effective_preferences_for(&arn, resource_type, account_id);
                        // "Describes if an Amazon Aurora or RDS database is idle": it is
                        // exactly when this service also holds an IDLE recommendation for it,
                        // which is what the database reader reports from.
                        let idle = self.idle_recommendations.get(&arn).is_some();
                        analysed_resource_row(
                            resource,
                            resource_type,
                            &arn,
                            &preferences,
                            idle,
                            last_refresh,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The export rows of the commercial software license inventory: one per licensed
    /// instance the analysis observes, from the SAME observation
    /// `GetLicenseRecommendations` reports ([`ObservedLicensedInstance`]) rather than from the
    /// findings catalog, which records no license configuration.
    fn license_export_rows(&self, region: &str, last_refresh: Option<f64>) -> Vec<ExportRow> {
        let account_id = ::engine::emulator_account_id();
        self.visible_licenses()
            .filter(|license| license.account_id == account_id)
            .map(|license| {
                let arn = license.instance_arn(region);
                let preferences =
                    self.effective_preferences_for(&arn, RESOURCE_TYPE_LICENSE, account_id);
                license_row(license, &arn, region, &preferences, last_refresh)
            })
            .collect()
    }

    /// The export rows of the idle resource analysis: one per resource the analysis found
    /// idle, with the utilization it observed as the evidence for the finding.
    fn idle_export_rows(&self, region: &str, last_refresh: Option<f64>) -> Vec<ExportRow> {
        let account_id = ::engine::emulator_account_id();
        self.visible_idle_recommendations()
            .filter(|analysis| analysis.account_id == account_id)
            .map(|analysis| {
                let arn = analysis.arn_in(region);
                let preferences = self.effective_preferences_for(&arn, RESOURCE_TYPE_IDLE, account_id);
                idle_row(analysis, &arn, &preferences, last_refresh)
            })
            .collect()
    }
}

/// One Auto Scaling group recommendation option as an export row.
///
/// Every cell is the value the recommendation READER reports for that member, computed from
/// the stored analysis the same way ([`AnalyzedAutoScalingGroup`]), so an exported row and a
/// `GetAutoScalingGroupRecommendations` response cannot disagree. The fields this analysis
/// records nothing for are left empty: the reserved-instance prices and the storage and
/// network of a configuration (the analysis prices an instance-hour on public on-demand
/// pricing alone), the GPU accelerator settings and the GPU utilization (no instance type in
/// this analysis has an attached accelerator), the EBS, disk and network utilization (the
/// analysis observes CPU and memory), and an option's estimated instance-hour reduction
/// (which "is only available for EC2 Auto Scaling groups with scaling policies", and this
/// analysis records no scaling policy).
fn auto_scaling_group_row(
    group: &AnalyzedAutoScalingGroup,
    preferences: &EffectivePreferences,
    option: Option<&GroupConfiguration>,
    architectures: &[String],
    last_refresh: Option<f64>,
) -> ExportRow {
    let mut row = ExportRow::new();
    put(&mut row, "AccountId", group.account_id.clone());
    put(&mut row, "AutoScalingGroupArn", group.arn.clone());
    put(&mut row, "AutoScalingGroupName", group.name.clone());
    put(&mut row, "Finding", group.finding());
    put(
        &mut row,
        "CurrentPerformanceRisk",
        group.current_performance_risk.clone(),
    );
    for metric in group.utilization_metrics() {
        let (Some(name), Some(statistic)) = (metric.name, metric.statistic) else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field("UtilizationMetrics", &name, &statistic),
            metric.value,
        );
    }
    put_configuration(&mut row, "CurrentConfiguration", &group.current);
    put_number(&mut row, "CurrentVCpus", group.current.vcpus);
    put_number(&mut row, "CurrentMemory", group.current.memory_gib);
    put_number(&mut row, "CurrentOnDemandPrice", group.current.hourly_price);
    put_preferences(&mut row, preferences);
    put_list(
        &mut row,
        "EffectiveRecommendationPreferencesCpuVendorArchitectures",
        architectures,
    );
    if preferences.infers_workload_types() {
        put_list(
            &mut row,
            "InferredWorkloadTypes",
            &group.inferred_workload_types,
        );
    }
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }

    let Some(option) = option else {
        return row;
    };
    put_configuration(&mut row, "RecommendationOptionsConfiguration", option);
    put_number(&mut row, "RecommendationOptionsVcpus", option.vcpus);
    put_number(&mut row, "RecommendationOptionsMemory", option.memory_gib);
    put_number(
        &mut row,
        "RecommendationOptionsOnDemandPrice",
        option.hourly_price,
    );
    put_number(
        &mut row,
        "RecommendationOptionsPerformanceRisk",
        option.performance_risk,
    );
    for metric in group.projected_utilization_metrics(option) {
        let (Some(name), Some(statistic)) = (metric.name, metric.statistic) else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field(
                "RecommendationOptionsProjectedUtilizationMetrics",
                &name,
                &statistic,
            ),
            metric.value,
        );
    }
    put(
        &mut row,
        "RecommendationOptionsMigrationEffort",
        group.migration_effort(option, preferences.infers_workload_types()),
    );
    let (saving, percentage) = group.savings(option, 0.0);
    put_number(
        &mut row,
        "RecommendationOptionsSavingsOpportunityPercentage",
        percentage,
    );
    put_number(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsValue",
        saving,
    );
    put(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsCurrency",
        CURRENCY_USD,
    );
    // The after-discount figures are reported only where the account activated the
    // preference, which is what "activates" savings net of Savings Plans and Reserved
    // Instances discounts.
    if preferences.after_discounts() {
        let (saving, percentage) = group.savings(option, group.savings_plans_discount_rate);
        put_number(
            &mut row,
            "RecommendationOptionsSavingsOpportunityAfterDiscountsPercentage",
            percentage,
        );
        put_number(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsValueAfterDiscounts",
            saving,
        );
        put(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsCurrencyAfterDiscounts",
            CURRENCY_USD,
        );
    }
    row
}

/// One Auto Scaling group configuration's columns, under the prefix the field names give it
/// (`CurrentConfiguration…` for the group's own, `RecommendationOptionsConfiguration…` for an
/// option's): the members `AutoScalingGroupConfiguration` reports
/// ([`GroupConfiguration::to_wire`]).
fn put_configuration(row: &mut ExportRow, prefix: &str, configuration: &GroupConfiguration) {
    let wire = configuration.to_wire();
    put(
        row,
        &format!("{prefix}InstanceType"),
        wire.instance_type.unwrap_or_default(),
    );
    put_list(
        row,
        &format!("{prefix}MixedInstanceTypes"),
        &wire.mixed_instance_types.unwrap_or_default(),
    );
    put(row, &format!("{prefix}Type"), wire.r#type.unwrap_or_default());
    put(
        row,
        &format!("{prefix}AllocationStrategy"),
        wire.allocation_strategy.unwrap_or_default(),
    );
    put(
        row,
        &format!("{prefix}DesiredCapacity"),
        wire.desired_capacity.to_string(),
    );
    put(row, &format!("{prefix}MinSize"), wire.min_size.to_string());
    put(row, &format!("{prefix}MaxSize"), wire.max_size.to_string());
}

/// One findings-catalog row as an export row: the columns a catalog row records about any
/// analysed resource — whose account owns it, its ARN at the Region's endpoint, its finding
/// and the reason codes behind it, the risk its current configuration runs, what adopting the
/// recommendation saves, and the preferences the recommendation was generated under.
///
/// The columns specific to a resource type's own configuration are left empty here: they are
/// members of that type's recommendation rather than of the catalog row every findings reader
/// shares.
fn analysed_resource_row(
    resource: &AnalysedResource,
    resource_type: &str,
    arn: &str,
    preferences: &EffectivePreferences,
    idle: bool,
    last_refresh: Option<f64>,
) -> ExportRow {
    let mut row = ExportRow::new();
    put(&mut row, "AccountId", resource.account_id.clone());
    put(&mut row, arn_field_of(resource_type), arn);
    // A database's analysis reports a finding for its INSTANCE and one for its STORAGE
    // rather than the one `Finding` every other resource type reports, and each side's reason
    // codes are the stored codes that side's own vocabulary declares.
    if resource_type == RESOURCE_TYPE_RDS_DB_INSTANCE {
        let (instance, storage) = rds_finding_members(&resource.finding);
        put(&mut row, "InstanceFinding", instance.unwrap_or_default());
        put(&mut row, "StorageFinding", storage.unwrap_or_default());
        put_list(
            &mut row,
            "InstanceFindingReasonCodes",
            &reason_codes_in(
                R_D_S_INSTANCE_FINDING_REASON_CODE_VALUES,
                &resource.finding_reason_codes,
            ),
        );
        put_list(
            &mut row,
            "StorageFindingReasonCodes",
            &reason_codes_in(
                R_D_S_STORAGE_FINDING_REASON_CODE_VALUES,
                &resource.finding_reason_codes,
            ),
        );
        put(
            &mut row,
            "Idle",
            if idle { IDLE_TRUE } else { IDLE_FALSE },
        );
        put(
            &mut row,
            "CurrentInstancePerformanceRisk",
            resource.current_performance_risk.clone(),
        );
        put_number(
            &mut row,
            "InstanceRecommendationOptionsSavingsOpportunityPercentage",
            percentage_of(resource.monthly_savings(), resource.current_monthly_cost),
        );
        put_number(
            &mut row,
            "InstanceRecommendationOptionsEstimatedMonthlySavingsValue",
            resource.monthly_savings(),
        );
        put(
            &mut row,
            "InstanceRecommendationOptionsEstimatedMonthlySavingsCurrency",
            CURRENCY_USD,
        );
    } else {
        put(&mut row, "Finding", resource.finding.clone());
        put_list(
            &mut row,
            "FindingReasonCodes",
            &resource.finding_reason_codes,
        );
        put(
            &mut row,
            "CurrentPerformanceRisk",
            resource.current_performance_risk.clone(),
        );
        put_number(
            &mut row,
            "RecommendationOptionsSavingsOpportunityPercentage",
            percentage_of(resource.monthly_savings(), resource.current_monthly_cost),
        );
        put_number(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsValue",
            resource.monthly_savings(),
        );
        put(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsCurrency",
            CURRENCY_USD,
        );
    }
    put_list(
        &mut row,
        "InferredWorkloadTypes",
        &resource.inferred_workload_types,
    );
    put_preferences(&mut row, preferences);
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }
    row
}

/// One commercial software license recommendation as an export row: the members
/// `LicenseRecommendation` reports, each computed from the stored observation the same way
/// `GetLicenseRecommendations` computes it — the license configuration in force, the finding
/// and the reason code behind it, and the one option the analysis has (the edition it would
/// move the license to) with what adopting it saves. A license with no price carries no
/// savings figure rather than an invented one.
fn license_row(
    license: &ObservedLicensedInstance,
    arn: &str,
    region: &str,
    preferences: &EffectivePreferences,
    last_refresh: Option<f64>,
) -> ExportRow {
    let mut row = ExportRow::new();
    put(&mut row, "AccountId", license.account_id.clone());
    put(&mut row, "ResourceArn", arn);
    put(&mut row, "Finding", license.finding());
    put_list(
        &mut row,
        "FindingReasonCodes",
        &license.finding_reason_codes(),
    );
    put_number(
        &mut row,
        "LookbackPeriodInDays",
        license.analysed_days(&preferences.look_back_period),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationInstanceType",
        license.instance_type.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationOperatingSystem",
        license.operating_system.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationLicenseName",
        license.license_name.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationLicenseEdition",
        license.license_edition.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationLicenseModel",
        license.license_model.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationLicenseVersion",
        license.license_version.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationNumberOfCores",
        license.number_of_cores.to_string(),
    );
    // The metric source is the CloudWatch Application Insights application that watches the
    // instance, which the reader reports as its provider and that application's ARN.
    let sources: Vec<String> = license
        .metrics_source(region)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|source| source.provider_arn)
        .collect();
    put_list(
        &mut row,
        "CurrentLicenseConfigurationMetricsSource",
        &sources,
    );
    if let Some(recommended) = license.recommended_license_edition() {
        put(&mut row, "RecommendationOptionsLicenseEdition", recommended);
        put(
            &mut row,
            "RecommendationOptionsLicenseModel",
            license.license_model.clone(),
        );
        put(
            &mut row,
            "RecommendationOptionsOperatingSystem",
            license.operating_system.clone(),
        );
    }
    if let Some(savings) = license.savings() {
        put_number(
            &mut row,
            "RecommendationOptionsSavingsOpportunityPercentage",
            savings.percentage,
        );
        put_number(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsValue",
            savings.monthly_amount,
        );
        put(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsCurrency",
            CURRENCY_USD,
        );
    }
    let tags: Vec<String> = license
        .tags
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    put_list(&mut row, "Tags", &tags);
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }
    row
}

/// One idle resource recommendation as an export row: the members
/// `IdleRecommendation` reports ([`IdleResourceAnalysis::to_recommendation`]), each computed
/// from the stored analysis the same way the reader computes it.
fn idle_row(
    analysis: &IdleResourceAnalysis,
    arn: &str,
    preferences: &EffectivePreferences,
    last_refresh: Option<f64>,
) -> ExportRow {
    let look_back_days = analysis.analysed_days(&preferences.look_back_period);
    let (percentage, _value) = analysis.savings();
    let mut row = ExportRow::new();
    put(&mut row, "AccountId", analysis.account_id.clone());
    put(&mut row, "ResourceArn", arn);
    put(&mut row, "ResourceId", analysis.resource_id());
    put(
        &mut row,
        "ResourceType",
        analysis.resource_type().unwrap_or_default(),
    );
    put(&mut row, "Finding", analysis.finding.clone());
    put(
        &mut row,
        "FindingDescription",
        analysis.finding_description(look_back_days),
    );
    put_number(&mut row, "LookbackPeriodInDays", look_back_days);
    // `SavingsOpportunity` / `SavingsOpportunityAfterDiscount` are one column each, and what
    // an `IdleSavingsOpportunity` is named for is its percentage: "the estimated monthly
    // savings possible as a percentage of monthly cost".
    put_number(&mut row, "SavingsOpportunity", percentage);
    put_number(&mut row, "SavingsOpportunityAfterDiscount", percentage);
    for metric in &analysis.utilization_metrics {
        let (Some(name), Some(statistic)) = (metric.name.as_deref(), metric.statistic.as_deref())
        else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field("UtilizationMetrics", name, statistic),
            metric.value,
        );
    }
    let tags: Vec<String> = analysis
        .tags
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    put_list(&mut row, "Tags", &tags);
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }
    row
}

/// Write the export objects of every job the clock has already made due, and start this
/// store's clock task for the job the calling export has just recorded: what every
/// `Export*Recommendations` operation does once it has recorded its job, so none of them
/// differs on it.
pub fn deliver_and_schedule<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, now: f64) {
    write_due_exports(&**engine, now);
    drive_export_clock(engine, 2 * super::EXPORT_JOB_STEP);
}

/// Write the export objects of every job the clock has made `Complete` and that has not been
/// written yet.
///
/// What the stored due instant has ALREADY made due, and nothing else: a job still `Queued`
/// or `InProgress` at `now` keeps its objects, so a client reading the destination before the
/// job completes finds nothing there. Writing is idempotent — the objects are TAKEN from the
/// job, so two callers racing write them once.
///
/// A destination that will not take an object (the bucket has been removed since the export
/// was created, say) FAILS the job: "The reason for an export job failure" is what
/// `failureReason` reports, and the job's status is then `Failed` rather than `Complete`.
/// When the object store cannot be reached at all nothing is taken, so the writes stay owed.
pub fn write_due_exports<Eng: Send + Sync + 'static>(engine: &Eng, now: f64) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let Some(objects) = s3::state::service_state(engine) else {
        ::tracing::warn!("the object store is not reachable from this engine, so no \
                          recommendations export was written");
        return;
    };
    // The due objects are taken out of this service's store and the lock released before the
    // object store is locked: the two locks are never held at once.
    let due: Vec<DueExport> = state.lock().export_jobs.take_due_exports(now);
    if due.is_empty() {
        return;
    }

    let mut failures: Vec<(String, String)> = Vec::new();
    {
        let mut store = objects.lock();
        for export in due {
            if let Some(reason) = write_export_objects(&mut store, &export) {
                failures.push((export.job_id, reason));
            }
        }
    }
    if !failures.is_empty() {
        let mut guard = state.lock();
        for (job_id, reason) in failures {
            guard.export_jobs.fail(&job_id, reason);
        }
    }
}

/// Write one job's two objects, answering the reason the destination did not take them.
///
/// Both objects are attempted whatever the other did, so a destination that took one of them
/// keeps it: an export file without its metadata, or the other way round, is what a client
/// reading the bucket then finds beside the `Failed` job the reason is recorded against. The
/// reason names the first object the destination refused.
fn write_export_objects(
    store: &mut s3::state::ComAmazonawsS3Amazons3State,
    export: &DueExport,
) -> Option<String> {
    let bucket = export.bucket.as_str();
    let mut refused: Option<String> = None;
    for (key, body, content_type) in [
        (
            export.key.as_str(),
            export.pending.export_file.as_str(),
            "text/csv",
        ),
        (
            export.metadata_key.as_str(),
            export.pending.metadata_file.as_str(),
            "application/json",
        ),
    ] {
        let object = s3::state::Object::stored(
            body.as_bytes().to_vec(),
            Some(content_type.to_string()),
        );
        let outcome = store.put_object(bucket, key, object);
        if let Some(reason) = refusal_reason(bucket, key, outcome) {
            refused = refused.or(Some(reason));
        }
    }
    refused
}

/// Why the destination did not take the object written to `key`, or `None` when it did.
///
/// Worded from the CONDITION the object store reported rather than from its own types: this
/// is what a client reads as the job's `failureReason`.
fn refusal_reason(
    bucket: &str,
    key: &str,
    outcome: Option<s3::state::ObjectWrite>,
) -> Option<String> {
    use s3::state::ObjectWrite;
    let condition = match outcome {
        Some(ObjectWrite::Created | ObjectWrite::Replaced) => return None,
        // The bucket existed when the export job was created and does not any more.
        None => format!("the destination Amazon S3 bucket {bucket} does not exist"),
        Some(ObjectWrite::KeyTooLong { bytes }) => {
            format!("the object key is {bytes} bytes long, which Amazon S3 does not accept")
        }
        Some(ObjectWrite::ObjectCountReached | ObjectWrite::TotalObjectCountReached) => {
            format!("the destination Amazon S3 bucket {bucket} cannot hold another object")
        }
        Some(
            ObjectWrite::ByteBudgetReached { .. } | ObjectWrite::TotalByteBudgetReached { .. },
        ) => format!(
            "the destination Amazon S3 bucket {bucket} cannot hold the bytes of the export"
        ),
        Some(ObjectWrite::MetadataTooLarge { bytes }) => {
            format!("the object's metadata is {bytes} bytes, which Amazon S3 does not accept")
        }
    };
    Some(format!(
        "Compute Optimizer could not write the recommendations export to {key} because \
         {condition}."
    ))
}

/// Start the one short in-process task that writes this job's objects when the clock makes
/// the job complete.
///
/// A client that exports and then downloads the file makes no further Compute Optimizer call,
/// so a write performed only by the next call could be a write never performed. The task does
/// not change WHEN the objects appear: it only calls [`write_due_exports`], which applies
/// what the stored due instant has already made due, so a client reading the destination
/// before that instant still finds nothing. With no async runtime to spawn onto there is no
/// task, and the objects are written by the next operation instead.
pub fn drive_export_clock<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, due_in: ::std::time::Duration) {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let engine = Arc::clone(engine);
    runtime.spawn(async move {
        ::tokio::time::sleep(due_in).await;
        write_due_exports(&*engine, super::now_secs());
    });
}
