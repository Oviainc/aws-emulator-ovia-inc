Issue-Source: trace-sweep-divergence
Issue-Trace: comprehendmedical-describe-job-malformed-jobid-rejected
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehendmedical-describe-job-malformed-jobid-rejected comprehendmedical-phi-detection-job-family
Issue-Shape-Id: com.amazonaws.comprehendmedical#ComprehendMedical_20181030
Issue-Key: comprehendmedical|DescribePHIDetectionJob|4xx->4xx|status
Issue-Related: 20261029T172356Z-comprehendmedical-asynchronous-job-error-contract-diverges-from-aws.md
Issue-Commit: cbdd761a1e2581410b5e098320f05dffad4febd1

# ComprehendMedical DescribePHIDetectionJob refuses an unknown JobId with the wrong status, error class and message

> _Note: this issue was identified against the code base at commit `cbdd761a1e2581410b5e098320f05dffad4febd1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePHIDetectionJob

## Description
One net-new wire divergence cluster (2 replayed steps) in the IMPLEMENTED ComprehendMedical PHI job family: the unknown-job branch of DescribePHIDetectionJob answers a different status class, a different modeled error shape and a different message than the recordings. The handler exists and projects real jobs correctly, so this is a fidelity gap, not a stub gap.

Cause 1 of 1: DescribePHIDetectionJob's absent-job refusal diverges in status, error class and message text.
AWS: Smithy (services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json) models DescribePHIDetectionJob input JobId as required, @length 1..32, @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$, output ComprehendMedicalAsyncJobProperties, errors InternalServerException/500, InvalidRequestException/400, ResourceNotFoundException/404, TooManyRequestsException/429 — so both a 400 and a 404 refusal are expressible as modeled. Recorded response for a JobId that satisfies the pattern and the length but names no job in the account: HTTP 400 with Message 'Could not find specified job.'. The whole of that message is FIXED text for this error class: it has NO variable part, and in particular the requested JobId is NOT echoed into it. The 400 status pairs with the InvalidRequestException class, the same refusal the recordings show for the sibling entities-detection job family's unknown-JobId describe/stop paths, so the absent-job condition is reported as an invalid request rather than as a missing resource.
Emulator: HTTP 404 with the ResourceNotFoundException shape and Message "No PHI detection job was found for JobId '<the requested jobId>'." — a sentence whose fixed text is emulator-authored and whose only variable part is the request's JobId interpolated into it. The miss branch at services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/describe_phi_detection_job.rs:165-177 formats that message from input.job_id and returns ResourceNotFoundException whenever state.phi_detection_jobs.jobs has no entry for the id (including after expire_jobs() has aged a job out).
Why: the status class and the SDK exception type both change, so a client's error handling branches differently against the emulator than against AWS — code catching InvalidRequestException (or treating 400 as a non-retryable request fault) never fires, while a ResourceNotFoundException handler fires where AWS raises none, and retry/backoff classification keyed on the status class diverges. A caller or log scraper matching AWS's documented sentence 'Could not find specified job.' never matches, and the emulator additionally leaks the requested JobId into a message AWS keeps constant, so the two negative classes (malformed id vs absent job) become distinguishable by text that does not exist on AWS.
Fix: the unknown/expired-job branch of DescribePHIDetectionJob's handler (src/ops/describe_phi_detection_job.rs) must raise the 400 InvalidRequestException refusal with the fixed message 'Could not find specified job.' and no interpolation. The sibling PHI stop/list paths and the entities-detection family use the same interpolated ResourceNotFoundException pattern, so fix them together to stop the paths drifting.
Traces: comprehendmedical-describe-job-malformed-jobid-rejected#task#2 (representative) and comprehendmedical-phi-detection-job-family#task#2, the same cluster.

Related open issues, none of which owns this step: 20261029T172356Z-comprehendmedical-asynchronous-job-error-contract-diverges-from-aws.md files the identical refusal for the ENTITIES-detection family (DescribeEntitiesDetectionV2Job/StopEntitiesDetectionV2Job) and, for DescribePHIDetectionJob, only the pattern-violation empty-Message defect at task#1 of this same fixture — not this absent-job step. 20261101T172029Z-comprehendmedical-phi-batch-jobs-can-never-succeed-every.md describes the message text from the BEHAVIORAL suite and asserts the ResourceNotFoundException code/404 status are correct; the wire recording replayed here shows the status and error class diverge too, and that issue carries no wire fixture/step key, so this step was reported unowned. 20261023T185154Z-comprehendmedical-the-phi-asynchronous-job-family-is-still.md and 20261029T175936Z-comprehendmedical-phi-job-handlers-answer-501-notimplemented-on.md both describe the family answering HTTP 501, which it no longer does.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, /__admin/health 200): 12 divergence clusters, all service=comprehendmedical. Eleven carry an owning issue; this is the only cluster with verdict=mismatch, stub_path empty AND owned_by empty.

Cluster: service=comprehendmedical | DescribePHIDetectionJob | status=400->404 | keys=$.Message,status
  representative: comprehendmedical-describe-job-malformed-jobid-rejected#task#2
  also affected: comprehendmedical-phi-detection-job-family#task#2 (affected_count=2)
  detail: "status: expected 400, got 404\n$.Message: value_mismatch (expected 'Could not find specified job.', got \"No PHI detection job was found for JobId 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'.\")"

Expected (recorded): HTTP 400, Message 'Could not find specified job.' — entirely fixed text.
Actual (emulator): HTTP 404, ResourceNotFoundException, Message "No PHI detection job was found for JobId '<jobId>'." — the 32-'a' id in the recorded detail is the fixture's own input value filling the emulator's template, not part of any contract.

Source: services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/describe_phi_detection_job.rs:165-177 (lookup miss -> ResourceNotFoundException with the format!-interpolated JobId message).
Model grounding: smithy_query on services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json — DescribePHIDetectionJob errors include InvalidRequestException with httpError 400, so the recorded 400 refusal is modeled and expressible.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an AWS CLI/SDK comprehendmedical client at it.
2. Call `aws comprehendmedical describe-phi-detection-job --job-id aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa` (32 characters: satisfies the modeled @length 1..32 and @pattern, names no job).
   Expected: HTTP 400 with Message exactly `Could not find specified job.` (fixed text; the JobId is NOT echoed).
   Actual: HTTP 404 ResourceNotFoundException with Message "No PHI detection job was found for JobId 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'.".
3. Same divergence on a job that has aged out: start a PHI job, let expire_jobs() age its metadata out, then describe it.
4. Keep the control that already matches: `--job-id 'not#a#valid#job#id!'` must stay a 400 pattern-violation refusal (the Message of that refusal is a separate, already-filed defect).
5. Re-replay fixture comprehendmedical-describe-job-malformed-jobid-rejected step task#2 and comprehendmedical-phi-detection-job-family step task#2 to confirm the fix.
