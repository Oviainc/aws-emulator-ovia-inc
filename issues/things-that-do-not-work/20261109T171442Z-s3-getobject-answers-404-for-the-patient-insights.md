Issue-Source: trace-sweep-divergence
Issue-Trace: connecthealth-patient-insights-job-completes-with-summary
Issue-Step: task#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connecthealth-patient-insights-job-completes-with-summary
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: s3|GetObject|2xx->4xx|$.GeneratedAt
Issue-Key: s3|GetObject|2xx->4xx|$.HccIncluded
Issue-Key: s3|GetObject|2xx->4xx|$.PatientSummaries
Issue-Key: s3|GetObject|2xx->4xx|$.TotalCount
Issue-Related: 20261102T111455Z-s3-scalar-parameter-refusals-leak-smithy-coercion-wording.md
Issue-Commit: d3c46033330ced39432477d7affcf1edba1040c9

# s3 GetObject answers 404 for the patient-insights summary object a completed connecthealth job reported as its output

> _Note: this issue was identified against the code base at commit `d3c46033330ced39432477d7affcf1edba1040c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1: GetObject of the summary object a completed patient-insights job wrote answers 404 instead of serving the object (fixture connecthealth-patient-insights-job-completes-with-summary, step task#13).
AWS: Smithy: s3#GetObject binds GET /{Bucket}/{Key+} with success status 200; Body is the @httpPayload streaming blob and the object's metadata rides on headers (accept-ranges, Last-Modified, ETag, Content-Type, x-amz-checksum-crc32, x-amz-checksum-type, x-amz-server-side-encryption). The only modelled errors are NoSuchKey (HTTP 404) and InvalidObjectState (HTTP 403), so 404 is the answer for a key that is not in the bucket and 200 the answer for one that is. Recorded behavior: after the connecthealth patient-insights job reaches its terminal success status, the summary object exists at the location the job reports, and GetObject on it answers 200 with the summary JSON document (PatientSummaries[].PatientSummary carrying JobId, SummaryType, PatientId, GeneratedAt and the Sections narrative, alongside TotalCount, GeneratedAt and HccIncluded) plus accept-ranges: bytes, x-amz-checksum-type: FULL_OBJECT and the Last-Modified/ETag/checksum/content-type/server-side-encryption headers.
Emulator: the same GetObject answers 404 with none of the body and none of those headers — the key is simply absent from the output bucket, so the NoSuchKey path is taken.
Hypothesis: nothing ever writes the object. The connecthealth patient-insights job transitions to its terminal success status without materialising its summary (and processing-metadata) object into the bucket named by outputDataConfig.s3OutputPath, so s3 has no key to serve; this is the same gap that leaves that job's insightsOutput.uri a bare prefix.
Why: a client follows the documented flow — start the job, poll until it succeeds, then GetObject the reported output — and gets 404 where AWS returns the generated summary. The insights are unreachable through the emulator, and a client cannot distinguish "job produced nothing" from "job output was never stored".
Fix: the cross-service materialisation of the job's output — whatever writes the job's result object into the configured output bucket at job completion — must create the summary object (and its processing-metadata sibling) so s3's GetObject handler finds the key and serves it with the modelled 200 and object-metadata headers.
Traces: connecthealth-patient-insights-job-completes-with-summary#task#13.

Not covered elsewhere: no open issue carries this replay key — the open connecthealth issues concern error-body wording, UntagResource and authorizer status, none of them the stored job output.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (score 87.1%, 8 mismatched steps of 62).

Step task#13, GetObject of the completed job's summary object:
  status: expected 200, got 404
  $.PatientSummaries: missing — expected one PatientSummary element carrying JobId, SummaryType ('Pre-visit'), PatientId, GeneratedAt and the Sections clinical narrative; got None
  $.TotalCount: missing (expected 1, got None)
  $.GeneratedAt: missing (expected a date-time-shaped value, got None)
  $.HccIncluded: missing (expected false, got None)
  $headers.accept-ranges: missing (expected 'bytes')
  $headers.x-amz-checksum-type: missing (expected 'FULL_OBJECT')
  $headers.last-modified, $headers.etag, $headers.x-amz-checksum-crc32, $headers.content-type, $headers.x-amz-server-side-encryption: all missing

The object key the step requests is the one the preceding GetPatientInsightsJob step reports as the job's output location (output/<jobId>/<patientId>/summary.json under the configured output bucket); the bucket, job id and patient id are request/state values, the 'output/.../summary.json' layout is the fixed part.

## Steps to Reproduce
1. Boot the emulator and replay fixture connecthealth-patient-insights-job-completes-with-summary from trace set simparity-wire-aws-v1.
2. Let the fixture start a patient-insights job and poll it to its terminal success status (steps task#11, task#12).
3. At step task#13 the fixture issues GET /<outputBucket>/output/<jobId>/<patientId>/summary.json (s3 GetObject) for the job's reported output object.
4. Observe 404 with an empty body and no object-metadata headers, where the recording answers 200 with the summary JSON document and the accept-ranges/etag/last-modified/checksum/content-type headers.
