// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Test-side mirror of the shipped backend ingest validator, driven entirely by
//! the vendored contract document [`backend_contract.json`](./backend_contract.json).
//!
//! The backend's ingest validator, and the limits, patterns and allowlists it
//! executes, are the source of truth. The backend publishes no
//! machine-readable export, so the JSON document is a HAND-SYNCED mirror of
//! those rules; `_vendoredFrom` records that. Nothing in this
//! module hand-copies a limit, an enum or a pattern: every rule is read from
//! the document, so a backend rule change arrives as a one-file sync whose
//! `contractVersion` pin and value assertions fail this build until the client
//! catches up.
//!
//! STRICTER THAN THE BACKEND ON PURPOSE. The shipped validator is tolerant at
//! its boundary: unknown fields are ignored, non-canonical `namespace` and
//! `errorType` spellings are FOLDED onto canonical values, and malformed
//! optional values are stored as `unknown` or dropped by projection. For the backend that
//! tolerance is availability; for OUR CLIENT it would hide bugs — a value the
//! backend folds or drops is telemetry we lost. So this validator errors on
//! everything the backend would not store exactly as sent: unknown keys,
//! non-canonical spellings, and out-of-range optional values are all failures
//! here, while genuinely optional fields (`environment`, `performance`,
//! `isFinal`, `sessionDurationMs`, `firstRun`, ...) may be absent
//! exactly as the backend permits.
//!
//! The document's regexes are enforced by hand-rolled matchers (this crate
//! deliberately has no regex dependency). Each matcher is derived from one
//! pattern literal, and `the_contract_patterns_match_their_implemented_matchers`
//! pins the document's pattern strings to those literals and exercises an
//! accept/reject corpus per matcher — so a pattern change in the synced JSON
//! fails the pin until the matcher is updated to match.

use crate::policy;
use crate::types::{
    AgentClient, Arch, BatchId, ContainerRuntime, CrashType, ErrorType, InstallChannel,
    InstallationId, LanguageVersion, MemoryBucket, MetricType, Namespace, OperationName, OsFamily,
    Outcome, ProductVersion, ProjectId, Region, RunMode, SdkDimension, SdkName, SdkVersion,
    SessionId, StartupOutcome, TelemetryFeatureFlags,
};
use crate::wire::{
    build_batches, epoch_seconds_now, patch_sent_at, BatchLimits, BatchPayload, CrashBlock,
    EnvironmentBlock, MetricKey, MetricValue, PerformanceBlock, ProductVersionFields,
    SessionContext,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

const CONTRACT_JSON: &str = include_str!("backend_contract.json");

/// The contract revision this client is written against. Bumped deliberately,
/// never as part of a routine JSON sync: a version bump in the synced document
/// fails `the_vendored_contract_version_is_pinned` until someone re-audits the
/// client against the new rules.
const EXPECTED_CONTRACT_VERSION: u64 = 1;

fn contract() -> &'static Value {
    static CONTRACT: OnceLock<Value> = OnceLock::new();
    CONTRACT.get_or_init(|| {
        serde_json::from_str(CONTRACT_JSON).expect("backend_contract.json must be valid JSON")
    })
}

fn node(path: &[&str]) -> &'static Value {
    let mut value = contract();
    for key in path {
        value = value
            .get(key)
            .unwrap_or_else(|| panic!("backend_contract.json is missing {path:?}"));
    }
    value
}

fn string(path: &[&str]) -> &'static str {
    node(path)
        .as_str()
        .unwrap_or_else(|| panic!("backend_contract.json {path:?} must be a string"))
}

fn integer(path: &[&str]) -> u64 {
    node(path)
        .as_u64()
        .unwrap_or_else(|| panic!("backend_contract.json {path:?} must be a nonnegative integer"))
}

fn float(path: &[&str]) -> f64 {
    node(path)
        .as_f64()
        .unwrap_or_else(|| panic!("backend_contract.json {path:?} must be a number"))
}

fn strings(path: &[&str]) -> Vec<&'static str> {
    node(path)
        .as_array()
        .unwrap_or_else(|| panic!("backend_contract.json {path:?} must be an array"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("backend_contract.json {path:?} must hold strings"))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Pattern matchers. Each implements exactly one pattern literal from the
// contract document; the pin test asserts the document still carries that
// literal and exercises an accept/reject corpus against the matcher.
// ---------------------------------------------------------------------------

const OPERATION_PATTERN_IMPLEMENTED: &str = "^(?=.*[a-z])[A-Z][A-Za-z0-9]{0,63}$";
const DOTTED_VERSION_PATTERN_IMPLEMENTED: &str = "^\\d+(?:\\.\\d+)*$";
const PROJECT_ID_PATTERN_IMPLEMENTED: &str = "^proj-[0-9a-f]{32}$";
const UUID_PATTERN_IMPLEMENTED: &str =
    "^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";
const PRODUCT_VERSION_PATTERN_IMPLEMENTED: &str =
    "^(0|[1-9]\\d*)\\.(0|[1-9]\\d*)(?:\\.(0|[1-9]\\d*))?(?:[-+][0-9A-Za-z.-]+)?$";
const REGION_PATTERN_IMPLEMENTED: &str = "^(?:unknown|[a-z]{2}(?:-gov|-iso[a-z]?)?-[a-z]+-\\d)$";
const WHOLE_SECOND_RFC3339_PATTERN_IMPLEMENTED: &str =
    "^\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}Z$";

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

fn matches_operation(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value.as_bytes()[0].is_ascii_uppercase()
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && value.bytes().any(|byte| byte.is_ascii_lowercase())
}

fn matches_dotted_version(value: &str) -> bool {
    value.split('.').all(|component| {
        !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn matches_project_id(value: &str) -> bool {
    value
        .strip_prefix("proj-")
        .is_some_and(|hex| hex.len() == 32 && hex.bytes().all(is_lower_hex))
}

fn matches_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let hyphen = matches!(index, 8 | 13 | 18 | 23);
        if hyphen != (*byte == b'-') || (!hyphen && !is_lower_hex(*byte)) {
            return false;
        }
    }
    matches!(bytes[14], b'1'..=b'8') && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
}

fn numeric_component(component: &str) -> bool {
    !component.is_empty()
        && component.bytes().all(|byte| byte.is_ascii_digit())
        && (component == "0" || !component.starts_with('0'))
}

fn matches_product_version(value: &str) -> bool {
    let (core, suffix) = match value.find(['-', '+']) {
        Some(at) => (&value[..at], Some(&value[at + 1..])),
        None => (value, None),
    };
    if suffix.is_some_and(|suffix| {
        suffix.is_empty()
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    }) {
        return false;
    }
    let mut components = core.split('.');
    let (major, minor, patch, extra) = (
        components.next(),
        components.next(),
        components.next(),
        components.next(),
    );
    if extra.is_some() {
        return false;
    }
    let (Some(major), Some(minor)) = (major, minor) else {
        return false;
    };
    numeric_component(major) && numeric_component(minor) && patch.is_none_or(numeric_component)
}

fn matches_region(value: &str) -> bool {
    fn area_and_digit(rest: &str) -> bool {
        let Some((area, digit)) = rest.rsplit_once('-') else {
            return false;
        };
        !area.is_empty()
            && area.bytes().all(|byte| byte.is_ascii_lowercase())
            && digit.len() == 1
            && digit.bytes().all(|byte| byte.is_ascii_digit())
    }
    if value == "unknown" {
        return true;
    }
    let bytes = value.as_bytes();
    if bytes.len() < 2 || !bytes[..2].iter().all(u8::is_ascii_lowercase) {
        return false;
    }
    let rest = &value[2..];
    // Mirrors the pattern's backtracking: every reading of the optional
    // `-gov` / `-iso[a-z]?` group must be tried (`us-gov-1` parses with `gov`
    // as the area and the optional group unmatched).
    let mut candidates = vec![rest];
    if let Some(after_gov) = rest.strip_prefix("-gov") {
        candidates.push(after_gov);
    }
    if let Some(after_iso) = rest.strip_prefix("-iso") {
        candidates.push(after_iso);
        let mut chars = after_iso.chars();
        if chars.next().is_some_and(|c| c.is_ascii_lowercase()) {
            candidates.push(chars.as_str());
        }
    }
    candidates
        .into_iter()
        .any(|candidate| candidate.strip_prefix('-').is_some_and(area_and_digit))
}

fn matches_whole_second_rfc3339(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 20
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            10 => *byte == b'T',
            13 | 16 => *byte == b':',
            19 => *byte == b'Z',
            _ => byte.is_ascii_digit(),
        })
}

// ---------------------------------------------------------------------------
// Validators.
// ---------------------------------------------------------------------------

fn object(value: &Value) -> Result<&Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| "expected JSON object".to_string())
}

fn nonempty_string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{key} must be a nonempty string"))
}

fn only_known_keys(
    map: &Map<String, Value>,
    required: &[&str],
    optional: &[&str],
    context: &str,
) -> Result<(), String> {
    for key in map.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            return Err(format!("{context} carries unknown field {key:?}"));
        }
    }
    for key in required {
        if !map.contains_key(*key) {
            return Err(format!("{context}.{key} is required"));
        }
    }
    Ok(())
}

fn optional_enum(
    object: &Map<String, Value>,
    key: &str,
    allowed: &[&str],
    context: &str,
) -> Result<(), String> {
    let Some(value) = object.get(key) else {
        return Ok(());
    };
    let value = value
        .as_str()
        .ok_or_else(|| format!("{context}.{key} must be a string"))?;
    if !allowed.contains(&value) {
        return Err(format!("{context}.{key} has unsupported value {value:?}"));
    }
    Ok(())
}

fn optional_bool(object: &Map<String, Value>, key: &str, context: &str) -> Result<(), String> {
    if object.get(key).is_some_and(|value| !value.is_boolean()) {
        return Err(format!("{context}.{key} must be boolean"));
    }
    Ok(())
}

fn optional_bounded_float(
    object: &Map<String, Value>,
    key: &str,
    maximum: f64,
    context: &str,
) -> Result<(), String> {
    let Some(value) = object.get(key) else {
        return Ok(());
    };
    if !value
        .as_f64()
        .is_some_and(|value| value >= 0.0 && value <= maximum)
    {
        return Err(format!("{context}.{key} must be a number in 0..={maximum}"));
    }
    Ok(())
}

fn valid_product_version(value: &str) -> bool {
    if value.len() > integer(&["identifiers", "maxProductVersionLength"]) as usize
        || !matches_product_version(value)
    {
        return false;
    }
    let core = value.find(['-', '+']).map_or(value, |at| &value[..at]);
    let maximum = integer(&["identifiers", "maxVersionComponent"]);
    core.split('.').take(2).all(|component| {
        component
            .parse::<u64>()
            .is_ok_and(|number| number <= maximum)
    })
}

fn validate_product_version_fields(envelope: &Map<String, Value>) -> Result<(), String> {
    for key in ["productVersion", "previousVersion"] {
        if envelope.contains_key(key) && !valid_product_version(nonempty_string(envelope, key)?) {
            return Err(format!("{key} is not an accepted product version"));
        }
    }
    optional_bool(envelope, "firstRun", "envelope")
}

fn validate_environment(value: &Value) -> Result<(), String> {
    let environment = object(value)?;
    only_known_keys(
        environment,
        &[],
        &strings(&["envelope", "environment", "optionalFields"]),
        "environment",
    )?;
    for (key, enum_name) in [
        ("os", "os"),
        ("arch", "arch"),
        ("sdkName", "sdkName"),
        ("runMode", "runMode"),
        ("installChannel", "installChannel"),
        ("containerRuntime", "containerRuntime"),
        ("agentClient", "agentClient"),
    ] {
        optional_enum(
            environment,
            key,
            &strings(&["enums", enum_name]),
            "environment",
        )?;
    }
    let max_version = integer(&["identifiers", "maxSdkOrLanguageVersionLength"]) as usize;
    for key in ["sdkVersion", "languageVersion"] {
        if environment.contains_key(key) {
            let value = nonempty_string(environment, key)?;
            if value.len() > max_version || !matches_dotted_version(value) {
                return Err(format!("environment.{key} is not a bounded version"));
            }
        }
    }
    if environment.contains_key("region")
        && !matches_region(nonempty_string(environment, "region")?)
    {
        return Err("environment.region is not an accepted region".to_string());
    }
    optional_bool(environment, "isCi", "environment")?;
    if let Some(feature_flags) = environment.get("featureFlags") {
        let feature_flags = object(feature_flags)?;
        if feature_flags.is_empty() {
            return Err("environment.featureFlags must not be empty when present".to_string());
        }
        let allowed = strings(&["featureFlags", "keys"]);
        for (key, value) in feature_flags {
            if !allowed.contains(&key.as_str()) || !value.is_boolean() {
                return Err(format!("environment.featureFlags.{key} is not allowlisted"));
            }
        }
    }
    Ok(())
}

fn validate_performance(value: &Value) -> Result<(), String> {
    let performance = object(value)?;
    only_known_keys(
        performance,
        &[],
        &strings(&["envelope", "performance", "optionalFields"]),
        "performance",
    )?;
    optional_enum(
        performance,
        "memoryUsage",
        &strings(&["enums", "memoryUsageBucket"]),
        "performance",
    )?;
    optional_enum(
        performance,
        "startupOutcome",
        &strings(&["enums", "startupOutcome"]),
        "performance",
    )?;
    optional_bounded_float(
        performance,
        "throughput",
        float(&["envelope", "performance", "maxThroughput"]),
        "performance",
    )?;
    optional_bounded_float(
        performance,
        "coldStartTime",
        float(&["envelope", "performance", "maxColdStartTimeMillis"]),
        "performance",
    )?;
    if performance.get("peakConcurrency").is_some_and(|value| {
        !value.as_u64().is_some_and(|value| {
            value <= integer(&["envelope", "performance", "maxPeakConcurrency"])
        })
    }) {
        return Err(
            "performance.peakConcurrency must be a bounded nonnegative integer".to_string(),
        );
    }
    Ok(())
}

fn validate_crash(value: &Value) -> Result<(), String> {
    let crash = object(value)?;
    only_known_keys(
        crash,
        &[],
        &strings(&["envelope", "crash", "optionalFields"]),
        "crash",
    )?;
    optional_enum(
        crash,
        "crashType",
        &strings(&["enums", "crashType"]),
        "crash",
    )
}

fn validate_envelope(value: &Value) -> Result<(), String> {
    let envelope = object(value)?;
    only_known_keys(
        envelope,
        &strings(&["envelope", "requiredFields"]),
        &strings(&["envelope", "optionalFields"]),
        "envelope",
    )?;
    if envelope.get("recordType").and_then(Value::as_str)
        != strings(&["enums", "recordType"]).first().copied()
    {
        return Err("recordType must be batch".to_string());
    }
    if nonempty_string(envelope, "schemaVersion")? != string(&["envelope", "schemaVersion"]) {
        return Err("schemaVersion is not the contract's schema version".to_string());
    }
    for key in ["installationId", "sessionId", "batchId"] {
        if !matches_uuid(nonempty_string(envelope, key)?) {
            return Err(format!("{key} must be a lowercase versioned UUID"));
        }
    }
    if !matches_project_id(nonempty_string(envelope, "projectId")?) {
        return Err("projectId violates the project id contract".to_string());
    }
    if !envelope.get("flushSequence").is_some_and(|value| {
        value
            .as_u64()
            .is_some_and(|n| n <= integer(&["maxSafeInteger"]))
    }) {
        return Err("flushSequence must be a nonnegative safe integer".to_string());
    }
    optional_bool(envelope, "isFinal", "envelope")?;
    if envelope.get("sessionDurationMs").is_some_and(|value| {
        !value
            .as_u64()
            .is_some_and(|n| n <= integer(&["envelope", "maxSessionDurationMs"]))
    }) {
        return Err("sessionDurationMs must be a bounded nonnegative integer".to_string());
    }
    let sent_at = nonempty_string(envelope, "sentAt")?;
    let parsed = matches_whole_second_rfc3339(sent_at)
        .then(|| OffsetDateTime::parse(sent_at, &Rfc3339).ok())
        .flatten()
        .ok_or_else(|| "sentAt must be whole-second UTC RFC3339".to_string())?;
    let normalized = parsed
        .format(&Rfc3339)
        .map_err(|error| format!("sentAt cannot be normalized: {error}"))?;
    if normalized != sent_at {
        return Err("sentAt must be whole-second UTC RFC3339".to_string());
    }
    validate_product_version_fields(envelope)?;
    if let Some(environment) = envelope.get("environment") {
        validate_environment(environment)?;
    }
    if let Some(performance) = envelope.get("performance") {
        validate_performance(performance)?;
    }
    if let Some(crash) = envelope.get("crash") {
        validate_crash(crash)?;
    }
    Ok(())
}

fn validate_latency(value: &Value, count: u64) -> Result<(), String> {
    let latency = object(value)?;
    let fields = strings(&["metricRow", "latency", "fields"]);
    only_known_keys(latency, &fields, &[], "latency")?;
    let read = |key: &str| {
        latency
            .get(key)
            .and_then(Value::as_f64)
            .filter(|value| *value >= 0.0)
            .ok_or_else(|| format!("latency.{key} must be nonnegative"))
    };
    let (min, max, sum) = (read("min")?, read("max")?, read("sumMillis")?);
    if max < min || (sum > 0.0 && count == 0) {
        return Err("latency bounds are inconsistent".to_string());
    }
    let count = count as f64;
    let (lower, upper) = (min * count, max * count);
    if !lower.is_finite() || !upper.is_finite() || upper > integer(&["maxSafeInteger"]) as f64 {
        return Err("latency bounds overflowed the safe integer range".to_string());
    }
    let tolerance = float(&["metricRow", "latency", "boundTolerance"])
        * 1.0_f64.max(lower.abs()).max(upper.abs());
    if count > 0.0 && (sum < lower - tolerance || sum > upper + tolerance) {
        return Err("latency sum is outside its possible bounds".to_string());
    }
    Ok(())
}

fn validate_row(value: &Value) -> Result<(), String> {
    let row = object(value)?;
    only_known_keys(
        row,
        &strings(&["metricRow", "requiredFields"]),
        &strings(&["metricRow", "optionalFields"]),
        "metric row",
    )?;
    if row.get("recordType").and_then(Value::as_str)
        != strings(&["enums", "recordType"]).get(1).copied()
    {
        return Err("metric recordType is invalid".to_string());
    }
    let metric_types = strings(&["enums", "metricType"]);
    if !row
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| metric_types.contains(&value))
    {
        return Err("metric type is not in the contract's row types".to_string());
    }
    let namespace = nonempty_string(row, "namespace")?;
    if namespace.len() > integer(&["namespace", "maxLength"]) as usize
        || namespace.trim() != namespace
    {
        return Err("metric namespace is not a trimmed bounded string".to_string());
    }
    if let Some(canonical) = strings(&["namespace", "knownValues"])
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(namespace))
    {
        if *canonical != namespace {
            return Err(format!(
                "metric namespace {namespace:?} must use its canonical spelling {canonical:?}"
            ));
        }
    }
    let operation = nonempty_string(row, "operation")?;
    if operation != string(&["operation", "foldedValue"])
        && (operation.len() > integer(&["operation", "maxLength"]) as usize
            || !matches_operation(operation))
    {
        return Err("metric operation violates the backend contract".to_string());
    }
    let outcomes = strings(&["enums", "outcome"]);
    if !row
        .get("outcome")
        .and_then(Value::as_str)
        .is_some_and(|value| outcomes.contains(&value))
    {
        return Err("metric outcome is not in the contract's outcome states".to_string());
    }
    let count = row
        .get("count")
        .and_then(Value::as_u64)
        .filter(|value| *value <= integer(&["maxSafeInteger"]))
        .ok_or_else(|| "metric count must be a nonnegative safe integer".to_string())?;
    if let Some(error_type) = row.get("errorType") {
        let error_type = error_type
            .as_str()
            .ok_or_else(|| "metric errorType must be a string".to_string())?;
        // The backend FOLDS non-canonical spellings; the client must never rely
        // on that, so only exact taxonomy entries and the fold target pass here.
        if !strings(&["errorType", "knownValues"]).contains(&error_type)
            && error_type != string(&["errorType", "foldedValue"])
        {
            return Err(format!(
                "metric errorType {error_type:?} is not a canonical taxonomy entry"
            ));
        }
    }
    if let Some(latency) = row.get("latency") {
        validate_latency(latency, count)?;
    }
    Ok(())
}

/// Validate one captured ingest body end to end: the envelope line followed
/// by zero or more metric rows, every line contract-valid. Shared with the
/// integration property tests so a live client's wire output is held to the
/// same rules as the unit-built batches here.
pub(crate) fn validate_ingest_body(body: &str) -> Result<(), String> {
    let mut lines = body.lines();
    let envelope = lines
        .next()
        .ok_or_else(|| "an ingest body must start with an envelope line".to_string())?;
    let envelope: Value = serde_json::from_str(envelope)
        .map_err(|error| format!("envelope line is not JSON: {error}"))?;
    validate_envelope(&envelope)?;
    for line in lines {
        let row: Value =
            serde_json::from_str(line).map_err(|error| format!("row line is not JSON: {error}"))?;
        validate_row(&row)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[test]
fn the_vendored_contract_version_is_pinned() {
    assert_eq!(
        integer(&["contractVersion"]),
        EXPECTED_CONTRACT_VERSION,
        "backend contract version changed: re-audit the client against the new rules \
         before bumping EXPECTED_CONTRACT_VERSION"
    );
}

#[test]
fn the_vendored_contract_is_internally_consistent() {
    let provenance = string(&["_vendoredFrom"]);
    assert!(
        provenance.contains("hand-synced"),
        "_vendoredFrom must record that this document is a hand-synced mirror"
    );
    let canonical_uuid = "6f3a1b2c-9d4e-4f5a-8b6c-7d8e9f0a1b2c";
    assert!(matches_uuid(canonical_uuid));
    assert_eq!(
        integer(&["identifiers", "maxIdentifierLength"]) as usize,
        canonical_uuid.len(),
        "the backend's identifier bound is exactly the UUID length"
    );
    for folded in [
        "memoryUsageBucket",
        "startupOutcome",
        "runMode",
        "crashType",
    ] {
        assert!(
            strings(&["enums", folded]).contains(&"unknown"),
            "the backend folds out-of-list enums.{folded} values to unknown and stores it"
        );
    }
    assert!(
        strings(&["envelope", "optionalFields"]).contains(&"productVersion")
            && !strings(&["envelope", "requiredFields"]).contains(&"productVersion"),
        "the backend keeps productVersion optional"
    );
    assert_eq!(integer(&["maxSafeInteger"]), (1_u64 << 53) - 1);
    assert_eq!(
        integer(&["transport", "maxBodyBytes"]),
        integer(&["clientConfigLimits", "maxBodyBytes", "max"])
    );
    for limit in [
        "flushIntervalSeconds",
        "maxRowsPerBatch",
        "maxBodyBytes",
        "samplingRate",
        "configTtlSeconds",
    ] {
        assert!(
            float(&["clientConfigLimits", limit, "min"])
                <= float(&["clientConfigLimits", limit, "max"]),
            "clientConfigLimits.{limit} bounds are inverted"
        );
    }
    assert_eq!(
        strings(&["enums", "recordType"]),
        ["batch", "metric"],
        "the envelope and row validators read these positionally"
    );
    for (path, expected_len) in [
        (["namespace", "knownValues"], 12),
        (["errorType", "knownValues"], 12),
        (["featureFlags", "keys"], 6),
        (["enums", "memoryUsageBucket"], 7),
        (["enums", "outcome"], 6),
    ] {
        assert_eq!(
            strings(&path).len(),
            expected_len,
            "{path:?} cardinality changed"
        );
    }
}

#[test]
fn the_product_version_limits_mirror_the_contract() {
    assert_eq!(
        crate::policy::PRODUCT_VERSION_MAX_LEN as u64,
        integer(&["identifiers", "maxProductVersionLength"])
    );
    assert_eq!(
        crate::policy::PRODUCT_VERSION_COMPONENT_MAX,
        integer(&["identifiers", "maxVersionComponent"])
    );
}

#[test]
fn the_product_version_newtype_accepts_only_contract_valid_versions() {
    let at_limit = format!("1.2.3-{}", "a".repeat(26));
    let over_long = format!("{at_limit}a");
    for good in [
        "0.0",
        "1.0",
        "12.34",
        "99.99",
        "0.1.0",
        "1.2.3",
        "1.2.3-rc.1",
        "1.0.0+build.5",
        "1.2.3+-rc",
        "1.0--",
        "1.2.123456789",
        at_limit.as_str(),
    ] {
        assert!(
            crate::types::ProductVersion::new(good).is_ok() && valid_product_version(good),
            "{good:?} must pass both the newtype and the contract validator"
        );
    }
    for bad in [
        "01.2",
        "1.00",
        "007.1",
        "100.0",
        "1.100",
        "1",
        "v1.0",
        "",
        "1.0.0.0",
        "1.2.03",
        "1.0-",
        "1.0.0+",
        "1.0.0-rc_1",
        "1.2.3-+rc",
        "-1.0",
        "+1.0",
        "1..0",
        "1.2.",
        over_long.as_str(),
    ] {
        assert!(
            crate::types::ProductVersion::new(bad).is_err() && !valid_product_version(bad),
            "{bad:?} must fail both the newtype and the contract validator"
        );
    }
}

#[test]
fn the_contract_patterns_match_their_implemented_matchers() {
    for (path, implemented) in [
        (
            ["identifiers", "dottedVersionPattern"],
            DOTTED_VERSION_PATTERN_IMPLEMENTED,
        ),
        (
            ["identifiers", "projectIdPattern"],
            PROJECT_ID_PATTERN_IMPLEMENTED,
        ),
        (["identifiers", "uuidPattern"], UUID_PATTERN_IMPLEMENTED),
        (
            ["identifiers", "productVersionPattern"],
            PRODUCT_VERSION_PATTERN_IMPLEMENTED,
        ),
        (["identifiers", "regionPattern"], REGION_PATTERN_IMPLEMENTED),
        (
            ["identifiers", "wholeSecondRfc3339Pattern"],
            WHOLE_SECOND_RFC3339_PATTERN_IMPLEMENTED,
        ),
        (["operation", "pattern"], OPERATION_PATTERN_IMPLEMENTED),
    ] {
        assert_eq!(
            string(&path),
            implemented,
            "{path:?} changed in the synced contract: update the matcher and its corpus"
        );
    }

    let corpus: &[(&str, fn(&str) -> bool, &[&str], &[&str])] = &[
        (
            "operation",
            matches_operation,
            &[
                "PutObject",
                "GetQueueUrl",
                "CreateDBInstance",
                "GetSAMLProvider",
                "ListObjectsV2",
                "Aa",
            ],
            &[
                "",
                "12345678",
                "arn:aws:s3:::my-bucket",
                "AKIAIOSFODNN7EXAMPLE",
                "my-bucket-name",
                "john.doe.example.com",
                "john.doe@example.com",
                "secret.txt",
                "Put Object",
                "putObject",
                "Put_Object",
                "PutObject.v2",
                "PutObject:",
                "Put-Object",
                "A",
                "PUTOBJECT",
                "1PutObject",
                "PutÖbject",
                "a/b",
            ],
        ),
        (
            "dottedVersion",
            matches_dotted_version,
            &["22", "0", "1.85.0", "3.12", "01.002", "1.2.3.4.5"],
            &[
                "", ".", "1.", ".1", "1..2", "1.x", "v1", "1.0-rc.1", "1.0 ", "٣",
            ],
        ),
        (
            "projectId",
            matches_project_id,
            &["proj-0123456789abcdef0123456789abcdef"],
            &[
                "proj-1234",
                "proj-0123456789ABCDEF0123456789ABCDEF",
                "proj-0123456789abcdef0123456789abcde",
                "project-0123456789abcdef0123456789abcdef",
                "0123456789abcdef0123456789abcdef",
            ],
        ),
        (
            "uuid",
            matches_uuid,
            &[
                "6f3a1b2c-9d4e-4f5a-8b6c-7d8e9f0a1b2c",
                "11111111-2222-1333-9444-555555555555",
                "99999999-8888-8777-b666-555555555555",
            ],
            &[
                "6F3A1B2C-9D4E-4F5A-8B6C-7D8E9F0A1B2C",
                "6f3a1b2c-9d4e-0f5a-8b6c-7d8e9f0a1b2c",
                "6f3a1b2c-9d4e-9f5a-8b6c-7d8e9f0a1b2c",
                "6f3a1b2c-9d4e-4f5a-cb6c-7d8e9f0a1b2c",
                "6f3a1b2c9d4e4f5a8b6c7d8e9f0a1b2c",
                "6f3a1b2c-9d4e-4f5a-8b6c-7d8e9f0a1b2",
            ],
        ),
        (
            "productVersion",
            matches_product_version,
            &[
                "0.1",
                "1.0",
                "1.2.3",
                "10.20.30",
                "1.0-rc.1",
                "1.0+build-7",
                "99.99",
            ],
            &[
                "1", "1.", ".1", "01.2", "1.02", "1.2.3.4", "1.0-", "v1.0", "1.0 ",
            ],
        ),
        (
            "region",
            matches_region,
            &[
                "us-west-2",
                "eu-central-1",
                "us-gov-west-1",
                "ap-southeast-3",
                "il-central-1",
                "us-gov-1",
                "us-iso-east-1",
                "us-isob-east-1",
                "eu-isoe-west-1",
                "zz-west-1",
                "unknown",
            ],
            &[
                "us-west",
                "us-west-22",
                "US-WEST-2",
                "us-West-2",
                "us--2",
                "Unknown",
                "u-west-1",
            ],
        ),
        (
            "wholeSecondRfc3339",
            matches_whole_second_rfc3339,
            &["2026-09-23T10:00:00Z"],
            &[
                "2026-09-23T10:00:00.000Z",
                "2026-09-23T10:00:00+00:00",
                "2026-09-23 10:00:00Z",
                "2026-09-23T10:00:00",
            ],
        ),
    ];
    for (name, matcher, accepted, rejected) in corpus {
        for value in *accepted {
            assert!(matcher(value), "{name} matcher must accept {value:?}");
        }
        for value in *rejected {
            assert!(!matcher(value), "{name} matcher must reject {value:?}");
        }
    }

    let max_operation = integer(&["operation", "maxLength"]) as usize;
    assert_eq!(max_operation, 64);
    assert!(matches_operation(&format!(
        "P{}",
        "a".repeat(max_operation - 1)
    )));
    assert!(!matches_operation(&format!(
        "P{}",
        "a".repeat(max_operation)
    )));
    assert_eq!(
        string(&["operation", "foldedValue"]),
        crate::policy::DIMENSION_UNKNOWN
    );
}

fn minimal_envelope() -> Value {
    serde_json::json!({
        "recordType": "batch",
        "schemaVersion": string(&["envelope", "schemaVersion"]),
        "installationId": "6f3a1b2c-9d4e-4f5a-8b6c-7d8e9f0a1b2c",
        "sessionId": "0a1b2c3d-4e5f-4a6b-9c7d-8e9f0a1b2c3d",
        "projectId": "proj-0123456789abcdef0123456789abcdef",
        "batchId": "99999999-8888-4777-a666-555555555555",
        "flushSequence": 0,
        "sentAt": "2026-09-23T10:00:00Z",
        "productVersion": "1.0"
    })
}

fn minimal_row() -> Value {
    serde_json::json!({
        "recordType": "metric",
        "type": "operation",
        "namespace": "s3",
        "operation": "PutObject",
        "count": 2,
        "outcome": "success"
    })
}

/// Guards against over-strictness: the backend requires
/// neither `environment`, `performance`, `isFinal`, `sessionDurationMs` nor
/// `firstRun`, so a minimal batch must pass this validator too.
#[test]
fn a_minimal_envelope_and_row_are_accepted() {
    validate_envelope(&minimal_envelope()).unwrap();
    validate_row(&minimal_row()).unwrap();

    let mut full = minimal_envelope();
    full["isFinal"] = Value::Bool(true);
    full["sessionDurationMs"] = 42.into();
    full["previousVersion"] = "0.9".into();
    full["firstRun"] = Value::Bool(false);
    full["environment"] = serde_json::json!({
        "os": "linux",
        "arch": "x86_64",
        "sdkName": "aws-sdk-rust",
        "sdkVersion": "1.82.0",
        "languageVersion": "1.85.0",
        "runMode": "standalone",
        "isCi": false,
        "containerRuntime": "none",
        "agentClient": "other",
        "region": "us-west-2",
        "installChannel": "source"
    });
    let flags: Map<String, Value> = strings(&["featureFlags", "keys"])
        .iter()
        .map(|key| ((*key).to_string(), Value::Bool(true)))
        .collect();
    full["environment"]["featureFlags"] = Value::Object(flags);
    full["performance"] = serde_json::json!({
        "memoryUsage": strings(&["enums", "memoryUsageBucket"])[0],
        "throughput": 4,
        "peakConcurrency": 2,
        "coldStartTime": 15,
        "startupOutcome": "success"
    });
    full["crash"] = serde_json::json!({ "crashType": "panic" });
    validate_envelope(&full).unwrap();

    let mut error_row = minimal_row();
    error_row["type"] = "error".into();
    error_row["outcome"] = "error".into();
    error_row["errorType"] = "Timeout".into();
    error_row["latency"] = serde_json::json!({ "min": 10, "max": 20, "sumMillis": 30 });
    validate_row(&error_row).unwrap();

    let mut folded_row = minimal_row();
    folded_row["namespace"] = "kendra".into();
    folded_row["type"] = "coverage_gap".into();
    folded_row["outcome"] = "not_emulated".into();
    folded_row["errorType"] = string(&["errorType", "foldedValue"]).into();
    validate_row(&folded_row).unwrap();

    let mut fractional = minimal_row();
    fractional["count"] = 3.into();
    fractional["latency"] = serde_json::json!({ "min": 0.1, "max": 0.1, "sumMillis": 0.3 });
    validate_row(&fractional).expect("sub-millisecond sums within tolerance are accepted");
}

#[test]
fn the_validator_rejects_envelope_contract_violations() {
    let cases: &[(&str, fn(&mut Value))] = &[
        ("legacy project id", |envelope| {
            envelope["projectId"] = "proj-1234".into();
        }),
        ("uppercase uuid", |envelope| {
            envelope["installationId"] = "6F3A1B2C-9D4E-4F5A-8B6C-7D8E9F0A1B2C".into();
        }),
        ("uuid version nibble", |envelope| {
            envelope["sessionId"] = "0a1b2c3d-4e5f-0a6b-9c7d-8e9f0a1b2c3d".into();
        }),
        ("uuid variant nibble", |envelope| {
            envelope["batchId"] = "99999999-8888-4777-c666-555555555555".into();
        }),
        ("wrong schema version", |envelope| {
            envelope["schemaVersion"] = "2".into();
        }),
        ("wrong record type", |envelope| {
            envelope["recordType"] = "metric".into();
        }),
        ("negative flush sequence", |envelope| {
            envelope["flushSequence"] = (-1).into();
        }),
        ("unsafe flush sequence", |envelope| {
            envelope["flushSequence"] = 9_007_199_254_740_992_u64.into();
        }),
        ("fractional sent at", |envelope| {
            envelope["sentAt"] = "2026-09-23T10:00:00.000Z".into();
        }),
        ("impossible calendar date", |envelope| {
            envelope["sentAt"] = "2026-13-01T00:00:00Z".into();
        }),
        ("offset timestamp", |envelope| {
            envelope["sentAt"] = "2026-09-23T10:00:00+00:00".into();
        }),
        ("product is not a contract field", |envelope| {
            envelope["product"] = serde_json::json!({ "productVersion": "1.0" });
        }),
        ("version component above the cap", |envelope| {
            envelope["productVersion"] = "100.0".into();
        }),
        ("zero-padded version", |envelope| {
            envelope["productVersion"] = "01.2".into();
        }),
        ("major-only version", |envelope| {
            envelope["productVersion"] = "1".into();
        }),
        ("unknown envelope field", |envelope| {
            envelope["unexpected"] = Value::Bool(true);
        }),
        ("string is-final", |envelope| {
            envelope["isFinal"] = "yes".into();
        }),
        ("oversized session duration", |envelope| {
            envelope["sessionDurationMs"] = 2_592_000_001_u64.into();
        }),
        ("unknown operating system", |envelope| {
            envelope["environment"] = serde_json::json!({ "os": "beos" });
        }),
        ("oversized sdk version", |envelope| {
            envelope["environment"] =
                serde_json::json!({ "sdkVersion": format!("1.{}", "0".repeat(64)) });
        }),
        ("unversioned region", |envelope| {
            envelope["environment"] = serde_json::json!({ "region": "us-west" });
        }),
        ("boolean-valued flag with unknown key", |envelope| {
            let mut flags = Map::new();
            flags.insert("telemetryLevel".to_string(), Value::Bool(true));
            envelope["environment"] = serde_json::json!({ "featureFlags": flags });
        }),
        ("non-boolean flag value", |envelope| {
            let key = strings(&["featureFlags", "keys"])[0].to_string();
            let mut flags = Map::new();
            flags.insert(key, "on".into());
            envelope["environment"] = serde_json::json!({ "featureFlags": flags });
        }),
        ("fractional peak concurrency", |envelope| {
            envelope["performance"] = serde_json::json!({ "peakConcurrency": 1.5 });
        }),
        ("negative throughput", |envelope| {
            envelope["performance"] = serde_json::json!({ "throughput": -1 });
        }),
        ("unknown crash type", |envelope| {
            envelope["crash"] = serde_json::json!({ "crashType": "segfault" });
        }),
        ("crash location is never sent", |envelope| {
            envelope["crash"] =
                serde_json::json!({ "crashType": "panic", "crashLocation": "src/main.rs" });
        }),
    ];
    for (name, mutate) in cases {
        let mut envelope = minimal_envelope();
        mutate(&mut envelope);
        assert!(
            validate_envelope(&envelope).is_err(),
            "envelope with {name} must be rejected"
        );
    }
    for key in strings(&["envelope", "requiredFields"]) {
        let mut envelope = minimal_envelope();
        envelope.as_object_mut().unwrap().remove(key);
        assert!(
            validate_envelope(&envelope).is_err(),
            "envelope without {key} must be rejected"
        );
    }
}

#[test]
fn the_validator_rejects_row_contract_violations() {
    let cases: &[(&str, fn(&mut Value))] = &[
        ("unknown metric type", |row| {
            row["type"] = "gauge".into();
        }),
        ("non-canonical known namespace", |row| {
            row["namespace"] = "S3".into();
        }),
        ("untrimmed namespace", |row| {
            row["namespace"] = " s3".into();
        }),
        ("oversized namespace", |row| {
            row["namespace"] = "a".repeat(65).into();
        }),
        ("operation with a space", |row| {
            row["operation"] = "Put Object".into();
        }),
        ("unknown outcome", |row| {
            row["outcome"] = "ok".into();
        }),
        ("negative count", |row| {
            row["count"] = (-1).into();
        }),
        ("fractional count", |row| {
            row["count"] = 1.5.into();
        }),
        ("unsafe count", |row| {
            row["count"] = 9_007_199_254_740_992_u64.into();
        }),
        ("non-canonical error type", |row| {
            row["errorType"] = "validation_error".into();
        }),
        ("free-text error type", |row| {
            row["errorType"] = "SomethingBrokeInProduction".into();
        }),
        ("unknown row field", |row| {
            row["latencyMs"] = 10.into();
        }),
        ("inverted latency bounds", |row| {
            row["latency"] = serde_json::json!({ "min": 10, "max": 5, "sumMillis": 10 });
        }),
        ("latency sum without samples", |row| {
            row["count"] = 0.into();
            row["latency"] = serde_json::json!({ "min": 0, "max": 0, "sumMillis": 5 });
        }),
        ("latency sum outside its bounds", |row| {
            row["latency"] = serde_json::json!({ "min": 10, "max": 20, "sumMillis": 100 });
        }),
        ("latency missing a field", |row| {
            row["latency"] = serde_json::json!({ "min": 10, "max": 20 });
        }),
        ("latency with an unknown field", |row| {
            row["latency"] =
                serde_json::json!({ "min": 10, "max": 20, "sumMillis": 30, "p99": 19 });
        }),
    ];
    for (name, mutate) in cases {
        let mut row = minimal_row();
        mutate(&mut row);
        assert!(
            validate_row(&row).is_err(),
            "row with {name} must be rejected"
        );
    }
    for key in strings(&["metricRow", "requiredFields"]) {
        let mut row = minimal_row();
        row.as_object_mut().unwrap().remove(key);
        assert!(
            validate_row(&row).is_err(),
            "row without {key} must be rejected"
        );
    }
}

#[test]
fn serialized_batch_passes_the_shipped_backend_validator_contract() {
    let context = SessionContext {
        installation_id: InstallationId::new("11111111-2222-4333-8444-555555555555").unwrap(),
        session_id: SessionId::new("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap(),
        project_id: ProjectId::new("proj-0123456789abcdef0123456789abcdef").unwrap(),
        version: ProductVersionFields {
            product_version: ProductVersion::new("1.0").unwrap(),
            previous_version: None,
            first_run: true,
        },
        environment: EnvironmentBlock {
            os: OsFamily::Linux,
            arch: Arch::Amd64,
            sdk_name: SdkDimension::Sdk(SdkName::AwsSdkRust),
            sdk_version: Some(SdkVersion::new("1.82.0").unwrap()),
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
    };
    let mut metrics = BTreeMap::new();
    let mut value = MetricValue::default();
    value.observe(Some(10));
    value.observe(Some(20));
    metrics.insert(
        MetricKey {
            metric_type: MetricType::Operation,
            namespace: Namespace::new("s3").unwrap(),
            operation: OperationName::new("PutObject").unwrap(),
            outcome: Outcome::Success,
            error_type: None,
        },
        value,
    );
    metrics.insert(
        MetricKey {
            metric_type: MetricType::CoverageGap,
            namespace: Namespace::new("lambda").unwrap(),
            operation: OperationName::new("Invoke").unwrap(),
            outcome: Outcome::NotEmulated,
            error_type: None,
        },
        MetricValue {
            count: 1,
            latency: None,
        },
    );
    metrics.insert(
        MetricKey {
            metric_type: MetricType::Error,
            namespace: Namespace::new("dynamodb").unwrap(),
            operation: OperationName::new("Query").unwrap(),
            outcome: Outcome::Error,
            error_type: Some(ErrorType::Timeout),
        },
        MetricValue {
            count: 1,
            latency: None,
        },
    );
    let payload = BatchPayload {
        context: &context,
        metrics: &metrics,
        flush_sequence: 1,
        is_final: false,
        session_duration_ms: 42,
        performance: PerformanceBlock {
            memory_usage: Some(MemoryBucket::LessThan256Mib),
            throughput: Some(4),
            peak_concurrency: Some(2),
            cold_start_time: Some(15),
            startup_outcome: Some(StartupOutcome::Success),
        },
        crash: Some(CrashBlock {
            crash_type: CrashType::Panic,
        }),
    };
    let batches = build_batches(
        &payload,
        BatchLimits {
            max_rows_per_batch: 1000,
            max_body_bytes: 262_144,
        },
        &mut || BatchId::new("99999999-8888-4777-a666-555555555555").unwrap(),
    );
    assert_eq!(batches.len(), 1);
    let body = patch_sent_at(&batches[0].body, &batches[0].batch_id, epoch_seconds_now()).unwrap();
    assert!(
        body.len() as u64 <= integer(&["transport", "maxBodyBytes"]),
        "batch body exceeds the transport cap"
    );
    let rows: Vec<Value> = body
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 4, "one envelope and three metric variants");
    assert_eq!(
        rows[0].get("productVersion").and_then(Value::as_str),
        Some("1.0"),
        "the contract makes productVersion optional, but the client always sends it: \
         without it the backend buckets the batch under the Other version dimension"
    );
    validate_envelope(&rows[0]).unwrap();
    for row in &rows[1..] {
        validate_row(row).unwrap();
    }
}

#[test]
fn the_backend_accepts_what_it_folds_and_optional_product_versions() {
    let mut envelope = minimal_envelope();
    envelope.as_object_mut().unwrap().remove("productVersion");
    validate_envelope(&envelope).expect("the backend keeps productVersion optional");

    let mut folded = minimal_envelope();
    folded["environment"] = serde_json::json!({
        "sdkVersion": "2",
        "languageVersion": "22",
        "runMode": "unknown"
    });
    folded["performance"] = serde_json::json!({
        "memoryUsage": "unknown",
        "startupOutcome": "unknown"
    });
    folded["crash"] = serde_json::json!({ "crashType": "unknown" });
    validate_envelope(&folded).expect("dotted versions and folded unknown tokens are accepted");

    for (key, value) in [("sdkVersion", "1.0-rc.1"), ("languageVersion", "v22")] {
        let mut envelope = minimal_envelope();
        envelope["environment"] = serde_json::json!({ key: value });
        assert!(
            validate_envelope(&envelope).is_err(),
            "environment.{key} {value:?} is outside the dotted-version pattern"
        );
    }
}

/// The client's SDK and language version newtypes must accept exactly what
/// the contract's dotted-version rule accepts, so a version the client builds
/// is never folded by the backend and a version the backend would keep is
/// never refused by the client.
#[test]
fn the_sdk_and_language_version_newtypes_match_the_dotted_version_rule() {
    let max = integer(&["identifiers", "maxSdkOrLanguageVersionLength"]) as usize;
    assert_eq!(max, policy::VERSION_DIMENSION_MAX_LEN);
    let contract_accepts = |value: &str| value.len() <= max && matches_dotted_version(value);
    let cases = [
        "22".to_string(),
        "0".to_string(),
        "1.82.0".to_string(),
        "3.12.1.4".to_string(),
        "9".repeat(max),
        "9".repeat(max + 1),
        String::new(),
        ".".to_string(),
        "1.".to_string(),
        ".1".to_string(),
        "1..2".to_string(),
        "1.x".to_string(),
        "v1".to_string(),
        "1.0-rc.1".to_string(),
    ];
    for value in &cases {
        let expected = contract_accepts(value);
        assert_eq!(
            SdkVersion::new(value).is_ok(),
            expected,
            "SdkVersion {value:?}"
        );
        assert_eq!(
            LanguageVersion::new(value).is_ok(),
            expected,
            "LanguageVersion {value:?}"
        );
    }
}

/// The contract states the backend's operation pattern: a PascalCase AWS
/// operation name, or the fold token. The client's [`OperationName`] grammar
/// is the SAME shape, and folds anything outside it to the unknown token.
/// Every operation the client can produce must therefore pass the contract's
/// pattern and length bound, and nothing identifier-shaped may survive.
#[test]
fn the_client_operation_grammar_is_a_subset_of_the_contract_pattern() {
    assert!(policy::DIMENSION_MAX_LEN <= integer(&["operation", "maxLength"]) as usize);
    for identifier_shaped in [
        "12345678",
        "arn:aws:sqs",
        "AKIAIOSFODNN7EXAMPLE",
        "my-bucket-name",
        "john.doe.example.com",
        "secret.txt",
    ] {
        assert!(!matches_operation(identifier_shaped));
        assert_eq!(
            OperationName::fold(identifier_shaped).as_str(),
            policy::DIMENSION_UNKNOWN,
            "the client must fold {identifier_shaped:?}"
        );
    }
    let mut attempts = dimension_fold_boundary_cases();
    let mut rng = crate::test_support::FuzzRng(FUZZ_SEED);
    attempts.extend((0..FUZZ_ITERATIONS).map(|_| rng.string()));
    for attempt in &attempts {
        let folded = OperationName::fold(attempt);
        assert!(
            folded.as_str() == policy::DIMENSION_UNKNOWN || matches_operation(folded.as_str()),
            "folded operation {folded:?} from {attempt:?} must match the contract pattern"
        );
        let mut row = minimal_row();
        row["operation"] = folded.as_str().into();
        validate_row(&row).unwrap_or_else(|reason| {
            panic!("folded operation {folded:?} from {attempt:?} must be contract-valid: {reason}")
        });
    }
}

const FUZZ_SEED: u64 = 0x5eed_1e11_e7e1_e017;
const FUZZ_ITERATIONS: usize = 4000;

/// The namespace grammar evaluated independently of the fold under test:
/// non-empty, at most [`crate::policy::DIMENSION_MAX_LEN`] bytes, ASCII
/// alphanumerics plus [`crate::policy::DIMENSION_EXTRA_ALLOWED_BYTES`].
fn expected_namespace_fold(attempt: &str) -> &str {
    let grammar_valid = !attempt.is_empty()
        && attempt.len() <= crate::policy::DIMENSION_MAX_LEN
        && attempt.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || crate::policy::DIMENSION_EXTRA_ALLOWED_BYTES.contains(&byte)
        });
    if grammar_valid {
        attempt
    } else {
        crate::policy::DIMENSION_UNKNOWN
    }
}

/// The operation grammar evaluated independently of the fold under test:
/// the contract's operation pattern, which is at most
/// [`crate::policy::DIMENSION_MAX_LEN`] bytes.
fn expected_operation_fold(attempt: &str) -> &str {
    if matches_operation(attempt) && attempt.len() <= crate::policy::DIMENSION_MAX_LEN {
        attempt
    } else {
        crate::policy::DIMENSION_UNKNOWN
    }
}

/// The edges the random strings rarely land on: empty, exactly the length
/// cap, one past it, every allowed punctuation byte, single disallowed
/// bytes (space, slash, colon, non-ASCII), and real AWS operation names with
/// acronyms and digits.
fn dimension_fold_boundary_cases() -> Vec<String> {
    let cap = crate::policy::DIMENSION_MAX_LEN;
    let punctuation = String::from_utf8(crate::policy::DIMENSION_EXTRA_ALLOWED_BYTES.to_vec())
        .expect("policy punctuation is ASCII");
    vec![
        String::new(),
        "a".repeat(cap),
        "a".repeat(cap + 1),
        format!("P{}", "a".repeat(cap - 1)),
        format!("P{}", "a".repeat(cap)),
        format!("a{punctuation}Z9"),
        "Send Message".to_string(),
        "sqs/SendMessage".to_string(),
        "arn:aws:sqs".to_string(),
        "Sénd".to_string(),
        "SendMessage".to_string(),
        "CreateDBInstance".to_string(),
        "GetSAMLProvider".to_string(),
        "ListObjectsV2".to_string(),
        "AKIAIOSFODNN7EXAMPLE".to_string(),
        crate::policy::DIMENSION_UNKNOWN.to_string(),
    ]
}

/// Every wire value is a closed vocabulary, enforced client
/// AND backend. Random strings pushed through every public input surface must
/// come out as contract-accepted tokens (or be rejected at the typed
/// boundary) — nothing free-form can serialize.
#[test]
fn random_inputs_fold_to_contract_accepted_vocabulary_only() {
    println!("closed-vocabulary fuzz: seed={FUZZ_SEED:#x} iterations={FUZZ_ITERATIONS}");
    let known_regions: std::collections::BTreeSet<&str> =
        crate::policy::KNOWN_REGIONS.iter().copied().collect();
    for attempt in dimension_fold_boundary_cases() {
        assert_eq!(
            crate::types::Namespace::fold(&attempt).as_str(),
            expected_namespace_fold(&attempt),
            "boundary case: {attempt:?}"
        );
        assert_eq!(
            crate::types::OperationName::fold(&attempt).as_str(),
            expected_operation_fold(&attempt),
            "boundary case: {attempt:?}"
        );
    }
    let mut rng = crate::test_support::FuzzRng(FUZZ_SEED);
    for iteration in 0..FUZZ_ITERATIONS {
        let raw_region = rng.region_candidate();
        let folded = crate::types::Region::fold(&raw_region);
        assert!(
            matches_region(folded.as_str()),
            "iteration {iteration}: folded region {folded:?} from {raw_region:?} \
             must satisfy the backend region pattern"
        );
        assert!(
            folded.as_str() == "unknown" || known_regions.contains(folded.as_str()),
            "iteration {iteration}: folded region {folded:?} must be known or the fold token"
        );

        let error_source = rng.string();
        let error_token =
            crate::policy::error_type_token(crate::types::ErrorType::from_code(&error_source));
        let error_row = serde_json::json!({
            "recordType": "metric",
            "type": "error",
            "namespace": "sqs",
            "operation": "SendMessage",
            "count": 1,
            "outcome": "error",
            "errorType": error_token,
        });
        validate_row(&error_row).unwrap_or_else(|reason| {
            panic!(
                "iteration {iteration}: folded errorType {error_token:?} from \
                 {error_source:?} must validate: {reason}"
            )
        });

        let mut headers = http::HeaderMap::new();
        if let Ok(value) = http::HeaderValue::from_str(&rng.string()) {
            headers.insert(http::header::USER_AGENT, value);
        }
        let sanitized = crate::metadata::sanitize_request_metadata(
            &headers,
            crate::types::Region::new(&raw_region).ok(),
        );
        let mut environment = serde_json::Map::new();
        environment.insert("os".into(), "linux".into());
        environment.insert("arch".into(), "x86_64".into());
        environment.insert("runMode".into(), "standalone".into());
        environment.insert("installChannel".into(), "source".into());
        environment.insert("isCi".into(), false.into());
        environment.insert("containerRuntime".into(), "none".into());
        let sdk_dimension = match sanitized.sdk {
            crate::metadata::Merged::Unset => crate::types::SdkDimension::Unknown,
            crate::metadata::Merged::Mixed => crate::types::SdkDimension::Mixed,
            crate::metadata::Merged::Value(sdk) => crate::types::SdkDimension::Sdk(sdk),
        };
        environment.insert("sdkName".into(), sdk_dimension.as_str().into());
        if let Some(version) = &sanitized.sdk_version {
            environment.insert("sdkVersion".into(), version.as_str().into());
        }
        if let Some(version) = &sanitized.language_version {
            environment.insert("languageVersion".into(), version.as_str().into());
        }
        if let Some(agent) = sanitized.agent_client {
            environment.insert("agentClient".into(), agent.as_str().into());
        }
        environment.insert("region".into(), folded.as_str().into());
        environment.insert(
            "featureFlags".into(),
            serde_json::json!({
                "persistence": false,
                "multiRegion": false,
                "authEmulation": false,
                "strictMode": false,
                "requestLogging": false,
                "fidelityGating": false,
            }),
        );
        validate_environment(&serde_json::Value::Object(environment.clone())).unwrap_or_else(
            |reason| {
                panic!(
                    "iteration {iteration}: sanitized metadata must serialize to a \
                     contract-valid environment ({reason}): {environment:?}"
                )
            },
        );

        let namespace_attempt = rng.string();
        let operation_attempt = rng.string();
        let namespace = crate::types::Namespace::fold(&namespace_attempt);
        let operation = crate::types::OperationName::fold(&operation_attempt);
        for (folded, expected, attempt) in [
            (
                namespace.as_str(),
                expected_namespace_fold(&namespace_attempt),
                &namespace_attempt,
            ),
            (
                operation.as_str(),
                expected_operation_fold(&operation_attempt),
                &operation_attempt,
            ),
        ] {
            assert_eq!(
                folded, expected,
                "iteration {iteration}: a dimension survives the fold iff it is \
                 grammar-valid, else becomes the unknown token: {attempt:?}"
            );
        }
        let failure =
            crate::client::OperationFailure::classify(rng.below(1000) as u16, &rng.string());
        let mut row = serde_json::json!({
            "recordType": "metric",
            "type": if failure.is_coverage_gap() { "coverage_gap" } else { "operation" },
            "namespace": namespace.as_str(),
            "operation": operation.as_str(),
            "count": 1,
            "outcome": serde_json::to_value(failure.outcome()).unwrap(),
        });
        if let Some(error_type) = failure.error_type() {
            row["errorType"] = crate::policy::error_type_token(error_type).into();
        }
        validate_row(&row).unwrap_or_else(|reason| {
            panic!(
                "iteration {iteration}: a folded handle and classified failure must be \
                 contract-valid ({reason}): {namespace_attempt:?}/{operation_attempt:?} {row}"
            )
        });

        let bucket = crate::policy::memory_usage_bucket(rng.next());
        let performance = serde_json::json!({
            "memoryUsage": serde_json::to_value(bucket).unwrap(),
            "throughput": 1,
            "peakConcurrency": 1,
            "coldStartTime": 1,
            "startupOutcome": "success",
        });
        validate_performance(&performance).unwrap_or_else(|reason| {
            panic!("iteration {iteration}: memory bucket {bucket:?} must validate: {reason}")
        });
    }
}
