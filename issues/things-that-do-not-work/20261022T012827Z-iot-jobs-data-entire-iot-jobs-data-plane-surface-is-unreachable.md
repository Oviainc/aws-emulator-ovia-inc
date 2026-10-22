Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-jobs-data.absent-thing-queue-reads-over-permissive-while-describe-rejected iot-jobs-data.illegal-status-transitions-rejected-queued-to-succeeded-allowed iot-jobs-data.include-toggles-have-opposite-defaults-per-operation iot-jobs-data.stale-expected-version-rejected-leaves-execution-unchanged iot-jobs-data.start-command-execution-client-token-idempotent-and-absent-target-over-permissive iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected iot-jobs-data.start-next-returns-in-progress-before-next-queued iot-jobs-data.status-details-replaced-not-merged-and-kept-when-omitted iot-jobs-data.terminal-execution-read-and-update-rejected-distinctly
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data|DescribeEndpoint|NotImplemented
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# Entire IoT Jobs Data Plane surface is unreachable: data-plane endpoint discovery (IoT DescribeEndpoint) answers NotImplemented, so all 9 jobs-data behavioral checks abort before any jobs-data call

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpoint

## Description
Cause 1 of 1: The IoT Jobs Data Plane emulator (sdkId "IoT Jobs Data Plane", crate service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice) is bootstrapped and its five operations (DescribeJobExecution, GetPendingJobExecutions, StartNextPendingJobExecution, UpdateJobExecution, StartCommandExecution) are modeled, but NONE of them can be reached in this emulator, because the only supported way for a client to learn the data-plane host is the IoT control-plane DescribeEndpoint call, and that call returns a scaffold NotImplemented error. Every one of the 9 iot-jobs-data behavioral checks dies on its first statement (`iot.describe_endpoint(endpointType="iot:Data-ATS")`), so the jobs-data emulator was never asked anything at all.

AWS: Smithy (com-amazonaws-iot-awsiotservice) models DescribeEndpoint as `GET /endpoint` with optional query member `endpointType` (EndpointType, @length 0..128, pattern ^[\s\S]*$), HTTP success code 200, and output member `endpointAddress` (EndpointAddress). Its only modeled errors are InternalFailureException (500), InvalidRequestException (400), ThrottlingException (400) and UnauthorizedException (401) — there is no "NotImplemented" error on this operation. With endpointType="iot:Data-ATS" real AWS answers 200 and an account-specific ATS host, and that host is where the jobs-data operations are served; the generic data.jobs.iot.<region>.amazonaws.com host is not usable (the checks record 403 AccessDeniedException there, and iot:Jobs is retired in favour of iot:Data-ATS). Smithy (com-amazonaws-iotjobsdataplane-...) binds the data-plane operations under /things/{thingName}/jobs... and /command-executions on that host.

Emulator: the control-plane call fails with `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeEndpoint operation: operation DescribeEndpoint is scaffolded but not implemented in this emulator`. Because the emulator never hands back a data-plane address, no client can construct a jobs-data client, and the whole jobs-data behavioral surface — queue reads on an absent thing, job-execution status-transition rules, include-toggle defaults, expectedVersion guards, statusDetails replace-vs-merge semantics, StartNext ordering, terminal-state read/write rejections, and StartCommandExecution validation/idempotency — is unexercised and unverified.

Why: a client follows the documented bootstrap sequence (DescribeEndpoint -> point the jobs-data client at the returned host -> poll/ack job executions). Against this emulator that first step raises an unmodeled NotImplemented error instead of returning 200 + endpointAddress, so a device agent or jobs-data integration cannot start at all; it cannot even fall back, since the generic host is not a valid substitute. The jobs-data emulator's fidelity is also untestable until endpoint discovery exists.

Note on related filings: the suite marks the iot-side DescribeEndpoint stub as already filed for the `iot` service, and an open bootstrap issue exists for the IoT control plane ("Bootstrap the com-amazonaws-iot-awsiotservice (AWS IoT) emulator", 20261021T214813Z-bootstrap-service-com-amazonaws-iot-awsiotservice.md). Neither of those carries the iot-jobs-data replay keys, so this issue records them: the jobs-data checks below are the ones that must be re-replayed once endpoint discovery answers, and they will almost certainly surface further jobs-data divergences that are currently invisible.

Fix: implement IoT control-plane DescribeEndpoint (services/service-com-amazonaws-iot-awsiotservice/src/ops/describe_endpoint.rs) so that endpointType="iot:Data-ATS" returns 200 with an endpointAddress that routes to this emulator's IoT Jobs Data Plane handlers, and make the jobs-data crate serve its five operations on the host that address names. Then re-replay the jobs-data checks listed under Traces.

Traces: iot-jobs-data.absent-thing-queue-reads-over-permissive-while-describe-rejected, iot-jobs-data.illegal-status-transitions-rejected-queued-to-succeeded-allowed, iot-jobs-data.include-toggles-have-opposite-defaults-per-operation, iot-jobs-data.stale-expected-version-rejected-leaves-execution-unchanged, iot-jobs-data.start-command-execution-client-token-idempotent-and-absent-target-over-permissive, iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected, iot-jobs-data.start-next-returns-in-progress-before-next-queued, iot-jobs-data.status-details-replaced-not-merged-and-kept-when-omitted, iot-jobs-data.terminal-execution-read-and-update-rejected-distinctly (all 9 iot-jobs-data checks in suite simparity-behavioral-aws-v1, 0/9 passed).

## Evidence
Expected (AWS, per Smithy com-amazonaws-iot-awsiotservice DescribeEndpoint): GET /endpoint?endpointType=iot:Data-ATS -> HTTP 200, body containing `endpointAddress` naming the account's ATS data host; the jobs-data operations are then reachable there (restJson1, sigv4).

Actual (emulator): every iot-jobs-data check aborts on its first call with HTTP error code `NotImplemented` — `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeEndpoint operation: operation DescribeEndpoint is scaffolded but not implemented in this emulator`. `NotImplemented` is not among DescribeEndpoint's modeled errors (InternalFailureException/500, InvalidRequestException/400, ThrottlingException/400, UnauthorizedException/401).

Suite result: iot-jobs-data 0/9 (0%), 9 errored, 0 reached any iot-jobs-data operation. No jobs-data request was ever issued, so the jobs-data emulator's behaviour on DescribeJobExecution / GetPendingJobExecutions / StartNextPendingJobExecution / UpdateJobExecution / StartCommandExecution is entirely unverified.

## Steps to Reproduce
1. Boot the emulator and create a botocore `iot` client against it.
2. Call `iot.describe_endpoint(endpointType="iot:Data-ATS")`.
3. Observe a ClientError with code `NotImplemented` instead of HTTP 200 with an `endpointAddress`.
4. Because no address is returned, an `iot-jobs-data` client cannot be bound to a data-plane host; e.g. the sequence used by every failing check (`create_thing` -> `create_job` -> `jd.describe_job_execution(thingName=..., jobId=...)`) cannot be started.
5. Re-replay checks: iot-jobs-data.absent-thing-queue-reads-over-permissive-while-describe-rejected, iot-jobs-data.illegal-status-transitions-rejected-queued-to-succeeded-allowed, iot-jobs-data.include-toggles-have-opposite-defaults-per-operation, iot-jobs-data.stale-expected-version-rejected-leaves-execution-unchanged, iot-jobs-data.start-command-execution-client-token-idempotent-and-absent-target-over-permissive, iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected, iot-jobs-data.start-next-returns-in-progress-before-next-queued, iot-jobs-data.status-details-replaced-not-merged-and-kept-when-omitted, iot-jobs-data.terminal-execution-read-and-update-rejected-distinctly.
