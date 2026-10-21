// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! The exact NDJSON wire schema and batch builder.
//!
//! One batch = one `batch` envelope followed by aggregate `metric` rows, joined
//! by `\n`. Everything serialized here is either a validated newtype, a
//! closed enum, or a number/bool — the schema has no field that could carry a
//! request body, header, ARN, account id, or error message, and the
//! `closed_key_set` test pins the complete key vocabulary.

use crate::policy;
use crate::types::{
    AgentClient, Arch, BatchId, ContainerRuntime, CrashType, ErrorType, InstallChannel,
    InstallationId, LanguageVersion, MemoryBucket, MetricType, Namespace, OperationName, OsFamily,
    Outcome, ProductVersion, ProjectId, Region, RunMode, SdkDimension, SdkVersion, SessionId,
    StartupOutcome, TelemetryFeatureFlags,
};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// The session envelope row that leads every batch.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub record_type: &'static str,
    pub schema_version: &'static str,
    pub installation_id: InstallationId,
    pub session_id: SessionId,
    pub project_id: ProjectId,
    pub batch_id: BatchId,
    pub flush_sequence: u64,
    pub is_final: bool,
    /// Whole-second RFC3339 UTC timestamp; patched at send time.
    pub sent_at: String,
    pub session_duration_ms: u64,
    #[serde(flatten)]
    pub version: ProductVersionFields,
    pub environment: EnvironmentBlock,
    pub performance: PerformanceBlock,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crash: Option<CrashBlock>,
}

/// The product version fields, flattened into the session row.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductVersionFields {
    pub product_version: ProductVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_version: Option<ProductVersion>,
    pub first_run: bool,
}

/// The `environment` block of the session row. Every field is a closed enum
/// or a validated domain newtype — a coarse, non-identifying dimension.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentBlock {
    pub os: OsFamily,
    pub arch: Arch,
    pub sdk_name: SdkDimension,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdk_version: Option<SdkVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_version: Option<LanguageVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_client: Option<AgentClient>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    pub run_mode: RunMode,
    pub install_channel: InstallChannel,
    pub is_ci: bool,
    pub container_runtime: ContainerRuntime,
    pub feature_flags: TelemetryFeatureFlags,
}

/// The `performance` block of the session row.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceBlock {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_usage: Option<MemoryBucket>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throughput: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_concurrency: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cold_start_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_outcome: Option<StartupOutcome>,
}

/// The `crash` block: crash *type* only, by design — never a location,
/// message, or backtrace.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashBlock {
    pub crash_type: CrashType,
}

/// One aggregate metric row (`recordType: "metric"`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricRow {
    pub record_type: &'static str,
    #[serde(rename = "type")]
    pub metric_type: MetricType,
    pub namespace: Namespace,
    pub operation: OperationName,
    pub count: u64,
    pub outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_type: Option<ErrorType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency: Option<LatencyBlock>,
}

/// Aggregate latency for a metric row; `count` on the row is the denominator.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyBlock {
    pub min: u64,
    pub max: u64,
    pub sum_millis: u64,
}

impl LatencyBlock {
    /// Fold one observation into the aggregate.
    pub fn observe(&mut self, millis: u64, first: bool) {
        if first {
            self.min = millis;
            self.max = millis;
        } else {
            self.min = self.min.min(millis);
            self.max = self.max.max(millis);
        }
        self.sum_millis = self.sum_millis.saturating_add(millis);
    }
}

/// The aggregation key for one metric row.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MetricKey {
    pub metric_type: MetricType,
    pub namespace: Namespace,
    pub operation: OperationName,
    pub outcome: Outcome,
    pub error_type: Option<ErrorType>,
}

/// The aggregate value for one metric row.
#[derive(Debug, Clone, Copy, Default)]
pub struct MetricValue {
    pub count: u64,
    pub latency: Option<LatencyBlock>,
}

impl MetricValue {
    /// Fold one observation (with optional latency) into the aggregate.
    pub fn observe(&mut self, latency_millis: Option<u64>) {
        if let Some(millis) = latency_millis {
            let mut block = self.latency.unwrap_or_default();
            block.observe(millis, self.count == 0 || self.latency.is_none());
            self.latency = Some(block);
        }
        self.count = self.count.saturating_add(1);
    }
}

/// Everything needed to render session rows, fixed for the process lifetime.
#[derive(Debug, Clone)]
pub struct SessionContext {
    pub installation_id: InstallationId,
    pub session_id: SessionId,
    pub project_id: ProjectId,
    pub version: ProductVersionFields,
    pub environment: EnvironmentBlock,
}

/// A built, ready-to-enqueue NDJSON batch.
#[derive(Debug, Clone)]
pub struct BuiltBatch {
    pub batch_id: BatchId,
    pub body: String,
}

/// Current time as whole epoch seconds — the only clock resolution `sentAt`
/// may carry.
pub fn epoch_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn render_row<T: serde::Serialize>(row: &T) -> Option<String> {
    serde_json::to_string(row).ok()
}

/// Everything one flush renders into NDJSON batches.
#[derive(Debug)]
pub struct BatchPayload<'a> {
    pub context: &'a SessionContext,
    pub metrics: &'a BTreeMap<MetricKey, MetricValue>,
    pub flush_sequence: u64,
    pub is_final: bool,
    pub session_duration_ms: u64,
    pub performance: PerformanceBlock,
    pub crash: Option<CrashBlock>,
}

/// The server-directed batch caps (the [`crate::policy`] fallbacks apply
/// when no server config is available).
#[derive(Debug, Clone, Copy)]
pub struct BatchLimits {
    /// NDJSON lines per batch, session row included.
    pub max_rows_per_batch: usize,
    /// Bytes of the joined body.
    pub max_body_bytes: usize,
}

/// Build NDJSON batches from the session context plus aggregated metrics,
/// honoring both limits exactly.
///
/// Splitting: metric rows are distributed greedily; each batch repeats the
/// session row (with its own fresh `BatchId`) so every batch is
/// self-describing. A metric row that cannot fit in a batch even alone is
/// dropped — it cannot be split further. Only the last batch of a final flush
/// carries `isFinal: true`.
pub fn build_batches(
    payload: &BatchPayload<'_>,
    limits: BatchLimits,
    new_batch_id: &mut dyn FnMut() -> BatchId,
) -> Vec<BuiltBatch> {
    let max_rows = limits.max_rows_per_batch.max(policy::MIN_ROWS_PER_BATCH);
    let max_bytes = limits.max_body_bytes;
    let context = payload.context;

    let session_row = |batch_id: BatchId, is_final: bool| SessionRow {
        record_type: policy::RECORD_TYPE_BATCH,
        schema_version: policy::SCHEMA_VERSION,
        installation_id: context.installation_id.clone(),
        session_id: context.session_id.clone(),
        project_id: context.project_id.clone(),
        batch_id,
        flush_sequence: payload.flush_sequence,
        is_final,
        sent_at: format_sent_at(epoch_seconds_now()),
        session_duration_ms: payload.session_duration_ms,
        version: context.version.clone(),
        environment: context.environment.clone(),
        performance: payload.performance.clone(),
        crash: payload.crash.clone(),
    };

    let metric_lines: Vec<String> = payload
        .metrics
        .iter()
        .filter_map(|(key, value)| {
            render_row(&MetricRow {
                record_type: policy::RECORD_TYPE_METRIC,
                metric_type: key.metric_type,
                namespace: key.namespace.clone(),
                operation: key.operation.clone(),
                count: value.count,
                outcome: key.outcome,
                error_type: key.error_type,
                latency: value.latency,
            })
        })
        .collect();

    let mut batches = Vec::new();
    let mut remaining = metric_lines.as_slice();
    loop {
        let batch_id = new_batch_id();
        let Some(session_line) = render_row(&session_row(batch_id.clone(), false)) else {
            break;
        };
        let mut body = session_line;
        let mut rows = 1usize;
        let mut taken = 0usize;
        while taken < remaining.len() && rows < max_rows {
            let line = &remaining[taken];
            if body.len() + 1 + line.len() > max_bytes {
                break;
            }
            body.push('\n');
            body.push_str(line);
            rows += 1;
            taken += 1;
        }
        if taken == 0 && !remaining.is_empty() {
            // The widest pending metric row cannot fit alongside the session
            // row even alone and cannot be split further: drop it rather
            // than wedge the flush, keeping at most one session-only
            // heartbeat when nothing else could be built.
            remaining = &remaining[1..];
            if remaining.is_empty() {
                if batches.is_empty() && body.len() <= max_bytes {
                    batches.push(BuiltBatch { batch_id, body });
                }
                break;
            }
            continue;
        }
        remaining = &remaining[taken..];
        if body.len() <= max_bytes {
            batches.push(BuiltBatch { batch_id, body });
        }
        if remaining.is_empty() {
            break;
        }
    }
    if payload.is_final {
        // `true` renders shorter than `false`, so the byte cap still holds.
        if let Some(last) = batches.last_mut() {
            if let Some(line) = render_row(&session_row(last.batch_id.clone(), true)) {
                let session_end = last.body.find('\n').unwrap_or(last.body.len());
                last.body.replace_range(..session_end, &line);
            }
        }
    }
    batches
}

/// Split a stored batch body into two halves for the backend's
/// 400-split-the-batch contract. Each half repeats the original session row
/// under a fresh `BatchId`. Returns `None` when the body has at most one
/// metric row (nothing left to split — the batch must be dropped).
pub fn split_batch_body(
    body: &str,
    new_batch_id: &mut dyn FnMut() -> BatchId,
) -> Option<[BuiltBatch; 2]> {
    let lines: Vec<&str> = body.lines().collect();
    let (session_line, metric_lines) = lines.split_first()?;
    if metric_lines.len() < 2 {
        return None;
    }
    let mid = metric_lines.len() / 2;
    let mut halves = Vec::with_capacity(2);
    for chunk in [&metric_lines[..mid], &metric_lines[mid..]] {
        let batch_id = new_batch_id();
        let mut line = rekey_session_line(session_line, &batch_id)?;
        for metric in chunk {
            line.push('\n');
            line.push_str(metric);
        }
        halves.push(BuiltBatch {
            batch_id,
            body: line,
        });
    }
    let [a, b] = <[BuiltBatch; 2]>::try_from(halves).ok()?;
    Some([a, b])
}

fn rekey_session_line(session_line: &str, batch_id: &BatchId) -> Option<String> {
    let mut row: serde_json::Value = serde_json::from_str(session_line).ok()?;
    row.as_object_mut()?
        .insert(policy::KEY_BATCH_ID.to_string(), batch_id.as_str().into());
    serde_json::to_string(&row).ok()
}

fn format_sent_at(epoch_seconds: u64) -> String {
    let timestamp = i64::try_from(epoch_seconds)
        .ok()
        .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    timestamp
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Patch the session row's `sentAt` to `now` as a whole-second RFC3339 UTC
/// timestamp, returning the body to POST. Retried batches therefore always
/// carry their real send time. `None` when the stored body is not a batch
/// this crate built — its session row is not a JSON object or names a
/// different `batchId` than the outbox row it was loaded from — so a
/// corrupted or tampered row is dropped rather than transmitted.
pub fn patch_sent_at(body: &str, batch_id: &BatchId, now_epoch_seconds: u64) -> Option<String> {
    let mut lines = body.lines();
    let session_line = lines.next()?;
    let mut row: serde_json::Value = serde_json::from_str(session_line).ok()?;
    let fields = row.as_object_mut()?;
    if fields
        .get(policy::KEY_BATCH_ID)
        .and_then(serde_json::Value::as_str)
        != Some(batch_id.as_str())
    {
        return None;
    }
    fields.insert(
        policy::KEY_SENT_AT.to_string(),
        format_sent_at(now_epoch_seconds).into(),
    );
    let first = serde_json::to_string(&row).ok()?;
    Some(
        std::iter::once(first.as_str())
            .chain(lines)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{ALLOWED_WIRE_KEYS, FORBIDDEN_WIRE_KEY_FRAGMENTS};
    use crate::types::SdkName;

    pub(crate) fn test_context() -> SessionContext {
        SessionContext {
            installation_id: InstallationId::new("11111111-2222-3333-4444-555555555555").unwrap(),
            session_id: SessionId::new("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap(),
            project_id: ProjectId::new("proj-0123456789abcdef0123456789abcdef").unwrap(),
            version: ProductVersionFields {
                product_version: ProductVersion::new("1.0").unwrap(),
                previous_version: Some(ProductVersion::new("0.9").unwrap()),
                first_run: true,
            },
            environment: EnvironmentBlock {
                os: OsFamily::Linux,
                arch: Arch::Amd64,
                sdk_name: SdkDimension::Sdk(SdkName::AwsSdkRust),
                sdk_version: Some(SdkVersion::new("1.0.0").unwrap()),
                language_version: Some(LanguageVersion::new("1.85.0").unwrap()),
                agent_client: Some(AgentClient::Other),
                region: Some(Region::new("us-west-2").unwrap()),
                run_mode: RunMode::Standalone,
                install_channel: InstallChannel::Source,
                is_ci: false,
                container_runtime: ContainerRuntime::None,
                feature_flags: TelemetryFeatureFlags {
                    persistence: true,
                    multi_region: false,
                    auth_emulation: true,
                    strict_mode: false,
                    request_logging: true,
                    fidelity_gating: true,
                },
            },
        }
    }

    fn test_batch_id(n: u32) -> BatchId {
        BatchId::new(format!("00000000-0000-4000-8000-{n:012}")).unwrap()
    }

    fn test_performance() -> PerformanceBlock {
        PerformanceBlock {
            memory_usage: Some(MemoryBucket::LessThan256Mib),
            throughput: Some(2),
            peak_concurrency: Some(3),
            cold_start_time: Some(4),
            startup_outcome: Some(StartupOutcome::Success),
        }
    }

    fn sample_metrics(rows: usize) -> BTreeMap<MetricKey, MetricValue> {
        let mut metrics = BTreeMap::new();
        for i in 0..rows {
            let mut value = MetricValue::default();
            value.observe(Some(5 + i as u64));
            value.observe(Some(15 + i as u64));
            metrics.insert(
                MetricKey {
                    metric_type: MetricType::Operation,
                    namespace: Namespace::new("sqs").unwrap(),
                    operation: OperationName::new(format!("Operation{i:04}")).unwrap(),
                    outcome: Outcome::Success,
                    error_type: None,
                },
                value,
            );
        }
        metrics
    }

    fn build(
        metrics: &BTreeMap<MetricKey, MetricValue>,
        max_rows: usize,
        max_bytes: usize,
    ) -> Vec<BuiltBatch> {
        build_flush(metrics, false, max_rows, max_bytes)
    }

    fn build_flush(
        metrics: &BTreeMap<MetricKey, MetricValue>,
        is_final: bool,
        max_rows: usize,
        max_bytes: usize,
    ) -> Vec<BuiltBatch> {
        let mut n = 0u32;
        build_batches(
            &BatchPayload {
                context: &test_context(),
                metrics,
                flush_sequence: 1,
                is_final,
                session_duration_ms: 1234,
                performance: test_performance(),
                crash: None,
            },
            BatchLimits {
                max_rows_per_batch: max_rows,
                max_body_bytes: max_bytes,
            },
            &mut || {
                n += 1;
                test_batch_id(n)
            },
        )
    }

    fn collect_keys(value: &serde_json::Value, keys: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    keys.push(k.clone());
                    collect_keys(v, keys);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    collect_keys(item, keys);
                }
            }
            _ => {}
        }
    }

    /// The complete wire output uses ONLY the pinned closed key set, and no
    /// key smells like a payload/identity carrier. This is the schema-level
    /// no-PII guard: adding any field to the wire requires editing the pinned
    /// allowlist here, in the same diff a reviewer sees the field definition.
    #[test]
    fn wire_schema_carries_no_forbidden_fields() {
        let mut metrics = sample_metrics(3);
        metrics.insert(
            MetricKey {
                metric_type: MetricType::Error,
                namespace: Namespace::new("dynamodb").unwrap(),
                operation: OperationName::new("Query").unwrap(),
                outcome: Outcome::ClientError,
                error_type: Some(ErrorType::Throttling),
            },
            MetricValue {
                count: 2,
                latency: None,
            },
        );
        let mut n = 0u32;
        let batches = build_batches(
            &BatchPayload {
                context: &test_context(),
                metrics: &metrics,
                flush_sequence: 2,
                is_final: true,
                session_duration_ms: 99,
                performance: test_performance(),
                crash: Some(CrashBlock {
                    crash_type: CrashType::Panic,
                }),
            },
            BatchLimits {
                max_rows_per_batch: 1000,
                max_body_bytes: 262_144,
            },
            &mut || {
                n += 1;
                test_batch_id(n)
            },
        );
        assert!(!batches.is_empty());

        let mut keys = Vec::new();
        for batch in &batches {
            for line in batch.body.lines() {
                let row: serde_json::Value = serde_json::from_str(line).unwrap();
                collect_keys(&row, &mut keys);
            }
        }
        assert!(!keys.is_empty());
        let actual: std::collections::BTreeSet<&str> = keys.iter().map(String::as_str).collect();
        let expected: std::collections::BTreeSet<&str> =
            ALLOWED_WIRE_KEYS.iter().copied().collect();
        assert_eq!(
            actual, expected,
            "the exercised wire schema must exactly equal the reviewed allowlist"
        );
        for key in &keys {
            let lower = key.to_ascii_lowercase();
            for forbidden in FORBIDDEN_WIRE_KEY_FRAGMENTS {
                assert!(
                    !lower.contains(forbidden),
                    "wire key {key:?} matches forbidden fragment {forbidden:?}"
                );
            }
        }
    }

    #[test]
    fn every_batch_leads_with_a_session_row_and_respects_the_row_cap() {
        let batches = build(&sample_metrics(10), 4, 262_144);
        assert_eq!(batches.len(), 4, "10 rows at 3 metric rows per batch");
        for batch in &batches {
            let lines: Vec<&str> = batch.body.lines().collect();
            assert!(lines.len() <= 4);
            let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
            assert_eq!(first["recordType"], "batch");
            assert_eq!(first["batchId"], batch.batch_id.as_str());
            for line in &lines[1..] {
                let row: serde_json::Value = serde_json::from_str(line).unwrap();
                assert_eq!(row["recordType"], "metric");
            }
        }
        let total_metric_rows: usize = batches.iter().map(|b| b.body.lines().count() - 1).sum();
        assert_eq!(total_metric_rows, 10, "no metric row is lost by splitting");
    }

    #[test]
    fn only_the_last_batch_of_a_split_final_flush_is_final() {
        let batches = build_flush(&sample_metrics(10), true, 4, 262_144);
        assert_eq!(batches.len(), 4);
        let finals: Vec<bool> = batches
            .iter()
            .map(|batch| {
                let session: serde_json::Value =
                    serde_json::from_str(batch.body.lines().next().unwrap()).unwrap();
                assert_eq!(session["batchId"], batch.batch_id.as_str());
                session["isFinal"].as_bool().unwrap()
            })
            .collect();
        assert_eq!(finals, [false, false, false, true]);
        let total_metric_rows: usize = batches.iter().map(|b| b.body.lines().count() - 1).sum();
        assert_eq!(
            total_metric_rows, 10,
            "marking final keeps every metric row"
        );
    }

    #[test]
    fn batches_split_on_the_byte_cap() {
        let metrics = sample_metrics(64);
        let unsplit = build(&metrics, 1000, 262_144);
        assert_eq!(unsplit.len(), 1);
        let body_len = unsplit[0].body.len();

        let cap = body_len / 3;
        let split = build(&metrics, 1000, cap);
        assert!(split.len() >= 3, "body of {body_len}B must split at {cap}B");
        for batch in &split {
            assert!(batch.body.len() <= cap, "each split batch fits the cap");
        }
        let total: usize = split.iter().map(|b| b.body.lines().count() - 1).sum();
        assert_eq!(total, 64);

        let ids: std::collections::BTreeSet<&str> =
            split.iter().map(|b| b.batch_id.as_str()).collect();
        assert_eq!(ids.len(), split.len(), "each split batch has a fresh id");
    }

    #[test]
    fn an_empty_flush_still_emits_the_session_heartbeat() {
        let batches = build(&BTreeMap::new(), 1000, 262_144);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].body.lines().count(), 1);
    }

    #[test]
    fn split_batch_body_rekeys_both_halves() {
        let batches = build(&sample_metrics(9), 1000, 262_144);
        let body = &batches[0].body;
        let mut n = 0u32;
        let halves = split_batch_body(body, &mut || {
            n += 1;
            BatchId::new(format!("00000000-0000-4000-9000-{n:012}")).unwrap()
        })
        .expect("9 metric rows are splittable");
        let sizes: Vec<usize> = halves.iter().map(|h| h.body.lines().count() - 1).collect();
        assert_eq!(sizes.iter().sum::<usize>(), 9);
        assert!(sizes.iter().all(|s| *s >= 4));
        for half in &halves {
            let first: serde_json::Value =
                serde_json::from_str(half.body.lines().next().unwrap()).unwrap();
            assert_eq!(first["recordType"], "batch");
            assert_eq!(first["batchId"], half.batch_id.as_str());
        }

        let single = build(&sample_metrics(1), 1000, 262_144);
        assert!(
            split_batch_body(&single[0].body, &mut BatchId::random).is_none(),
            "a one-metric batch cannot split further"
        );
    }

    #[test]
    fn sent_at_is_patched_to_whole_seconds_at_send_time() {
        let batches = build(&sample_metrics(2), 1000, 262_144);
        let patched = patch_sent_at(&batches[0].body, &batches[0].batch_id, 1_789_988_511).unwrap();
        let first: serde_json::Value =
            serde_json::from_str(patched.lines().next().unwrap()).unwrap();
        assert_eq!(first["sentAt"], "2026-09-21T11:01:51Z");
        assert_eq!(
            patched.lines().count(),
            batches[0].body.lines().count(),
            "patching must not add or drop rows"
        );
        assert!(first["sentAt"].is_string());
    }

    #[test]
    fn a_stored_body_that_is_not_this_crates_batch_is_never_patched_for_sending() {
        let batches = build(&sample_metrics(2), 1000, 262_144);
        let other_id = BatchId::new("99999999-9999-4999-8999-999999999999").unwrap();
        assert!(
            patch_sent_at(&batches[0].body, &other_id, 1_789_988_511).is_none(),
            "a body naming a different batchId than its outbox row is dropped"
        );
        for tampered in [
            "",
            "not json",
            "[]",
            "\"a string\"",
            "{\"recordType\":\"batch\"}",
        ] {
            assert!(
                patch_sent_at(tampered, &batches[0].batch_id, 1_789_988_511).is_none(),
                "{tampered:?} must not be sent"
            );
        }
    }

    #[test]
    fn oversized_single_rows_are_dropped_not_wedged() {
        let mut metrics = sample_metrics(2);
        metrics.insert(
            MetricKey {
                metric_type: MetricType::Operation,
                namespace: Namespace::new("s3").unwrap(),
                operation: OperationName::new(format!("P{}", "a".repeat(63))).unwrap(),
                outcome: Outcome::Success,
                error_type: None,
            },
            MetricValue {
                count: 1,
                latency: None,
            },
        );
        // Cap chosen so the session row fits but session+widest row does not.
        let session_only = build(&BTreeMap::new(), 1000, 262_144);
        let cap = session_only[0].body.len() + 60;
        let batches = build(&metrics, 1000, cap);
        for batch in &batches {
            assert!(batch.body.len() <= cap);
        }
    }
}
