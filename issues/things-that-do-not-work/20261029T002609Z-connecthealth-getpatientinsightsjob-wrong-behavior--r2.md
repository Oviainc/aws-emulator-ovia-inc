Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f21d2937dde7ed50f2d73e4c6f2a7c8bdd006019

# The derived insightsOutput URI is bounded by a BYTE count, so a multi-byte s3OutputPath inside the modelled 1024-CHARACTER S3Uri bound silently loses its per-job prefix

_Filed: 2026-10-29T00:26:09Z_
_Operation: GetPatientInsightsJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f21d2937dde7ed50f2d73e4c6f2a7c8bdd006019`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::patient_insights_output_uri` is the one place the terminal `insightsOutput.uri` a job reports is derived: it appends `/<jobId>/` to the `s3OutputPath` the start persisted, and falls back to reporting the configured path UNCHANGED when the derived value would not fit the modelled `S3Uri` bound. The fit test is `derived.len() <= MAX_S3_URI_LEN` — `str::len()` is a BYTE count, while `MAX_S3_URI_LEN` is documented in the same module as the MODELLED `@length max 1024`, which Smithy measures in characters (the crate states exactly that for the other modelled string bound it keeps itself, 20 lines above the constant).

So a configured path that the model accepts (≤1024 characters) but whose UTF-8 encoding is longer than 1024 bytes drives the fallback even though the derived URI is far inside the bound. Example: a path of 600 characters of which 400 are two-byte characters is 1000 bytes; the derived URI is 634 characters (1034 bytes), i.e. comfortably inside `S3Uri`'s 1024-character bound, yet the handler reports the bare configured path with no job prefix. Two jobs configured with that path then report the IDENTICAL `insightsOutput.uri`, which is the exact outcome the function's own doc says the job id exists to prevent.

The module's sibling derivation does it the other way round: `idc_application_id`, which keeps the reported `WebAppConfiguration$idcApplicationId` inside its own modelled 256-character bound, truncates with `chars().take(MAX_IDC_INSTANCE_SEGMENT)` — characters, not bytes — and the tag plane enforces its modelled `TagKey`/`TagValue` lengths through the framework's own length walk, which this crate's tests pin as character-counting. `patient_insights_output_uri` is the one derivation in the module that counts bytes.

This is a different gap from the fixed GetPatientInsightsJob issue "Every patient insights job ends in FAILED, so SUCCEEDED and insightsOutput are unreachable": that one was about the terminal status being unreachable; this one is about the URI the (now reachable) terminal status reports.

## Evidence

The byte-counted bound — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:1352-1358`:
```
pub fn patient_insights_output_uri(s3_output_path: &str, job_id: &str) -> String {
    let derived = format!("{}/{job_id}/", s3_output_path.trim_end_matches('/'));
    if derived.len() <= MAX_S3_URI_LEN {
        derived
    } else {
        s3_output_path.to_string()
    }
```

What the constant it is compared against IS — `src/state.rs:127-133`:
```
/// The MODELLED length bound of an S3 URI this service reports or accepts: `S3Uri` is
/// `@length max 1024` (`smithy_query(query="shape",shape="S3Uri,InsightsOutput")`), which
/// holds for `InsightsOutput$uri` as much as for `OutputDataConfig$s3OutputPath`.
...
pub const MAX_S3_URI_LEN: usize = 1024;
```

The crate's OWN statement of how a modelled `@length` is measured, in the same file — `src/state.rs:144-147`:
```
/// The MODELLED length bounds of one tag entry, both ends of each: `TagKey` is
/// `@length 1..128` and `TagValue` is `@length 0..256`
/// (`smithy_query(query="shape",shape="TagMap,TagKey,TagValue")`), which are also AWS's
/// standard tagging limits. Measured in CHARACTERS, as `@length` on a string is defined.
```

The sibling derivation in the SAME module that keeps a derived value inside a modelled bound by CHARACTERS — `src/state.rs:1250-1254`:
```
pub fn idc_application_id(idc_instance_id: &str, suffix: &str) -> String {
    let instance: String = idc_instance_id
        .chars()
        .take(MAX_IDC_INSTANCE_SEGMENT)
        .collect();
```
(its doc, `src/state.rs:1219-1227`: "The most characters of the Identity Center instance id a create named that this service carries into the application identifier it reports ... This is what keeps the reported identifier inside its own modelled bound" — and the test checks it with `reported.chars().count()`, `tests/all/domain_web_app_identity.rs:202`.)

What the fallback destroys — `src/state.rs:1339-1346`:
```
/// Where a completed patient insights job's output sits: UNDER the `s3OutputPath` the
/// start configured, in a prefix of the job's own id.
///
/// The configured path is where the caller asked for output, and the job's id is what
/// keeps two jobs configured with the same path from reporting the same location, so the
/// reported URI is the configured path (with any trailing separator collapsed) followed
/// by `<jobId>/`.
```

That a multi-byte path is a value the MODEL accepts — `smithy_query(query="shape", shape="OutputDataConfig")` gives `s3OutputPath`: `{"target": "S3Uri", "required": true, "length": {"max": 1024}, "pattern": "^s3://[a-z0-9][\\.\\-a-z0-9]{1,61}[a-z0-9](/.*)?$"}` — the key segment is `(/.*)?`, so every character is allowed after the bucket field, and the only size rule is the 1024-character `@length`.

Where the byte-measured value is written and read: `src/state.rs:629-634` (`JobStatus::InProgress => { self.status = JobStatus::Succeeded; self.insights_output = Some(InsightsOutput { uri: patient_insights_output_uri(&self.spec.output_data_config.s3_output_path, &self.job_id) }); }`) and this operation's projection, `src/ops/get_patient_insights_job.rs:239` (`insights_output: job.insights_output.clone(),`).

Test coverage: `tests/all/patient_insights_job_terminal_output.rs` is the file that pins this URI, and every path it builds is ASCII — `long_path` is `format!("{head}{}", "a".repeat(len - head.len()))` (line 145) — and its own room helper is byte-based (`path.trim_end_matches('/').len() + job_id.len() + 2 <= MAX_S3_URI_LEN`, line 265). So the character/byte distinction is unpinned, and switching the comparison to a character count leaves every existing assertion in that file (and every other test) unchanged. By contrast the tag plane HAS the multi-byte rows: `tests/all/tag_map_bounds.rs:222-226` ("`@length` counts characters, so a key of 128 multi-byte characters is INSIDE the ...") and `src/service/tag_bounds.rs:535-538`.

## Steps to Reproduce

1. POST /domain with {"name":"clinic"} and capture domainId.
2. POST /domain/{domainId}/patient-insights-job with the six required members, setting outputDataConfig.s3OutputPath to a valid S3 URI of about 600 CHARACTERS whose key segment is mostly two-byte characters (e.g. "s3://bucket-name/" followed by 'é' repeated ~500 times plus ASCII padding) — 600 characters is well inside S3Uri's modelled @length max 1024, so the boundary accepts it, but the UTF-8 encoding is >990 bytes. Capture jobId.
3. Wait ~900 ms (two PATIENT_INSIGHTS_JOB_STEP_SPANs) so the clock drives the job to SUCCEEDED.
4. GET /domain/{domainId}/patient-insights-job/{jobId}: jobStatus is SUCCEEDED and insightsOutput.uri is the configured s3OutputPath VERBATIM, with no "/<jobId>/" suffix — although the derived URI would be ~634 characters, far inside the 1024-character bound the fallback exists to protect.
5. Start a second job in the same domain with the identical s3OutputPath, wait, and read it: both jobs report the SAME insightsOutput.uri, the collision src/state.rs:1342-1344 says the job id is there to prevent.
6. Repeat steps 2-4 with an ASCII path of the same 600 characters: the uri correctly carries the "/<jobId>/" suffix, showing the only difference is that the bound is measured in bytes.
