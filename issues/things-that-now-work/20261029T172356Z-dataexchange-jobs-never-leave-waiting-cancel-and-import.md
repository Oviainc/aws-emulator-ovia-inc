Issue-Source: trace-sweep-divergence
Issue-Trace: dataexchange-job-cancel-before-start
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dataexchange-job-cancel-before-start dataexchange-job-error-paths dataexchange-job-import-signed-url-lifecycle
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange|GetJob|2xx->2xx|$.State
Issue-Key: dataexchange|GetJob|2xx->2xx|$
Issue-Key: dataexchange|GetJob|2xx->2xx|$.Errors
Issue-Related: 20261029T172356Z-dataexchange-serializes-every-date-time-member-as-an.md
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# DataExchange jobs never leave WAITING: cancel and import never reach a terminal state

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJob

## Description
The emulator's job record is created in State WAITING and nothing ever moves it: neither an explicit cancel nor the execution of a started job advances it, and the Errors list is never populated. Two causes.

Cause 1 of 2: CancelJob does not move the job to CANCELLED. AWS: CancelJob (DELETE /v1/jobs/{JobId}, 204, errors ConflictException 409 / ResourceNotFoundException 404) cancels a job that has not started, and a subsequent GetJob reports State 'CANCELLED' with UpdatedAt advanced to the cancellation instant (Smithy: State enum = WAITING | IN_PROGRESS | ERROR | COMPLETED | CANCELLED | TIMED_OUT; GetJob output State/UpdatedAt). Emulator: the cancel is accepted but the following GetJob still answers 200 with State 'WAITING' and an UpdatedAt unchanged from creation. Why: a client that cancels a job and then polls for a terminal state polls forever; worse, the job still looks startable, so cancel is silently a no-op and cancel-then-recreate flows double-run. Fix: CancelJob's handler and the job state transition in dataexchange job state (set State=CANCELLED and advance UpdatedAt; reject cancel of an already-running job with ConflictException). Traces: dataexchange-job-cancel-before-start#task#6, dataexchange-job-error-paths#task#9.

Cause 2 of 2: a started IMPORT_ASSET_FROM_SIGNED_URL job never reaches a terminal state and never reports its error. AWS: after StartJob, the job executes and settles in one of the terminal states (COMPLETED, ERROR, CANCELLED, TIMED_OUT); when the import's source cannot be fetched it settles State 'ERROR' and GetJob returns the Errors list with one JobError whose Code is the fixed value 'RESOURCE_NOT_FOUND_EXCEPTION' and whose Message is the fixed text 'Resource not found. Unable to find source Amazon S3 object.' (Smithy: GetJob output Errors -> ListOfJobError; JobError { Code required, Message required, Details, ResourceId, ResourceType }). Emulator: GetJob keeps answering 200 with State 'WAITING' and no Errors member at all — the replay's poll for a terminal state gave up after 61 attempts / 60s. Why: every client-side wait-for-job loop (and every SDK waiter) hangs instead of completing or failing, and the failure reason is never surfaced, so a caller cannot distinguish a slow job from a broken one. Fix: the dataexchange job execution/state-advance path behind StartJob (drive WAITING -> IN_PROGRESS -> terminal) plus GetJob's serialization of the Errors list. Traces: dataexchange-job-import-signed-url-lifecycle#task#6.

(The timestamp members in both responses are also wrong-typed, and the signed URL's host is environment-specific; those are not part of this issue — the encoding defect is filed separately as the date-time serialization issue.)

## Evidence
Cause 1 — GetJob after CancelJob, expected 200 / actual 200: $.State value_mismatch (expected 'CANCELLED', got 'WAITING'); $.UpdatedAt did not advance past the creation instant (expected a later instant than CreatedAt, got the CreatedAt value). Smithy: CancelJob -> DELETE /v1/jobs/{JobId} code 204; State enum includes CANCELLED.

Cause 2 — GetJob while polling a started import job, expected 200 / actual 200: '$: poll capped at 60s'; '$.State: poll: not in [COMPLETED, ERROR, TIMED_OUT, CANCELLED] after 61 attempts'; $.State value_mismatch (expected 'ERROR', got 'WAITING'); $.Errors missing (expected [{'Code': 'RESOURCE_NOT_FOUND_EXCEPTION', 'Message': 'Resource not found. Unable to find source Amazon S3 object.'}], got None). The Code and Message above are the fixed text of that error class; the job/asset/revision ids in the surrounding body are per-run values.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: CreateJob (Type IMPORT_ASSET_FROM_SIGNED_URL), then DELETE /v1/jobs/{JobId} to cancel it before starting, then GET /v1/jobs/{JobId} (dataexchange-job-cancel-before-start#task#6; also dataexchange-job-error-paths#task#9). State is still 'WAITING'; it should be 'CANCELLED' with UpdatedAt advanced.
3. Cause 2: CreateJob + StartJob for an IMPORT_ASSET_FROM_SIGNED_URL whose source object does not exist, then poll GET /v1/jobs/{JobId} (dataexchange-job-import-signed-url-lifecycle#task#6). The job stays 'WAITING' past 60s with no Errors member; it should settle State 'ERROR' with Errors[0].Code 'RESOURCE_NOT_FOUND_EXCEPTION' and Errors[0].Message 'Resource not found. Unable to find source Amazon S3 object.'.
