Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-serverless.delete-application-removes-from-list emr-serverless.start-stop-application-lifecycle emr-serverless.delete-application-while-started-rejected emr-serverless.delete-started-application-rejected
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless.delete-application-removes-from-list|run#14|AssertionError|assert message == (
Issue-Key: emr-serverless.start-stop-application-lifecycle|run#19|AssertionError|assert message == (
Issue-Key: emr-serverless.delete-application-while-started-rejected|run#10|AssertionError|assert message == (
Issue-Key: emr-serverless.delete-started-application-rejected|run#11|AssertionError|assert message == (
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# EMR Serverless application-lifecycle refusals carry emulator-authored messages instead of AWS's StartApplication/DeleteApplication status-gate sentences (2 causes)

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartApplication

## Description
Two distinct root causes, both in the EMR Serverless application-lifecycle status gates. The HTTP status (400) and the error code (ValidationException) are right in both; the MESSAGE text is emulator-authored and does not match what AWS returns, so a client that matches on the refusal sentence breaks.

Cause 1 of 2: StartApplication's non-startable-state refusal message.
AWS: StartApplication is modelled with ValidationException (HTTP 400) — Smithy: StartApplication errors = {InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ValidationException 400}; input applicationId is the only member (@httpLabel, len 1..64, pattern ^[0-9a-z]+$). The model does not state the refusal wording, so: recorded behavior only — when the application is not in a startable status (e.g. after DeleteApplication has left it TERMINATED), AWS answers 400 ValidationException with the message "Application cannot be STARTED. Applications must be in one of the following statuses: [CREATED, STOPPED]". Fixed text of that template: "Application cannot be STARTED. Applications must be in one of the following statuses: " followed by the bracketed list of allowed statuses; the only variable part is the ORDER in which AWS renders the two allowed statuses (the recordings show both "[CREATED, STOPPED]" and "[STOPPED, CREATED]", so either order is acceptable — the rest is exact). The sentence names no application id.
Emulator: returns 400 ValidationException with the message "Application is in TERMINATED state and cannot be started" (services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/start_application.rs lines 159-167). It is a different sentence, it is keyed on the current state rather than on the allowed-status list, and it only exists for TERMINATED — any other non-startable settled state falls into the `_` arm and the start is ACCEPTED instead of refused.
Why: a client (or a retry/err-classification layer) that reads AWS's "Applications must be in one of the following statuses: [...]" sentence to discover which statuses would permit the call finds neither that phrase nor the status list, and cannot distinguish 'wrong lifecycle state' from any other ValidationException. Worse, for non-startable states other than TERMINATED the emulator does not refuse at all, so an invalid start looks successful.
Fix: the StartApplication handler's state gate and its ValidationException message construction — refuse from a single allowed-status set and render AWS's sentence from that set.
Traces: emr-serverless.delete-application-removes-from-list, emr-serverless.start-stop-application-lifecycle (both reach the gate by deleting the application, then calling StartApplication on the TERMINATED id; both fail asserting the message, actual "Application is in TERMINATED state and cannot be started").

Cause 2 of 2: DeleteApplication's non-deletable-state refusal message.
AWS: DeleteApplication is modelled with ValidationException (HTTP 400) — Smithy: DeleteApplication errors = {InternalServerException 500, ResourceNotFoundException 404, ValidationException 400}; its doc states the precondition "An application has to be in a stopped or created state in order to be deleted." The model does not state the refusal wording, so: recorded behavior only — deleting a STARTED application gives 400 ValidationException with "Application <applicationId> must be in of the following statuses [CREATED, STOPPED]. Current status: STARTED". Template: fixed text "Application ", then the applicationId FROM THE REQUEST, then " must be in of the following statuses " (AWS's own wording, including the missing word after "in"), then the bracketed allowed-status list (order may be "[CREATED, STOPPED]" or "[STOPPED, CREATED]"), then ". Current status: " and the application's CURRENT settled status from state. The id, the status-list order and the current status are the only variable parts.
Emulator: returns 400 ValidationException with "Application is in STARTED state; an application has to be in CREATED or STOPPED state in order to be deleted" (services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/delete_application.rs lines 158-168). The sentence is reworded, omits the applicationId entirely, prints the allowed statuses as prose rather than AWS's bracketed list, and drops the trailing ". Current status: <status>" clause.
Why: a client that parses the refusal to report which application it was and what status blocked the delete gets neither the id nor AWS's "Current status:" clause, and an exact-match assertion on AWS's sentence fails even though the lifecycle decision itself was correct.
Fix: the DeleteApplication handler's deletable-state gate and its ValidationException message construction.
Traces: emr-serverless.delete-application-while-started-rejected, emr-serverless.delete-started-application-rejected (both start the application, wait for STARTED, call DeleteApplication and assert the message; both fail with actual "Application is in STARTED state; an application has to be in CREATED or STOPPED state in order to be deleted").

## Evidence
Cause 1 — StartApplication on a deleted (TERMINATED) application:
  Expected (AWS): HTTP 400, code ValidationException, message "Application cannot be STARTED. Applications must be in one of the following statuses: [CREATED, STOPPED]" (the two statuses may appear in either order).
  Actual (emulator): HTTP 400, code ValidationException, message "Application is in TERMINATED state and cannot be started".
  Failing assertions: check_emr_serverless_delete_application_removes_from_list.py line 53 and check_emr_serverless_start_stop_application_lifecycle.py line 51, both AssertionError: Application is in TERMINATED state and cannot be started.
  Source: services/.../src/ops/start_application.rs:162 hardcodes that sentence, and only for the "TERMINATED" arm; all other non-startable states fall through to the accepting `_` arm at line 172.

Cause 2 — DeleteApplication on a STARTED application:
  Expected (AWS): HTTP 400, code ValidationException, message "Application <applicationId> must be in of the following statuses [CREATED, STOPPED]. Current status: STARTED" (statuses in either order; <applicationId> is the id from the request; the current status comes from state).
  Actual (emulator): HTTP 400, code ValidationException, message "Application is in STARTED state; an application has to be in CREATED or STOPPED state in order to be deleted".
  Failing assertions: check_emr_serverless_delete_application_while_started_rejected.py line 42 and check_emr_serverless_delete_started_application_rejected.py line 40, both AssertionError: Application is in STARTED state; an application has to be in CREATED or STOPPED state in order to be deleted.
  Source: services/.../src/ops/delete_application.rs:161-164 builds that sentence from the settled state and never includes the applicationId or the "Current status:" clause.

In both causes the status and error code already match AWS; the divergence is confined to the message text (and, for cause 1, to the set of states that are refused at all).

## Steps to Reproduce
Cause 1 (StartApplication status gate):
1. CreateApplication(name=<unique>, releaseLabel="emr-7.0.0", type="SPARK", clientToken=<unique>) and wait for GetApplication state CREATED.
2. StartApplication, wait for STARTED; StopApplication, wait for STOPPED; DeleteApplication, wait for GetApplication state TERMINATED (the record is soft-deleted and leaves ListApplications).
3. Call StartApplication(applicationId=<that id>) again.
4. Observe 400 ValidationException with message "Application is in TERMINATED state and cannot be started"; AWS answers "Application cannot be STARTED. Applications must be in one of the following statuses: [CREATED, STOPPED]".

Cause 2 (DeleteApplication status gate):
1. CreateApplication as above and wait for CREATED.
2. StartApplication(applicationId=<id>) and poll GetApplication until state is STARTED.
3. Call DeleteApplication(applicationId=<id>).
4. Observe 400 ValidationException with message "Application is in STARTED state; an application has to be in CREATED or STOPPED state in order to be deleted"; AWS answers "Application <id> must be in of the following statuses [CREATED, STOPPED]. Current status: STARTED". The application is correctly left STARTED in both cases.

Replay: run_behavioral_suite and inspect the four checks listed in trace_ids.
