Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-jobs-data.absent-thing-queue-reads-over-permissive-while-describe-rejected iot-jobs-data.illegal-status-transitions-rejected-queued-to-succeeded-allowed iot-jobs-data.include-toggles-have-opposite-defaults-per-operation iot-jobs-data.stale-expected-version-rejected-leaves-execution-unchanged iot-jobs-data.start-command-execution-client-token-idempotent-and-absent-target-over-permissive iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected iot-jobs-data.start-next-returns-in-progress-before-next-queued iot-jobs-data.status-details-replaced-not-merged-and-kept-when-omitted iot-jobs-data.terminal-execution-read-and-update-rejected-distinctly
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data.absent-thing-queue-reads-over-permissive-while-describe-rejected|_jobs_data#1|PublishedUrlRejected|return iot, ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(host))
Issue-Key: iot-jobs-data.illegal-status-transitions-rejected-queued-to-succeeded-allowed|_jobs_data#1|PublishedUrlRejected|return iot, ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(host))
Issue-Key: iot-jobs-data.include-toggles-have-opposite-defaults-per-operation|_jobs_data#1|PublishedUrlRejected|return iot, ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(host))
Issue-Key: iot-jobs-data.stale-expected-version-rejected-leaves-execution-unchanged|_jobs_data#1|PublishedUrlRejected|return iot, ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(host))
Issue-Key: iot-jobs-data.start-command-execution-client-token-idempotent-and-absent-target-over-permissive|_jobs_data#1|PublishedUrlRejected|return iot, ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(host))
Issue-Key: iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected|_jobs_data#1|PublishedUrlRejected|return iot, ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(host))
Issue-Key: iot-jobs-data.start-next-returns-in-progress-before-next-queued|_jobs_data#1|PublishedUrlRejected|return iot, ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(host))
Issue-Key: iot-jobs-data.status-details-replaced-not-merged-and-kept-when-omitted|_jobs_data#1|PublishedUrlRejected|return iot, ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(host))
Issue-Key: iot-jobs-data.terminal-execution-read-and-update-rejected-distinctly|_jobs_data#1|PublishedUrlRejected|return iot, ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(host))
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# iot-jobs-data is unreachable through its documented discovery path: the iot:Data-ATS endpoint it must be addressed at is published as a real-AWS hostname, not an address on the emulator's own listener (1 cause, 9 checks)

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpoint

## Description
Cause 1 of 1: the iot-jobs-data data plane has no reachable address — the endpoint a client must use to address it is announced as a public AWS hostname the emulator does not serve.

AWS: iot-jobs-data (the IoT Jobs/Commands data plane) is not addressed at the regional control-plane endpoint; a client must be built against the account's data endpoint, discovered with iot DescribeEndpoint(endpointType="iot:Data-ATS"). Smithy com.amazonaws.iot#DescribeEndpoint returns endpointAddress, documented as `identifier.iot.region.amazonaws.com`, and on real AWS that host serves the Jobs data plane: GetPendingJobExecutions, StartNextPendingJobExecution, DescribeJobExecution, UpdateJobExecution and StartCommandExecution all answer there (the Jobs data-plane operations themselves are modelled with InvalidRequestException 400, ResourceNotFoundException 404, ThrottlingException 429, CertificateValidationException 400, InvalidStateTransitionException 409, TerminalStateException 410 — none of which can ever be observed here). In an emulator the same contract requires the announced address to be one the emulator itself serves — its own host and port, or a per-resource listener/path that the front door routes to the iot-jobs-data handlers. The specific host, port and path are environment-specific and not part of the requirement; being an address the emulator answers on is.

Emulator: DescribeEndpoint answers HTTP 200 with `<identifier>-ats.iot.<region>.amazonaws.com` (observed `jzdx8mw2jjhboa-ats.iot.us-west-2.amazonaws.com`), a literal public-AWS host synthesized from the emulator account id and signed region. That host is on none of the emulator's listeners, so a client built against it never arrives: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>-ats.iot.<region>.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. No iot-jobs-data operation is ever invoked.

Why: a client doing exactly what AWS prescribes for this service — discover the data endpoint, then address iot-jobs-data at it — is pointed outside the emulator and either reaches real AWS or fails on name resolution. Consequently the whole Jobs/Commands data plane is untestable and unusable here: all nine of this service's checks (queue reads for an absent thing, QUEUED→SUCCEEDED and illegal status transitions, per-operation includeJobDocument/includeJobExecutionState defaults, stale expectedVersion rejection, StartCommandExecution clientToken idempotency and absent-target handling, unknown/deprecated command rejection, start-next ordering, statusDetails replace-vs-merge semantics, terminal-execution read/update refusals) stop at client construction and exercise nothing. Every status, error code and state-machine rule this service owns is unverified behind that one unreachable address.

Fix: the address rendering in iot's DescribeEndpoint handler (services/service-com-amazonaws-iot-awsiotservice/src/ops/describe_endpoint.rs, `EndpointType::address`) must publish the emulator's own advertised listener for the IoT data plane instead of the hard-coded `*.amazonaws.com` templates, AND the front door must route that published address to the iot-jobs-data (Jobs + Commands) handlers so a client constructed from it is served. Tracked for the iot service as the companion issue filed in this same sweep; filed separately here because these nine failing checks are reported under the iot-jobs-data heading and carry their own replay keys.

Traces: iot-jobs-data.absent-thing-queue-reads-over-permissive-while-describe-rejected, iot-jobs-data.illegal-status-transitions-rejected-queued-to-succeeded-allowed, iot-jobs-data.include-toggles-have-opposite-defaults-per-operation, iot-jobs-data.stale-expected-version-rejected-leaves-execution-unchanged, iot-jobs-data.start-command-execution-client-token-idempotent-and-absent-target-over-permissive, iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected, iot-jobs-data.start-next-returns-in-progress-before-next-queued, iot-jobs-data.status-details-replaced-not-merged-and-kept-when-omitted, iot-jobs-data.terminal-execution-read-and-update-rejected-distinctly.

## Evidence
Expected: `endpoint = iot.describe_endpoint(endpointType="iot:Data-ATS")["endpointAddress"]` yields a host the emulator serves, so `ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(endpoint))` can be built and its Jobs/Commands calls are answered by the emulator (on real AWS the `<id>-ats.iot.<region>.amazonaws.com` host answers them).

Actual: the published address is `<identifier>-ats.iot.<region>.amazonaws.com` — a public AWS host on no emulator listener — and client construction is refused before any iot-jobs-data operation is sent:

  File "context.py", line 572, in endpoint_route
    return self._published_target(url, "endpoint_url"), {}
  File "context.py", line 586, in _published_target
    raise PublishedUrlRejected(...)
  simparity.published.PublishedUrlRejected: endpoint_url was given https://jzdx8mw2jjhboa-ats.iot.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:40261/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

(the identifier, region and port vary per run and are not part of the requirement). All 9 iot-jobs-data checks fail identically at this step (0/9 passed); no Jobs data-plane status, error code or message was observable at all.

## Steps to Reproduce
1. Call iot DescribeEndpoint(endpointType="iot:Data-ATS") against the emulator and take endpointAddress.
2. Build an iot-jobs-data client with endpoint_url set to that address (the only documented way to address the Jobs/Commands data plane).
3. Issue any Jobs data-plane call — e.g. GetPendingJobExecutions(thingName=...), StartNextPendingJobExecution(thingName=...), UpdateJobExecution(jobId=..., thingName=..., status=...), or StartCommandExecution(commandArn=..., targetArn=...).
4. The call never reaches the emulator: the announced host is outside it (PublishedUrlRejected in the harness; name-resolution/connection failure in a sandbox without egress).
5. Replay the listed checks (suite simparity-behavioral-aws-v1): each stops at step 2/3 and exercises no iot-jobs-data behaviour.
