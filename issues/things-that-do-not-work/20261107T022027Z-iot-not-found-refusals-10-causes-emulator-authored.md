Issue-Source: trace-sweep-divergence
Issue-Trace: iot-authorizer-duplicate-and-notfound-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-authorizer-duplicate-and-notfound-errors iot-authorizer-error-paths iot-authorizer-lifecycle iot-authorizer-lifecycle-inactive iot-custom-authorizer-lifecycle iot-command-duplicate-and-execution-errors iot-command-lifecycle iot-dynamic-thing-group-lifecycle iot-dynamic-thing-group-workflow iot-fleet-indexing-error-paths iot-thing-group-duplicate-and-idempotent-delete iot-job-and-job-execution-error-paths iot-job-execution-workflow iot-job-list-lifecycle iot-job-execution-errors iot-policy-targets-and-missing-registration-task iot-thing-type-and-thing-lifecycle iot-thing-type-create-delete-then-error iot-describe-index-invalid-name iot-describe-index-not-found-error
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|DescribeAuthorizer|4xx->4xx|$.message
Issue-Key: iot|DeleteAuthorizer|4xx->4xx|$.message
Issue-Key: iot|GetCommand|4xx->4xx|$.message
Issue-Key: iot|DescribeThingGroup|4xx->4xx|$.message
Issue-Key: iot|DescribeJob|4xx->4xx|$.message
Issue-Key: iot|DeleteJob|4xx->4xx|$.message
Issue-Key: iot|DescribeJobExecution|4xx->4xx|$.message
Issue-Key: iot|DescribeThingRegistrationTask|4xx->4xx|$.message
Issue-Key: iot|DescribeThingType|4xx->4xx|$.message
Issue-Key: iot|DescribeIndex|4xx->4xx|$.message
Issue-Key: iot|DescribeIndex|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|DescribeIndex|4xx->4xx|status
Issue-Commit: ff097a89ab5b098f807ae300b709617042ce117b

# iot not-found refusals: 10 causes — emulator-authored wording replaces AWS's per-resource not-found text

> _Note: this issue was identified against the code base at commit `ff097a89ab5b098f807ae300b709617042ce117b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAuthorizer

## Description
The emulator serves every IoT not-found refusal from one generic template, '<Resource> <name> does not exist'. AWS IoT has NO single not-found sentence: each resource family has its own fixed wording, some of them account-qualified, and one of the ten cases below is not a 404 at all. 10 net-new causes, all in service iot.

Cause 1 of 10: DescribeAuthorizer on a missing authorizer.
AWS: Smithy: DescribeAuthorizer errors include ResourceNotFoundException (httpError 404); authorizerName is @required, @length(1..128), @pattern ^[\w=,@-]+$. Recorded behavior: 404 ResourceNotFoundException whose message is the fixed text 'Authorizer ' + <authorizerName from the request> + ' not found' — fixed parts 'Authorizer ' and ' not found', variable part the requested authorizerName.
Emulator: 404 with message 'Authorizer <name> does not exist'.
Why: a client that matches or logs AWS's refusal text (and any test asserting it) sees a sentence AWS never emits; the trailing clause differs, so string-based error handling fails even though the status is right.
Fix: the DescribeAuthorizer handler's not-found path / the authorizer state's lookup-miss error construction.
Traces: iot-authorizer-duplicate-and-notfound-errors#task#3 (also iot-authorizer-duplicate-and-notfound-errors#teardown#6, iot-authorizer-error-paths#task#5, iot-authorizer-lifecycle#task#8, iot-authorizer-lifecycle-inactive#task#4, iot-custom-authorizer-lifecycle#task#8, iot-custom-authorizer-lifecycle#teardown#13).

Cause 2 of 10: DeleteAuthorizer on a missing authorizer.
AWS: Smithy: DeleteAuthorizer errors include ResourceNotFoundException (404). Recorded behavior: 404, same fixed wording as the describe path — 'Authorizer ' + <authorizerName> + ' not found'.
Emulator: 404 'Authorizer <name> does not exist'.
Why: same text mismatch on the delete path.
Fix: the DeleteAuthorizer handler's not-found path.
Traces: iot-authorizer-duplicate-and-notfound-errors#task#4 (also iot-custom-authorizer-lifecycle#task#10).

Cause 3 of 10: GetCommand on a missing command.
AWS: Smithy: GetCommand errors include ResourceNotFoundException (404). Recorded behavior: 404 whose message is 'Command ' + <commandId> + ' cannot be found.' — note the 'cannot be found.' clause and the terminating period; the only variable part is the commandId from the URI label.
Emulator: 404 'Command <id> does not exist' (different clause, no period).
Why: the commands plane uses a different not-found sentence from the registry plane in AWS; collapsing both to one template makes the emulator's refusal unrecognizable to either.
Fix: the GetCommand handler's not-found path.
Traces: iot-command-duplicate-and-execution-errors#task#3 (also iot-command-lifecycle#task#2).

Cause 4 of 10: DescribeThingGroup on a missing thing group.
AWS: Smithy: DescribeThingGroup errors include ResourceNotFoundException (404). Recorded behavior: 404 whose message is account-qualified — 'ThingGroup ' + <accountId> + ':' + <thingGroupName> + ' not found.' Fixed text: 'ThingGroup ', the ':' separator and ' not found.'; variable parts: the caller's account id and the requested group name.
Emulator: 404 'Thing group <name> does not exist' — different noun casing ('Thing group' vs 'ThingGroup'), no account qualifier, no period.
Why: the account-id prefix is part of AWS's message contract for this family; a client parsing it for the account scope gets nothing.
Fix: the DescribeThingGroup handler's not-found path.
Traces: iot-dynamic-thing-group-lifecycle#task#3 (also iot-dynamic-thing-group-workflow#task#7, iot-fleet-indexing-error-paths#task#5, iot-thing-group-duplicate-and-idempotent-delete#task#4).

Cause 5 of 10: DescribeJob on a missing job.
AWS: Smithy: DescribeJob errors include ResourceNotFoundException (404); jobId is @required, @length(1..64). Recorded behavior: 404 'Job ' + <jobId> + ' cannot be found.'
Emulator: 404 'Job <id> does not exist'.
Why: same clause/period mismatch on the jobs plane.
Fix: the DescribeJob handler's not-found path.
Traces: iot-job-and-job-execution-error-paths#task#4 (also iot-job-execution-workflow#task#9, iot-job-list-lifecycle#teardown#7).

Cause 6 of 10: DeleteJob on a missing job.
AWS: recorded behavior: 404 with the same fixed text as DescribeJob — 'Job ' + <jobId> + ' cannot be found.'
Emulator: 404 'Job <id> does not exist'.
Fix: the DeleteJob handler's not-found path.
Why: same text mismatch on the delete path.
Traces: iot-job-and-job-execution-error-paths#task#5.

Cause 7 of 10: DescribeJobExecution when the JOB itself does not exist.
AWS: recorded behavior: 404 whose message names the missing JOB, not the thing — the fixed text 'Job execution cannot be found as the Job ' + <jobId> + ' does not exist.'
Emulator: 404 'No job execution of job <jobId> exists on thing <thingName>' — it reports a missing EXECUTION on an existing job and interpolates the thing name AWS never mentions here.
Why: the two conditions are distinguishable in AWS (missing job vs missing execution of an existing job) and the emulator conflates them, so a client cannot tell which resource to create.
Fix: the DescribeJobExecution handler's lookup ordering — resolve the job first and emit the job-missing refusal before the per-thing execution lookup.
Traces: iot-job-and-job-execution-error-paths#task#7 (also iot-job-execution-errors#task#3, iot-job-execution-workflow#task#5).

Cause 8 of 10: DescribeThingRegistrationTask on a missing task.
AWS: recorded behavior: 404 whose message is the FIXED sentence "Task doesn't exist" with NO variable part at all — the task id is not interpolated.
Emulator: 404 'Registration task <taskId> does not exist', i.e. it adds a resource noun and interpolates the id.
Why: a client matching AWS's constant string never matches, and the emulator leaks the requested id into a message AWS keeps opaque.
Fix: the DescribeThingRegistrationTask handler's not-found path.
Traces: iot-policy-targets-and-missing-registration-task#task#3.

Cause 9 of 10: DescribeThingType on a missing thing type.
AWS: recorded behavior: 404, account-qualified like the thing-group family — 'Thing Type ' + <accountId> + ':' + <thingTypeName> + ' not found.' Fixed text 'Thing Type ' (two words, both capitalised) and ' not found.'
Emulator: 404 'Thing type <name> does not exist' — lowercase 'type', no account qualifier, no period.
Fix: the DescribeThingType handler's not-found path.
Why: same family-specific wording loss, plus the missing account scope.
Traces: iot-thing-type-and-thing-lifecycle#task#8 (also iot-thing-type-create-delete-then-error#task#2).

Cause 10 of 10: DescribeIndex on an unknown index answers the wrong STATUS and error class, not just the wrong text.
AWS: Smithy: DescribeIndex errors include both InvalidRequestException (httpError 400) and ResourceNotFoundException (404); indexName is @required, @pattern ^[a-zA-Z0-9:_-]+$. Recorded behavior: an index name that is not one of the service's recognised indices is a 400 InvalidRequestException with x-amzn-errortype 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/' and the fixed text 'Unrecognized indexName ' + <indexName from the request>.
Emulator: 404 with x-amzn-errortype 'ResourceNotFoundException' and message 'Index <name> does not exist'.
Why: this is a retry/branching difference, not cosmetic — an SDK maps 404/ResourceNotFoundException to a not-found exception type, so code that treats an unrecognized index name as a request-validation error (AWS's 400 InvalidRequestException) takes the wrong branch, and the emulator also drops the IoT doc-namespace suffix AWS appends to the errortype header.
Fix: the DescribeIndex handler's validation — classify an unrecognized indexName as InvalidRequestException(400) before the state lookup, and the shared errortype serializer must append the IoT xmlNamespace suffix for the legacy iot exception family.
Traces: iot-describe-index-invalid-name#task#1 (also iot-describe-index-not-found-error#task#1).

Related open issues that do NOT cover these: '20261106T201842Z-iot-registry-plane-9-net-new-causes-creatething.md' and '20261106T201842Z-iot-refusal-fidelity-10-net-new-causes-createpolicy.md' are behavioral findings and carry no replay key for any of the ten fixture steps above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. Expected = recorded AWS response, Actual = emulator response.

1. DescribeAuthorizer, iot-authorizer-duplicate-and-notfound-errors#task#3 — status 404 both; $.message expected 'Authorizer sim2real-probe-iot-authorizer-missing-3fd37fcf not found', got 'Authorizer sim2real-probe-iot-authorizer-missing-3fd37fcf does not exist'. Cluster of 7 steps.
2. DeleteAuthorizer, iot-authorizer-duplicate-and-notfound-errors#task#4 — $.message expected '... not found', got '... does not exist'. Cluster of 2.
3. GetCommand, iot-command-duplicate-and-execution-errors#task#3 — expected 'Command sim2real-probe-iot-cmd-missing-99f1bf78 cannot be found.', got 'Command sim2real-probe-iot-cmd-missing-99f1bf78 does not exist'. Cluster of 2.
4. DescribeThingGroup, iot-dynamic-thing-group-lifecycle#task#3 — expected 'ThingGroup 000000000000:sim2real-probe-iot-dyngroup-missing-6c337def not found.', got 'Thing group sim2real-probe-iot-dyngroup-missing-6c337def does not exist'. Cluster of 4.
5. DescribeJob, iot-job-and-job-execution-error-paths#task#4 — expected 'Job ... cannot be found.', got 'Job ... does not exist'. Cluster of 3.
6. DeleteJob, iot-job-and-job-execution-error-paths#task#5 — expected 'Job ... cannot be found.', got 'Job ... does not exist'.
7. DescribeJobExecution, iot-job-and-job-execution-error-paths#task#7 — expected 'Job execution cannot be found as the Job ... does not exist.', got 'No job execution of job ... exists on thing ...'. Cluster of 3.
8. DescribeThingRegistrationTask, iot-policy-targets-and-missing-registration-task#task#3 — expected "Task doesn't exist", got 'Registration task sim2real-probe-iot-missing-task-919dc29c does not exist'.
9. DescribeThingType, iot-thing-type-and-thing-lifecycle#task#8 — expected 'Thing Type 000000000000:sim2real-probe-iot-notype-1cc9c9fc not found.', got 'Thing type sim2real-probe-iot-notype-1cc9c9fc does not exist'. Cluster of 2.
10. DescribeIndex, iot-describe-index-invalid-name#task#1 — status expected 400, got 404; $.message expected 'Unrecognized indexName sim2real-probe-iot-no-such-index-0cd0ac25', got 'Index sim2real-probe-iot-no-such-index-0cd0ac25 does not exist'; $headers.x-amzn-errortype expected 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/', got 'ResourceNotFoundException'. Cluster of 2.

(Resource names, account ids and index names above are this recording's values; the requirement is the fixed text plus the request/state-derived substitutions described per cause.)

## Steps to Reproduce
1. git clone the repo and boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Observe the ten clusters listed under Evidence, starting with fixture iot-authorizer-duplicate-and-notfound-errors step task#3.
4. Equivalently, by hand against the emulator's iot endpoint: GET /authorizer/does-not-exist, GET /commands/does-not-exist, GET /thing-groups/does-not-exist, GET /jobs/does-not-exist, DELETE /jobs/does-not-exist, GET /things/<t>/jobs/does-not-exist, GET /thing-registration-tasks/does-not-exist, GET /thing-types/does-not-exist, GET /indices/does-not-exist, and compare each status, x-amzn-errortype header and message against the AWS wording given per cause.
