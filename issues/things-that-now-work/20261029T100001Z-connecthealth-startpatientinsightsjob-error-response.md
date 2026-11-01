Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: cc22e59eeda2de4fd1330e74811c0c72a53dba7a

# The clientToken fingerprint cannot tell an absent s3Sources from an empty one, so a changed request is replayed as the first job instead of the documented ConflictException

_Filed: 2026-10-29T10:00:01Z_
_Operation: StartPatientInsightsJob | Class: error-response_

> _Note: this issue was identified against the code base at commit `cc22e59eeda2de4fd1330e74811c0c72a53dba7a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PatientInsightsJobSpec::fingerprint` is the digest `StartPatientInsightsJob` compares a reused `clientToken` against: equal digest means "the retry of one request" and the first job is replayed; a different digest is the modelled `ConflictException`. The digest folds in a part for every parameter, and the function's own doc states the rule it follows — "an optional member contributes a part whether or not it was supplied" — which the sibling optional member in the very same function honours (`fhir_server` pushes an explicit empty part when absent). The `s3Sources` section does not: `self.input_data_config.s3_sources.iter().flatten()` yields NO part for `None` and NO part for `Some(vec![])`, and no element count is folded in ahead of the elements (unlike `domain_creation_fingerprint`, which pushes `tags.len()` "so the sections cannot be read into one another").

So two requests the model admits as different — `"inputDataConfig": {}` and `"inputDataConfig": {"s3Sources": []}` (the modelled `S3Sources` list declares `length {max: 10}` and no minimum, so an empty list is admissible) — produce the same fingerprint. Presenting the second under a `clientToken` already bound to the first is answered 200 with the FIRST job's ids instead of the 409 `ConflictException` this handler documents for "the same token presented with different job parameters". The difference is not cosmetic for this crate: the start persists `input_data_config` verbatim on the record and `GetPatientInsightsJob` projects it straight back out (`input_data_config: job.spec.input_data_config.clone()`), so the stored `Some([])` and `None` are two observably different records — the replay silently drops the member the second caller supplied and the later read reports no `s3Sources` at all.

## Evidence

The rule the digest states for itself — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:493-495`:
```
    /// Every value is length-prefixed before it is folded in, so two different parameter
    /// sets cannot collide by running their text together, and an optional member
    /// contributes a part whether or not it was supplied.
```

The code that breaks it, beside the sibling optional member that keeps it — `src/state.rs:508-518`:
```
        match &self.input_data_config.fhir_server {
            Some(server) => {
                parts.push("fhirServer".to_string());
                parts.push(server.fhir_endpoint.clone());
                parts.push(server.oauth_token.clone().unwrap_or_default());
            }
            None => parts.push(String::new()),
        }
        for source in self.input_data_config.s3_sources.iter().flatten() {
            parts.push(source.uri.clone());
        }
```
(`s3_sources` is `Option<Vec<S3Source>>`; `.iter().flatten()` pushes zero parts for `None` and zero parts for `Some(vec![])`.)

The precaution the sibling fingerprint of this same crate takes for a variable-length section — `src/state.rs:545-549` and `src/state.rs:561`:
```
/// `tags` is digested in KEY order, ... the entry count is
/// folded in ahead of them so the sections cannot be read into one another.
```
```
    parts.push(tags.len().to_string());
```

The behaviour the handler documents for a changed request — `src/ops/start_patient_insights_job.rs:106-109`:
```
//! `clientToken` is an `@idempotencyToken`: a retry carrying a token this domain has
//! already bound to a job answers with THAT job rather than starting a second one, and
//! the same token presented with different job parameters is the modelled
//! `ConflictException`.
```
and the comparison that decides it — `src/ops/start_patient_insights_job.rs:250-268`:
```
                if existing.spec.fingerprint() != fingerprint {
                    return Err(StartPatientInsightsJobErrorModeled::ConflictException(
...
                // The idempotent replay: the job the first call created, read back out
                // of the store.
                return Ok(StartPatientInsightsJobOutput {
                    job_arn: existing.arn.clone(),
```

That the two spellings are observably different records: the start stores the config as given (`src/ops/start_patient_insights_job.rs:229-236`, `input_data_config: input.input_data_config`) and the reader projects it back (`src/ops/get_patient_insights_job.rs:242`, `input_data_config: job.spec.input_data_config.clone()`).

Model admissibility of the empty list — `smithy_query(query="shape",shape="InputDataConfig")`: `"s3Sources": {"target": "S3Sources", "length": {"max": 10}, "type": "list"}` (optional, no minimum).

Test coverage: `tests/all/patient_insights_job_lifecycle.rs:505-575` is the conflict table, introduced as "one row per member a request can differ in" — its `inputDataConfig` rows swap in a `fhirServer` and in `s3_sources(2)`, both of which DO change the digest, so no row exercises the absent-vs-empty `s3Sources` pair; `minimal_body()` (same file, :115-124) always sends one source.

## Steps to Reproduce

1. POST /domain with {"name":"clinic"} and capture domainId.
2. POST /domain/{domainId}/patient-insights-job with the required members (patientContext, insightsContext, encounterContext, userContext, outputDataConfig), "inputDataConfig": {} and "clientToken": "t1" — 200, capture jobId J1.
3. POST the identical body again with the same "clientToken": "t1" but "inputDataConfig": {"s3Sources": []}.
4. Expected per src/ops/start_patient_insights_job.rs:106-109: 409 ConflictException, because the token is bound to a job started with different parameters. Actual: 200 carrying jobId J1 — the fingerprints are equal, since neither spelling of s3Sources contributes a part.
5. GET /domain/{domainId}/patient-insights-job/J1: inputDataConfig carries no s3Sources member, so the second request's parameter was dropped rather than refused — while a job genuinely started with "s3Sources": [] would have reported the empty list back.
