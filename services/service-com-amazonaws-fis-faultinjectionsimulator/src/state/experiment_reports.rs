// SPDX-License-Identifier: CC0-1.0

//! The experiment REPORT a run leaves behind: where its object lives, the document that
//! is written there, and the generation record a later read reports.

use super::*;

/// The key the report of the experiment `experiment_id` is written under, inside the bucket
/// its report configuration names.
///
/// `ReportConfigurationS3OutputPrefix` is "the prefix of the S3 bucket where the experiment
/// report will be stored", so the key is that prefix, then the experiment the report is
/// about, then [`REPORT_OBJECT_NAME`] — which keeps the reports of two experiments sharing
/// one prefix apart. A prefix the caller wrote with leading or trailing slashes names the
/// same place as one without them, so they are trimmed rather than producing an empty key
/// segment.
pub fn report_object_key(prefix: Option<&str>, experiment_id: &str) -> String {
    let prefix = prefix.unwrap_or_default().trim_matches('/');
    if prefix.is_empty() {
        format!("{experiment_id}/{REPORT_OBJECT_NAME}")
    } else {
        format!("{prefix}/{experiment_id}/{REPORT_OBJECT_NAME}")
    }
}

/// The ARN of the report object `key` in `bucket` — `ExperimentReportS3Report$arn`, "the
/// Amazon Resource Name (ARN) of the generated report".
///
/// Formatted here rather than through `::engine::emulator_arn` because an Amazon S3
/// object's ARN carries NEITHER a region nor an account segment (the AWS Service
/// Authorization Reference gives it as
/// `arn:${Partition}:s3:::${BucketName}/${ObjectName}`), and that helper always
/// interpolates the emulator account.
pub fn report_object_arn(bucket: &str, key: &str) -> String {
    format!("arn:aws:s3:::{bucket}/{key}")
}

/// `at` as the ISO-8601 instant a report document states a time as, or `None` for a time
/// the record does not carry (an `endTime` is absent until the experiment ends).
pub(super) fn report_timestamp(at: Option<SystemTime>) -> Option<String> {
    at.and_then(|at| {
        ::aws_smithy_types::DateTime::from(at)
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
            .ok()
    })
}

/// A modelled timestamp the record already holds, as a report document states it. See
/// [`report_timestamp`], which does the same for a [`SystemTime`].
pub(super) fn report_datetime(at: Option<&::aws_smithy_types::DateTime>) -> Option<String> {
    at.and_then(|at| at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok())
}

/// The bytes of the experiment report for `record`, generated at `generated_at`, or the
/// serializer's own error when the document could not be rendered — which the caller turns
/// into a FAILED generation ([`REPORT_NOT_BUILT_CODE`]) rather than writing a report with
/// no bytes in it.
///
/// Every value in it is READ OFF the experiment's own persisted record — its identity and
/// execution role, the lifecycle its run walked and the state it ended in, the actions the
/// run carried out or skipped with the state each of them reports, what its targets
/// resolved to, its stop conditions, and the report configuration that asked for this
/// report. Nothing is synthesised: what this emulator does not hold does not appear, and
/// `notes` says so for the CloudWatch dashboards a report configuration may name, whose
/// snapshots and metric data are produced by no part of this emulator.
///
/// Every map in it is rendered in key order ([`ordered_entries`], the crate's one map
/// renderer) and `generated_at` is the instant the generation fell DUE rather than the one
/// somebody polled at, so the same record always renders the same bytes.
pub fn experiment_report_document(
    record: &ExperimentRecord,
    generated_at: SystemTime,
) -> Result<Vec<u8>, ::serde_json::Error> {
    let state = record.to_state();
    let actions: BTreeMap<&str, ::serde_json::Value> = record
        .actions
        .iter()
        .map(|(name, action)| {
            (
                name.as_str(),
                ::serde_json::json!({
                    "actionId": action.action_id,
                    "description": action.description,
                    "parameters": ordered_entries(action.parameters.as_ref()),
                    "targets": ordered_entries(action.targets.as_ref()),
                    "startAfter": action.start_after,
                    "state": action.state.as_ref().map(|state| ::serde_json::json!({
                        "status": state.status,
                        "reason": state.reason,
                    })),
                    "startTime": report_datetime(action.start_time.as_ref()),
                    "endTime": report_datetime(action.end_time.as_ref()),
                }),
            )
        })
        .collect();
    let targets: BTreeMap<&str, ::serde_json::Value> = record
        .targets
        .iter()
        .map(|(name, target)| {
            (
                name.as_str(),
                ::serde_json::json!({
                    "resourceType": target.resource_type,
                    "resourceArns": target.resource_arns,
                    "resourceTags": ordered_entries(target.resource_tags.as_ref()),
                    "selectionMode": target.selection_mode,
                    "parameters": ordered_entries(target.parameters.as_ref()),
                    "filters": target.filters.as_ref().map(|filters| filters
                        .iter()
                        .map(|filter| ::serde_json::json!({
                            "path": filter.path,
                            "values": filter.values,
                        }))
                        .collect::<Vec<_>>()),
                }),
            )
        })
        .collect();
    let resolved_targets: Vec<::serde_json::Value> = record
        .resolved_targets
        .iter()
        .map(|resolved| {
            ::serde_json::json!({
                "targetName": resolved.target_name,
                "resourceType": resolved.resource_type,
                "targetInformation": ordered_entries(resolved.target_information.as_ref()),
            })
        })
        .collect();
    let stop_conditions: Vec<::serde_json::Value> = record
        .stop_conditions
        .iter()
        .map(|condition| {
            ::serde_json::json!({ "source": condition.source, "value": condition.value })
        })
        .collect();
    let configuration = record.experiment_report_configuration.as_ref();
    let dashboards: Vec<::serde_json::Value> = configuration
        .and_then(|configuration| configuration.data_sources.as_ref())
        .and_then(|sources| sources.cloud_watch_dashboards.as_ref())
        .map(|dashboards| {
            dashboards
                .iter()
                .map(|dashboard| {
                    ::serde_json::json!({ "dashboardIdentifier": dashboard.dashboard_identifier })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut notes = vec![
        "This report is built from the experiment's own record: the lifecycle its run \
         walked, the actions the run carried out or skipped, and the resources its targets \
         resolved to.",
    ];
    if !dashboards.is_empty() {
        notes.push(
            "The CloudWatch dashboards the report configuration names are listed under \
             reportConfiguration.dataSources; no dashboard snapshot and no metric data is \
             included, because this emulator captures none.",
        );
    }
    let document = ::serde_json::json!({
        "generatedAt": report_timestamp(Some(generated_at)),
        "experiment": {
            "id": record.id,
            "arn": record.arn,
            "experimentTemplateId": record.experiment_template_id,
            "roleArn": record.role_arn,
            "state": {
                "status": state.status,
                "reason": state.reason,
                "error": state.error.as_ref().map(|error| ::serde_json::json!({
                    "accountId": error.account_id,
                    "code": error.code,
                    "location": error.location,
                })),
            },
            "creationTime": report_timestamp(Some(record.creation_time)),
            "startTime": report_timestamp(Some(record.start_time)),
            "endTime": report_timestamp(record.end_time),
            "experimentOptions": {
                "accountTargeting": record.experiment_options.account_targeting,
                "emptyTargetResolutionMode": record.experiment_options.empty_target_resolution_mode,
                "actionsMode": record.experiment_options.actions_mode,
            },
            "tags": ordered_entries(Some(&record.tags)),
        },
        "actions": actions,
        "targets": targets,
        "resolvedTargets": resolved_targets,
        "stopConditions": stop_conditions,
        "reportConfiguration": {
            "preExperimentDuration": configuration
                .and_then(|configuration| configuration.pre_experiment_duration.clone()),
            "postExperimentDuration": configuration
                .and_then(|configuration| configuration.post_experiment_duration.clone()),
            "dataSources": { "cloudWatchDashboards": dashboards },
        },
        "notes": notes,
    });
    ::serde_json::to_vec_pretty(&document)
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service: it
/// routes in process, so no network client is configured here. `region` is the request's
/// own signing region, so a report is written where the caller's own S3 requests go.
pub(super) fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Materialise the report generation the CLOCK owes the experiment `id` at `now`: write the
/// report to the Amazon S3 output its configuration names, and record on the experiment
/// where it went.
///
/// The SECOND write a reporting operation performs, beside
/// [`ExperimentRecord::settle`], and owed on the same terms: FIS generates the experiment
/// report once the experiment has ENDED, so the generation was due at `now` whether or not
/// anybody called, and two reads at the same instant report the same report state. An
/// experiment that configured no report owes nothing and nothing is written for it.
///
/// Takes the store lock TWICE and holds it across neither await: once to settle the clock
/// and claim the generation ([`ExperimentRecord::claim_report_generation`], which also
/// builds the document), and once to record what the write did. A generation that has
/// reached its own terminal state, or one claimed within [`REPORT_CLAIM_GRACE`], is not
/// claimed again, so the report object is written once.
pub async fn materialise_experiment_report<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    id: &str,
    now: SystemTime,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let generation = {
        let breaches = stop_conditions::AlarmBreaches::read(engine);
        let mut store = state.lock();
        let Some(record) = store.settled_experiment_mut(id, now, &breaches) else {
            return;
        };
        match record.claim_report_generation(now) {
            Some(generation) => generation,
            None => return,
        }
    };

    let outcome = write_report_generation(ctx, region, generation).await;
    record_report_generation(&state, id, outcome);
}

/// Materialise the report generation the CLOCK owes EVERY experiment at `now` — what
/// [`materialise_experiment_report`] does for one, for an operation that settles the whole
/// store rather than a single record.
///
/// Settling is what MATERIALISES the end of a run, and the end is what makes the
/// generation due, so an operation that ends every elapsed run owes the second write for
/// each of them: otherwise a client polling that operation reads the terminal status while
/// the report of that same run goes unwritten until somebody reads the experiment itself,
/// and two callers at one instant disagree about whether the report exists.
///
/// Takes the store lock to settle and claim, releases it, and takes it again per write to
/// record what that write did — so no lock is held across a call. Claiming is what keeps
/// the report object written once ([`ExperimentRecord::claim_report_generation`]).
pub async fn materialise_experiment_reports<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    now: SystemTime,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let generations = {
        let breaches = stop_conditions::AlarmBreaches::read(engine);
        let mut store = state.lock();
        store.settle_experiments(now, &breaches);
        store.claim_report_generations(now)
    };
    for (id, generation) in generations {
        let outcome = write_report_generation(ctx, region, generation).await;
        record_report_generation(&state, &id, outcome);
    }
}

/// Carry out one CLAIMED generation: the ARN of the report object it wrote, or the error
/// code and the reason the generation failed with.
///
/// Takes no store lock at all — the document was already built from the record at the
/// instant the generation fell due — so the write happens with the store free.
pub(super) async fn write_report_generation(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    generation: ReportGeneration,
) -> Result<String, (&'static str, String)> {
    match generation {
        // Nowhere to write it: the configuration asked for a report without naming an
        // output, so the generation fails rather than reporting a report nothing holds.
        ReportGeneration::NoOutput => Err((
            REPORT_OUTPUT_NOT_CONFIGURED_CODE,
            "experimentReportConfiguration names no \
             outputs.s3Configuration.bucketName for the report to be written to."
                .to_string(),
        )),
        // No document to write: the same rule, for the report that could not be built.
        ReportGeneration::NotBuilt(reason) => Err((REPORT_NOT_BUILT_CODE, reason)),
        ReportGeneration::Write {
            bucket,
            key,
            document,
        } => match s3_client(ctx, region)
            .put_object()
            .bucket(&bucket)
            .key(&key)
            .body(::aws_sdk_s3::primitives::ByteStream::from(document))
            .content_type(REPORT_CONTENT_TYPE)
            .send()
            .await
        {
            Ok(_) => Ok(report_object_arn(&bucket, &key)),
            Err(error) => Err((
                REPORT_OUTPUT_NOT_WRITTEN_CODE,
                format!(
                    "Could not write the experiment report to s3://{bucket}/{key}: {}",
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error),
                ),
            )),
        },
    }
}

/// Record what the write of the experiment `id`'s report did on that experiment's own
/// generation record, so a later read reports the generation that happened.
pub(super) fn record_report_generation(
    state: &::engine::ServiceState<ComAmazonawsFisFaultinjectionsimulatorState>,
    id: &str,
    outcome: Result<String, (&'static str, String)>,
) {
    let mut store = state.lock();
    let Some(report) = store
        .experiments
        .get_mut(id)
        .and_then(|record| record.report.as_mut())
    else {
        return;
    };
    match outcome {
        Ok(arn) => report.written(arn),
        Err((code, reason)) => report.failed(code, reason),
    }
}
