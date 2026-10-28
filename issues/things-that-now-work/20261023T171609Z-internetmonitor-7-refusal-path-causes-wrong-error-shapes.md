Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: internetmonitor.create-monitor-duplicate-name-rejected internetmonitor.create-monitor-traffic-percentage-out-of-range-rejected internetmonitor.tag-resource-exceeds-tag-limit-rejected internetmonitor.start-query-measurements-reaches-succeeded internetmonitor.get-query-results-schema-for-empty-monitor internetmonitor.delete-monitor-removes-monitor internetmonitor.update-monitor-nonexistent-monitor-rejected internetmonitor.get-query-status-wrong-monitor-rejected internetmonitor.stop-query-leaves-terminal-status
Issue-Shape-Id: com.amazonaws.internetmonitor#InternetMonitor20210603
Issue-Key: internetmonitor.create-monitor-duplicate-name-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: internetmonitor.create-monitor-traffic-percentage-out-of-range-rejected|run#5|AssertionError|assert code == "BadRequestException", f"CODE={code!r}"
Issue-Key: internetmonitor.tag-resource-exceeds-tag-limit-rejected|run#8|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: internetmonitor.start-query-measurements-reaches-succeeded|run#10|AssertionError|assert raised == "BadRequestException", f"expected BadRequestException, got: {raised}"
Issue-Key: internetmonitor.get-query-results-schema-for-empty-monitor|run#8|AssertionError|assert code == "BadRequestException", (
Issue-Key: internetmonitor.delete-monitor-removes-monitor|run#20|AssertionError|assert code == "ResourceNotFoundException", f"GetMonitor on deleted monitor returned unexpected code: {code}"
Issue-Key: internetmonitor.update-monitor-nonexistent-monitor-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: internetmonitor.get-query-status-wrong-monitor-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException for bogus QueryId, got: {code}"
Issue-Key: internetmonitor.stop-query-leaves-terminal-status|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException, got: {code}"
Issue-Commit: 326b33f63f33ac22c419b7169b10d47cfd8c011f

# internetmonitor: 7 refusal-path causes — wrong error shapes (ValidationException for BadRequestException / ResourceNotFoundException), non-AWS refusal message templates, and GetQueryStatus/StopQuery answering 501

> _Note: this issue was identified against the code base at commit `326b33f63f33ac22c419b7169b10d47cfd8c011f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMonitor, GetMonitor, UpdateMonitor, TagResource, StartQuery, GetQueryStatus, StopQuery

## Description
The internetmonitor emulator serves its happy paths but diverges from AWS on seven distinct refusal/not-found paths. All seven are in one service and are reported as one bundled issue; a fixer must close ALL SEVEN causes below before submitting.

Model consulted via smithy_query on services/service-com-amazonaws-internetmonitor-internetmonitor20210603/smithy-model/com-amazonaws-internetmonitor-internetmonitor20210603.json.

Cause 1 of 7: CreateMonitor duplicate-name conflict message is not AWS's request-derived template.
AWS: Smithy models ConflictException (HTTP 409) on CreateMonitor. Recorded behavior: the message is built from the request and names the rejected MonitorName, in the form 'Monitor with name <MonitorName> already exists' (no trailing period). The rule — message = "Monitor with name " + the MonitorName submitted + " already exists" — is what is required, not any one recorded name.
Emulator: the code is correct (ConflictException) but the message uses a different template: e.g. 'The monitor sim2real-im-dup-06cfc9437863 already exists.' for name sim2real-im-dup-06cfc9437863.
Why: clients and test suites that match the documented/recorded conflict text (or parse the name out of it) do not recognize the conflict.
Fix: the ConflictException construction in the CreateMonitor handler.
Traces: internetmonitor.create-monitor-duplicate-name-rejected.

Cause 2 of 7: CreateMonitor with TrafficPercentageToMonitor out of range is refused with the wrong error shape.
AWS: Smithy: CreateMonitor.TrafficPercentageToMonitor @range min 1, max 100; CreateMonitor's modeled errors are AccessDeniedException/ConflictException/InternalServerException/LimitExceededException/ThrottlingException/ValidationException — BadRequestException is NOT modeled on this operation. Recorded behavior only (measured on real internetmonitor, us-west-2): the out-of-range value is refused with HTTP 400 error code BadRequestException, message 'Invalid request body' (internetmonitor does not use the ValidationException shape for this class of error on this operation), and no monitor is created.
Emulator: refuses the request, but with error code ValidationException instead of BadRequestException.
Why: a client that branches on the error code (retry/report logic, or a catch of BadRequestException) sees an unexpected code and takes the wrong branch; the refusal itself is correct, the classification is not.
Fix: the error-shape selection in CreateMonitor's input validation (the handler/validation layer must emit the BadRequestException shape for request-body validation failures, not the generic scaffold validation error).
Traces: internetmonitor.create-monitor-traffic-percentage-out-of-range-rejected.

Cause 3 of 7: TagResource over the tag limit is refused with the wrong error shape.
AWS: Smithy: TagResource.Tags targets TagMap with @length min 0, max 200, and TagResource's modeled errors DO include BadRequestException (HTTP 400) alongside NotFoundException (404)/AccessDeniedException/TooManyRequestsException/InternalServerErrorException. A request carrying 201 tag entries is therefore refused with 400 BadRequestException (recorded message example: 'Invalid request body'), leaving the resource's existing tags untouched.
Emulator: refuses it with error code ValidationException — a shape TagResource does not model.
Why: the error the client is told to expect for a malformed tag set (BadRequestException) is never produced, so code catching it does not catch this failure at all.
Fix: the TagResource handler's tag-count validation, emitting the modeled BadRequestException.
Traces: internetmonitor.tag-resource-exceeds-tag-limit-rejected.

Cause 4 of 7: StartQuery against a monitor with zero monitored resources is refused with the wrong error shape.
AWS: Smithy models StartQuery errors as AccessDeniedException/InternalServerException/LimitExceededException/ThrottlingException/ValidationException. Recorded behavior only (measured on real internetmonitor): StartQuery against a monitor whose Resources is empty — for every QueryType, not just one — is refused with HTTP 400 error code BadRequestException, message example 'Invalid request body'; there is no query lifecycle (no QUEUED/RUNNING/SUCCEEDED) and the monitor's own ACTIVE state is untouched.
Emulator: refuses StartQuery, but with error code ValidationException instead of BadRequestException.
Why: callers distinguishing "this monitor has no queryable data" (BadRequestException) from an input-syntax problem cannot do so, and never reach the downstream GetQueryResults/GetQueryStatus assertions.
Fix: the error-shape selection in the StartQuery handler's resourceless-monitor rejection path.
Traces: internetmonitor.start-query-measurements-reaches-succeeded, internetmonitor.get-query-results-schema-for-empty-monitor.

Cause 5 of 7: a read of a deleted (nonexistent) monitor answers ValidationException instead of a not-found error.
AWS: recorded behavior only — GetMonitor for a monitor that has been deleted is refused as a not-found condition: error code ResourceNotFoundException with a request-derived message of the form 'No resource found for account <accountId> and resource monitor/<MonitorName>'. (Smithy lists ResourceNotFoundException with HTTP 404 on UpdateMonitor; it is not listed on GetMonitor, so the code and shape here are taken from the recording.)
Emulator: after a successful DeleteMonitor, GetMonitor on that name is refused with error code ValidationException rather than ResourceNotFoundException.
Why: a client cannot tell "this monitor does not exist" (a terminal, non-retryable, often expected condition during cleanup/poll loops) from "your request was malformed"; standard waiter/cleanup logic keyed on ResourceNotFoundException never terminates.
Fix: the not-found path of the GetMonitor handler (and the shared monitor-lookup helper in the service state it uses) must raise the not-found error shape.
Traces: internetmonitor.delete-monitor-removes-monitor.

Cause 6 of 7: the not-found message is not AWS's account+resource-derived template.
AWS: recorded behavior only — UpdateMonitor against a monitor that does not exist answers ResourceNotFoundException (Smithy: ResourceNotFoundException, HTTP 404, modeled on UpdateMonitor) whose message is built from the request and the caller's account: 'No resource found for account <accountId> and resource monitor/<MonitorName>'. That construction rule is the requirement; the recorded account id and name are examples.
Emulator: the code is correct (ResourceNotFoundException) but the message is a different template, e.g. 'The monitor sim2real-im-missing-18ebe7be8a6e does not exist.' where AWS returns 'No resource found for account 000000000000 and resource monitor/sim2real-im-missing-18ebe7be8a6e'.
Why: the emulator's message names neither the account nor the resource path, so clients and tests that surface or parse AWS's not-found text (to identify which account/resource was missing) get different information than AWS gives.
Fix: the ResourceNotFoundException construction shared by the monitor read/mutate handlers (UpdateMonitor and the Cause 5 GetMonitor path should use the same request-derived template).
Traces: internetmonitor.update-monitor-nonexistent-monitor-rejected.

Cause 7 of 7: GetQueryStatus and StopQuery are unimplemented scaffold stubs and answer 501 NotImplemented where AWS refuses an unknown QueryId with 400 ValidationException.
AWS: Smithy: GetQueryStatus (GET /v20210603/Monitors/{MonitorName}/Queries/{QueryId}/Status) and StopQuery (DELETE /v20210603/Monitors/{MonitorName}/Queries/{QueryId}) both model ValidationException at HTTP 400 ('Invalid request.'). Recorded behavior: with a syntactically valid but never-minted QueryId on a real ACTIVE monitor, both answer 400 ValidationException (recorded message example 'Invalid query ID provided'); StopQuery does not silently succeed as a no-op, and the monitor stays healthy and readable afterwards.
Emulator: both operations answer HTTP 501 with error code NotImplemented — the handlers still return crate::scaffolded::Scaffolded::not_implemented(..) (see Traces for files), so no query-lifecycle behavior is asserted at all.
Why: the whole query interface is unusable — a client gets an unmodeled 501/NotImplemented that no SDK error branch expects, instead of the modeled 400 ValidationException, and cannot poll or cancel a query.
Fix: implement the two operation handlers, returning the modeled ValidationException for a QueryId that does not exist for the named monitor.
Traces: internetmonitor.get-query-status-wrong-monitor-rejected, internetmonitor.stop-query-leaves-terminal-status; services/service-com-amazonaws-internetmonitor-internetmonitor20210603/src/ops/get_query_status.rs:167 and services/service-com-amazonaws-internetmonitor-internetmonitor20210603/src/ops/stop_query.rs:157.

## Evidence
Behavioral suite: 8/21 passed (38.1%), internetmonitor 8/21, 13 failed. Per failing check (expected = AWS, actual = emulator):

1. internetmonitor.create-monitor-duplicate-name-rejected [mismatch]
   AssertionError: expected message 'Monitor with name sim2real-im-dup-06cfc9437863 already exists', got 'The monitor sim2real-im-dup-06cfc9437863 already exists.' (code ConflictException matched).

2. internetmonitor.create-monitor-traffic-percentage-out-of-range-rejected [mismatch]
   CreateMonitor(TrafficPercentageToMonitor=0): assert code == "BadRequestException" -> AssertionError: CODE='ValidationException'.

3. internetmonitor.tag-resource-exceeds-tag-limit-rejected [mismatch]
   TagResource with 201 tag entries: assert code == "BadRequestException" -> AssertionError: expected BadRequestException, got 'ValidationException'.

4. internetmonitor.start-query-measurements-reaches-succeeded [mismatch]
   StartQuery(MEASUREMENTS) on a Resources=[] monitor: AssertionError: expected BadRequestException, got: ValidationException.
   internetmonitor.get-query-results-schema-for-empty-monitor [mismatch]
   AssertionError: expected BadRequestException for StartQuery(MEASUREMENTS) against a resourceless monitor, got: ValidationException.

5. internetmonitor.delete-monitor-removes-monitor [mismatch]
   After UpdateMonitor->INACTIVE + DeleteMonitor, GetMonitor on the same name: AssertionError: GetMonitor on deleted monitor returned unexpected code: ValidationException (expected ResourceNotFoundException with message 'No resource found for account <acct> and resource monitor/<name>').

6. internetmonitor.update-monitor-nonexistent-monitor-rejected [mismatch]
   AssertionError: expected message 'No resource found for account 000000000000 and resource monitor/sim2real-im-missing-18ebe7be8a6e', got 'The monitor sim2real-im-missing-18ebe7be8a6e does not exist.' (code ResourceNotFoundException matched).

7. internetmonitor.get-query-status-wrong-monitor-rejected [mismatch]
   GetQueryStatus(QueryId=00000000-0000-4000-8000-000000000000): AssertionError: expected ValidationException for bogus QueryId, got: NotImplemented.
   internetmonitor.stop-query-leaves-terminal-status [mismatch]
   StopQuery(same bogus QueryId): AssertionError: expected ValidationException, got: NotImplemented.
   Source confirmation: src/ops/get_query_status.rs and src/ops/stop_query.rs both still carry the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and end in Scaffolded::not_implemented(..).

Model evidence (smithy_query query="operation"): CreateMonitor.TrafficPercentageToMonitor @range 1..100, errors {AccessDenied, Conflict(409), InternalServer, LimitExceeded, Throttling, Validation(400)}; TagResource.Tags TagMap @length 0..200, errors include BadRequestException(400) and NotFoundException(404); UpdateMonitor errors include ResourceNotFoundException(404); GetMonitor errors {AccessDenied, InternalServer, Throttling, Validation}; GetQueryStatus/StopQuery errors include ValidationException(400).

## Steps to Reproduce
Boot the emulator and point a botocore internetmonitor client at it (account 000000000000, region us-west-2), then:

Cause 1: create_monitor(MonitorName=N, TrafficPercentageToMonitor=15); wait for ACTIVE; create_monitor(MonitorName=N, TrafficPercentageToMonitor=90) -> inspect Error.Message of the ConflictException.
Cause 2: create_monitor(MonitorName=<fresh>, TrafficPercentageToMonitor=0) -> inspect Error.Code (expect BadRequestException, message 'Invalid request body'); then list_monitors() and confirm no monitor was created.
Cause 3: create a monitor, take its Arn, then tag_resource(ResourceArn=arn, Tags={f"key-{i}": f"value-{i}" for i in range(201)}) -> inspect Error.Code (expect BadRequestException); list_tags_for_resource shows tags unchanged.
Cause 4: create_monitor(MonitorName=N) with no Resources; wait ACTIVE; start_query(MonitorName=N, StartTime=now-2h, EndTime=now, QueryType='MEASUREMENTS', FilterParameters=[]) -> inspect Error.Code (expect BadRequestException); repeat for every QueryType; get_monitor(N) still ACTIVE.
Cause 5: create_monitor(N); wait ACTIVE; update_monitor(N, Status='INACTIVE'); wait INACTIVE; delete_monitor(N); then get_monitor(N) -> inspect Error.Code (expect ResourceNotFoundException).
Cause 6: update_monitor(MonitorName=<never-created>, Status='ACTIVE') -> inspect Error.Message (expect 'No resource found for account <accountId> and resource monitor/<name>').
Cause 7: create_monitor(N); wait ACTIVE; get_query_status(MonitorName=N, QueryId='00000000-0000-4000-8000-000000000000') and stop_query(MonitorName=N, QueryId=same) -> both answer HTTP 501 NotImplemented; expect HTTP 400 ValidationException.

Or replay the checks listed in trace_ids with the behavioral suite.
