Issue-Source: trace-sweep-divergence
Issue-Trace: emr-serverless-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-serverless-error-paths emr-serverless-list-applications-maxresults-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless|GetApplication|4xx->4xx|$.message
Issue-Key: emr-serverless|StartJobRun|4xx->4xx|$.message
Issue-Key: emr-serverless|CancelJobRun|4xx->4xx|$.message
Issue-Key: emr-serverless|UntagResource|4xx->4xx|$.message
Issue-Key: emr-serverless|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: emr-serverless|ListApplications|4xx->4xx|$.message
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# emr-serverless rejection messages are built from the request instead of AWS's text

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication

## Description
Five causes with one shared fault: the emulator synthesises every rejection body out of the request it just received, so each error message differs from the fixed text AWS's recorded responses carry. The HTTP status is right in all five; the ResourceNotFoundException / ValidationException message rule is not. Each cause is a separate replay failure and all five must be closed.

Cause 1 of 5: GetApplication not-found message echoes the requested application id.
AWS: Smithy: GetApplication errors ResourceNotFoundException (HTTP 404), whose `message` is @required (String1024, length 1..1024). Recorded behavior: an unknown application id is refused with 404 and the fixed message 'Application does not exist' — AWS does not echo the caller's identifier back.
Emulator: 404 (correct) with 'Application 00fakeidnonexistent0000000000000 does not exist' — the id is interpolated into the message.
Why: the message is reflected caller input, so a client that matches on the not-found text (or a recorded-response assertion) never matches, and the emulator reflects arbitrary caller-supplied strings into its error bodies.
Fix: the not-found payload builder in the service state (crate::state::application_not_found), which formats the id into the message rather than emitting the fixed phrase.
Traces: emr-serverless-error-paths#task#2; services/service-com-amazonaws-emrserverless-awstoledowebservice/src/state.rs:501-506.

Cause 2 of 5: StartJobRun not-found message echoes the requested application id.
AWS: Smithy: StartJobRun POST /applications/{applicationId}/jobruns, errors ResourceNotFoundException 404 / ValidationException 400 / ConflictException 409. Recorded behavior: submitting to an unknown application is refused with 404 and the same fixed message 'Application does not exist'.
Emulator: 404 (correct) with 'Application 00fakeidnonexistent0000000000000 does not exist'.
Why: same as Cause 1 — the identifier-echoing message makes the not-found body unmatched by clients and leaks request input into the error channel.
Fix: same shared not-found payload builder in the service state, reached from StartJobRun's handler.
Traces: emr-serverless-error-paths#task#4; services/service-com-amazonaws-emrserverless-awstoledowebservice/src/state.rs:501-506.

Cause 3 of 5: CancelJobRun not-found message is a different sentence and echoes both ids.
AWS: Smithy: CancelJobRun DELETE /applications/{applicationId}/jobruns/{jobRunId}, errors ResourceNotFoundException 404 / ValidationException 400. Recorded behavior: an unknown job run is refused with 404 and the fixed message "Job Run with given ID doesn't exist" — the job-run not-found text is its own phrase and names no identifier.
Emulator: 404 (correct) with 'Job run 00g8i8lkpddpt600 does not exist in application 0000000000000001', which echoes the requested job-run id and the application id.
Why: a client distinguishing an unknown job run from an unknown application by the error text sees neither AWS phrase, and the emulator again reflects caller input (and its own internal application id) into the body.
Fix: the job-run not-found payload builder in the service state (crate::state::job_run_not_found).
Traces: emr-serverless-error-paths#task#5; services/service-com-amazonaws-emrserverless-awstoledowebservice/src/state.rs:508-514.

Cause 4 of 5: UntagResource reports a bad resource ARN as a raw pattern-constraint failure instead of AWS's ARN vendor check.
AWS: Smithy: UntagResource DELETE /tags/{resourceArn}; `resourceArn` is @required with length 60..1024 and pattern ^arn:(aws[a-zA-Z0-9-]*):emr-serverless:.+:(\d{12}):\/applications\/[0-9a-zA-Z]+(\/jobruns\/[0-9a-zA-Z]+|\/sessions\/[0-9a-zA-Z]+)?$; errors ValidationException 400 / ResourceNotFoundException 404. Recorded behavior: an ARN belonging to another service is refused with 400 and a message naming the expected vendor — 'Invalid input resource arn: expected vendor emr-serverless'. AWS performs a resource-ARN vendor check and never surfaces the member's regex.
Emulator: 400 (correct status) but renders the generic boundary-constraint message "1 validation error detected: Value 'arn:aws:s3:::…' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws…" and the replay also flags x-amzn-errortype as a shape mismatch against the recorded response.
Why: the body echoes the caller's whole ARN and the service's internal regex instead of stating which vendor was expected, so a client cannot tell a wrong-service ARN from a malformed one, and the emulator exposes model internals in a customer-visible message.
Fix: UntagResource's input validation — run the resource-ARN vendor check ahead of the generic pattern constraint and emit the vendor-named ValidationException (and the matching error-type header) from the operation's error channel.
Traces: emr-serverless-error-paths#task#7.

Cause 5 of 5: ListApplications maxResults range rejection echoes the offending value.
AWS: Smithy: ListApplications `maxResults` is @httpQuery("maxResults") with @range(min 1, max 50) and no default; error ValidationException 400. Recorded behavior: an out-of-range maxResults is refused with 400 and the message "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1" — the member and its constraint are named, the submitted value is not quoted.
Emulator: 400 (correct) with "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1" — the value is interpolated into the message.
Why: the emulator's range-violation renderer differs from AWS's for every numeric member, so clients and recorded-response assertions built on AWS's wording break, and the divergence applies service-wide rather than to this one request.
Fix: the shared range-constraint message renderer used by ListApplications' input validation — omit the value for a range violation, as AWS does, while keeping the member name and constraint text.
Traces: emr-serverless-list-applications-maxresults-out-of-range-rejected#task#3 (cluster also covers task#4 of the same fixture).

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 21.7%, 18 mismatched of 23 steps). Status matched in all five causes; the message (and for Cause 4 the error-type header) diverged:

Cause 1 — emr-serverless-error-paths#task#2 GetApplication, 404 -> 404: $.message value_mismatch (expected 'Application does not exist', got 'Application 00fakeidnonexistent0000000000000 does not exist').

Cause 2 — emr-serverless-error-paths#task#4 StartJobRun, 404 -> 404: $.message value_mismatch (expected 'Application does not exist', got 'Application 00fakeidnonexistent0000000000000 does not exist').

Cause 3 — emr-serverless-error-paths#task#5 CancelJobRun, 404 -> 404: $.message value_mismatch (expected "Job Run with given ID doesn't exist", got 'Job run 00g8i8lkpddpt600 does not exist in application 0000000000000001').

Cause 4 — emr-serverless-error-paths#task#7 UntagResource, 400 -> 400: $.message value_mismatch (expected 'Invalid input resource arn: expected vendor emr-serverless', got "1 validation error detected: Value 'arn:aws:s3:::xxxx…' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws[a-zA-Z0-9-]*):emr-serverless:…"); $headers.x-amzn-errortype shape_mismatch (got 'ValidationException').

Cause 5 — emr-serverless-list-applications-maxresults-out-of-range-rejected#task#3 ListApplications, 400 -> 400: $.message value_mismatch (expected "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1", got "1 validation error detected: Value '0' at 'maxResults' failed …"); same cluster covers task#4.

Source of the echoing in the checkout: state.rs:502 `format!("Application {application_id} does not exist")` and state.rs:511 `format!("Job run {job_run_id} does not exist in application {application_id}")`. state_reset_failures was 0 for this run.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay the wire corpus (replay_wire_corpus).
2. Cause 1: fixture emr-serverless-error-paths step task#2 — GET /applications/<unknown-id>. Emulator returns 404 with the id interpolated into the message; AWS returns 404 'Application does not exist'.
3. Cause 2: same fixture step task#4 — POST /applications/<unknown-id>/jobruns. Same echoing message instead of 'Application does not exist'.
4. Cause 3: same fixture step task#5 — DELETE /applications/<id>/jobruns/<unknown-run-id>. Emulator returns 404 'Job run <id> does not exist in application <appId>'; AWS returns 404 "Job Run with given ID doesn't exist".
5. Cause 4: same fixture step task#7 — DELETE /tags/<an ARN whose vendor is not emr-serverless>?tagKeys=... Emulator returns 400 with the boundary pattern-constraint message quoting the ARN and the regex; AWS returns 400 'Invalid input resource arn: expected vendor emr-serverless'.
6. Cause 5: fixture emr-serverless-list-applications-maxresults-out-of-range-rejected steps task#3 and task#4 — GET /applications?maxResults=0 (and the above-range variant). Emulator's 400 message quotes the submitted value; AWS's omits it.
