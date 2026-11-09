Issue-Source: trace-sweep-divergence
Issue-Trace: connecthealth-patient-insights-job-completes-with-summary
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connecthealth-patient-insights-job-completes-with-summary
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: connecthealth|StartPatientInsightsJob|2xx->2xx|$.creationTime
Issue-Key: connecthealth|StartPatientInsightsJob|2xx->2xx|$.jobId
Issue-Key: connecthealth|GetPatientInsightsJob|2xx->2xx|$.creationTime
Issue-Key: connecthealth|GetPatientInsightsJob|2xx->2xx|$.insightsOutput.uri
Issue-Key: connecthealth|GetPatientInsightsJob|2xx->2xx|$.statusDetails
Issue-Key: connecthealth|GetPatientInsightsJob|2xx->2xx|$.updatedTime
Issue-Commit: d3c46033330ced39432477d7affcf1edba1040c9

# connecthealth patient-insights job responses diverge: epoch-number timestamps, non-UUID jobId, prefix-only insightsOutput.uri and absent statusDetails

> _Note: this issue was identified against the code base at commit `d3c46033330ced39432477d7affcf1edba1040c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPatientInsightsJob

## Description
Two causes, both in the connecthealth patient-insights job surface. The emulator answers 200 for both operations but the response document it serves is not the one AWS serves: timestamp members go out as JSON numbers, the minted job id has the wrong shape, and the completed job under-reports where its output went. This issue bundles 2 causes; BOTH must be closed.

Cause 1 of 2: StartPatientInsightsJob response shape (fixture connecthealth-patient-insights-job-completes-with-summary, step task#11).
AWS: Smithy: StartPatientInsightsJob output member creationTime targets smithy.api#Timestamp with timestampFormat "date-time", so on the restJson1 wire it is an RFC3339/ISO-8601 date-time STRING, not a JSON number; jobId targets JobId (string, @length 1..36). The model constrains the id only by length, so its form is recorded behavior only: the recorded response carries a 36-character UUID-shaped id (dash-grouped 8-4-4-4-12), the same identifier form the sibling connecthealth fixture records for a listening-session id.
Emulator: creationTime is serialized as a bare JSON number of fractional epoch seconds, and jobId is a 32-character undashed lowercase-hex string.
Why: a client deserializing creationTime as a date-time string gets a type error, or silently reads a number where a timestamp was modelled; a client that validates, logs or re-parses the job id against the UUID form AWS mints rejects or mis-parses it, and the id is 32 characters wide instead of 36 everywhere it is carried onward (job ARN, output prefixes, subsequent Get calls).
Fix: the operation's output serializer must honour @timestampFormat("date-time") on this service's timestamp members instead of falling back to the restJson1 epoch-seconds default (same defect reaches every connecthealth timestamp member), and the job-id minting in this service's state must produce the UUID form rather than a 32-hex digest.
Traces: connecthealth-patient-insights-job-completes-with-summary#task#11 ($.creationTime, $.jobId).

Cause 2 of 2: GetPatientInsightsJob under-reports a SUCCEEDED job (fixture connecthealth-patient-insights-job-completes-with-summary, step task#12).
AWS: Smithy: GetPatientInsightsJob output creationTime and updatedTime are Timestamp with timestampFormat "date-time" (date-time strings, as in Cause 1); insightsOutput targets InsightsOutput whose required uri is S3Uri (@pattern ^s3://[a-z0-9][.\-a-z0-9]{1,61}[a-z0-9](/.*)?$, @length max 1024); statusDetails targets NonEmptyString and is documented "Contains information about the status of a job". The exact composition is recorded behavior only: for a job that reached the terminal success status the recorded response's insightsOutput.uri is a full OBJECT location built from the configured outputDataConfig.s3OutputPath plus output/<jobId>/<patientId>/summary.json, and statusDetails is a JSON document of the form {"documentProcessing":{"total":<n>,"processed":<n>},"processingMetadataUri":"s3://<outputBucket>/output/<jobId>/<patientId>/processing-metadata.json"} — the fixed keys are documentProcessing/total/processed/processingMetadataUri, while the bucket, job id, patient id and counts come from the request and the job's own state.
Emulator: creationTime and updatedTime are fractional-epoch JSON numbers; insightsOutput.uri stops at the directory prefix s3://<outputBucket>/output/<jobId>/, omitting the <patientId>/summary.json tail; statusDetails is absent from the body altogether.
Why: a client that takes insightsOutput.uri and reads it is handed a prefix rather than the generated summary object, so the documented "read the insights where the job says it put them" flow cannot complete; a client that reads document-processing progress or the processing-metadata location out of statusDetails finds the member missing and cannot tell a finished job's output apart from an empty one.
Fix: the job's terminal-success transition and the GetPatientInsightsJob response projection in this service's state/handler — the output-URI builder must address the per-patient summary object (not just the job prefix) and the transition must populate status_details with the processing document; timestamps share Cause 1's serializer fix.
Traces: connecthealth-patient-insights-job-completes-with-summary#task#12 ($.creationTime, $.updatedTime, $.insightsOutput.uri, $.statusDetails).

Not covered elsewhere: the open issue 20261029T202808Z-connecthealth-response-bodies-diverge-from-the-recorded-wire.md concerns ERROR bodies (message casing, DeleteDomain answering 500); both causes here are success-path response documents and neither is owned by it.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (score 87.1%, 8 mismatched steps of 62).

Cause 1 — StartPatientInsightsJob, step task#11, status 200 expected and 200 received, body diverges:
  $.creationTime: expected a date-time-shaped value, got the JSON number 1794244298.42841
  $.jobId: expected a UUID-shaped value, got 'ffb8afcd501d7012500d1edb5eb4941e' (32 hex chars, no dashes)

Cause 2 — GetPatientInsightsJob, step task#12, status 200 expected and 200 received, body diverges:
  $.creationTime: expected a date-time-shaped value, got 1794244298.42841
  $.updatedTime: expected a date-time-shaped value, got 1794244299.22841
  $.insightsOutput.uri: expected the object location '<s3OutputPath>/output/<jobId>/<patientId>/summary.json', got only '<s3OutputPath>/output/<jobId>/'
  $.statusDetails: expected the JSON document {"documentProcessing":{"total":1,"processed":1},"processingMetadataUri":"<s3OutputPath>/output/<jobId>/<patientId>/processing-metadata.json"} (counts, bucket, job id and patient id are request/state values; the keys are fixed), got None — the member was not serialized at all.

## Steps to Reproduce
1. Boot the emulator and replay fixture connecthealth-patient-insights-job-completes-with-summary from trace set simparity-wire-aws-v1.
2. Cause 1: at step task#11 the fixture POSTs /domain/{domainId}/patient-insights-job (StartPatientInsightsJob) against a live domain. Inspect the 200 body: creationTime is a JSON number instead of an RFC3339 string and jobId is 32 undashed hex characters instead of a UUID.
3. Cause 2: at step task#12 the fixture GETs /domain/{domainId}/patient-insights-job/{jobId} (GetPatientInsightsJob) after the job has reached its terminal success status. Inspect the 200 body: creationTime/updatedTime are JSON numbers, insightsOutput.uri ends at '/output/<jobId>/' with no '<patientId>/summary.json', and statusDetails is missing.
