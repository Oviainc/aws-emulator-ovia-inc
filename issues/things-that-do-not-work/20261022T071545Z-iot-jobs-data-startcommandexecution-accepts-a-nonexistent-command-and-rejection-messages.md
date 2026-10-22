Issue-Source: trace-sweep-divergence
Issue-Trace: iotjobsdata-command-execution-validation-refusals
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotjobsdata-command-execution-validation-refusals iotjobsdata-absent-thing-permissive-queue-reads iotjobsdata-job-execution-queue-poll-and-start
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data|StartCommandExecution|4xx->2xx|$.message
Issue-Key: iot-jobs-data|StartCommandExecution|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot-jobs-data|StartCommandExecution|4xx->2xx|status
Issue-Key: iot-jobs-data|StartCommandExecution|4xx->4xx|$.message
Issue-Related: 20261022T071545Z-iot-jobs-data-job-execution-state-is-never.md
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# StartCommandExecution accepts a nonexistent command and rejection messages are built by the wrong rules

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCommandExecution

## Description
Three causes in iot-jobs-data (sdkId "IoT Jobs Data Plane", restJson1) share one fault: the request-validation layer of the command-execution surface, and the rules that build its rejection payloads, do not match the service. A fixer must close ALL THREE before submitting.

Cause 1 of 3: StartCommandExecution accepts a commandArn that names no existing command and answers 200.
AWS: POST /command-executions validates the referenced command before starting an execution and refuses the request with a 400 client error (Smithy: StartCommandExecution errors ValidationException errorKind client httpError 400; input members targetArn and commandArn both required). The recording shows status 400 with an x-amzn-errortype error header and a message naming the missing command, for example "The command <commandName> was not found.".
Emulator: 200 — the execution is accepted, no message and no x-amzn-errortype header are emitted.
Why: this is a permissive accept. A client that mistypes or races a command deletion gets an executionId for an execution that can never run, instead of the modeled 400, so the error never surfaces and the failure appears later as a silently dead execution.
Fix: StartCommandExecution's request validation (existence check on commandArn) and its error serializer (status 400, modeled error type, message naming the command).
Traces: iotjobsdata-command-execution-validation-refusals#task#2.

Cause 2 of 3: the unsupported-target rejection message is a fixed ARN-format template instead of naming the target's resource type.
AWS: when targetArn names a resource type that cannot be a command target, the 400 message is built from the request — it names the resource type parsed out of the supplied ARN, for example "Resource type job not supported as a target" (Smithy: StartCommandExecution input targetArn required, @length 0..2048, refused with ValidationException httpError 400). Two recorded steps with different target ARNs both carry that request-derived form.
Emulator: status 400 is correct, but the body always carries one fixed template — "targetArn must name a thing, as arn:<partition>:iot:<region>:<account>:thing/<name>" — which never states which resource type was rejected.
Why: the message rule is request-derived and therefore part of the contract; a client (or an operator reading logs) that keys off the rejected resource type cannot tell what it passed, and the two distinct recorded rejections become indistinguishable.
Fix: StartCommandExecution's targetArn validation message construction.
Traces: iotjobsdata-command-execution-validation-refusals#task#3 (same cluster: #task#4).

Cause 3 of 3: DescribeJobExecution's not-found message does not follow the service's request-derived form.
AWS: for a thing/jobId with no execution, the 404 message names the job and the thing in the service's form, for example "Job <jobId> not found for <thingName>" (Smithy: DescribeJobExecution errors include ResourceNotFoundException errorKind client httpError 404; input members jobId and thingName are required httpLabels, so the rule is built from the request).
Emulator: status 404 and the error type are right, but the message is its own phrasing — "no job execution <jobId> for thing <thingName>".
Why: the message rule is derived from the request members, so the wording is part of the behavior clients and log scrapers see; only the text diverges here, the status and error code already match.
Fix: the DescribeJobExecution handler's not-found message construction.
Traces: iotjobsdata-absent-thing-permissive-queue-reads#task#4 (same cluster: iotjobsdata-job-execution-queue-poll-and-start#teardown#10).

The deeper reason DescribeJobExecution/UpdateJobExecution reach a not-found path at all is filed separately as the iot-jobs-data job-execution-state issue; this issue is about the validation and message rules themselves.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; state_reset_failures 0), service iot-jobs-data:
1) StartCommandExecution, fixture iotjobsdata-command-execution-validation-refusals step task#2: expected status 400 with $.message 'The command sim2real-probe-ijd-nocmd-32767411 was not found.' and an x-amzn-errortype header; got status 200 with no message and no x-amzn-errortype.
2) StartCommandExecution, same fixture step task#3 (and task#4): status 400 matches, but $.message expected 'Resource type job not supported as a target', got 'targetArn must name a thing, as arn:<partition>:iot:<region>:<account>:thing/<name>'.
3) DescribeJobExecution, fixture iotjobsdata-absent-thing-permissive-queue-reads step task#4 (and iotjobsdata-job-execution-queue-poll-and-start#teardown#10): status 404 matches, but $.message expected 'Job sim2real-probe-ijd-nojob-32767411 not found for sim2real-probe-ijd-nothing-32767411', got 'no job execution sim2real-probe-ijd-nojob-32767411 for thing sim2real-probe-ijd-nothing-32767411'. Recorded ids/names above are examples; the requirement is the status, the error code and the stated message rule.
Model basis: services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/smithy-model/com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice.json.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture iotjobsdata-command-execution-validation-refusals step task#2 — POST /command-executions with a commandArn that names no command; observe 200 instead of a 400 ValidationException.
3. Cause 2: replay the same fixture steps task#3 and task#4 — POST /command-executions with a targetArn whose resource type is not a thing; observe the fixed ARN-format message instead of one naming the rejected resource type.
4. Cause 3: replay fixture iotjobsdata-absent-thing-permissive-queue-reads step task#4 — GET /things/{thingName}/jobs/{jobId} for a thing/job with no execution; compare the 404 message text.
