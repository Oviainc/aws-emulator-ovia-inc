Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.command-execution-delete-non-terminal-rejected iot.command-execution-state-transitions iot.concurrent-command-execution-creation-respects-limit iot.delete-command-execution-non-terminal-rejected iot.e2e-ota-update-package-job-delivery iot.job-execution-in-progress-timeout-transitions iot.jobs-data-step-timeout-times-out-one-thing-while-a-reset-twin-survives iot.ordering-list-command-executions-chronological iot.update-package-configuration-auto-versioning-applied iot.visibility-timeout-job-execution-timeout-expires
Issue-Key: iot.command-execution-delete-non-terminal-rejected|run#3|PublishedUrlRejected|jobs_data = ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(endpoint))
Issue-Key: iot.command-execution-state-transitions|run#3|PublishedUrlRejected|jobs_data = ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(endpoint))
Issue-Key: iot.concurrent-command-execution-creation-respects-limit|run#6|PublishedUrlRejected|jobs_data = ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(endpoint_addr))
Issue-Key: iot.delete-command-execution-non-terminal-rejected|run#9|PublishedUrlRejected|jobs_data = ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(data_endpoint))
Issue-Key: iot.e2e-ota-update-package-job-delivery|run#42|PublishedUrlRejected|jobs_data = ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(jobs_endpoint))
Issue-Key: iot.job-execution-in-progress-timeout-transitions|run#2|PublishedUrlRejected|jobs_endpoint = ctx.endpoint_url(iot.describe_endpoint(endpointType="iot:Data-ATS")["endpointAddress"])
Issue-Key: iot.jobs-data-step-timeout-times-out-one-thing-while-a-reset-twin-survives|run#3|PublishedUrlRejected|jobs = ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(jobs_host))
Issue-Key: iot.ordering-list-command-executions-chronological|run#3|PublishedUrlRejected|jobs_data = ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(endpoint))
Issue-Key: iot.update-package-configuration-auto-versioning-applied|run#3|PublishedUrlRejected|jobs_endpoint = ctx.endpoint_url(iot.describe_endpoint(endpointType="iot:Data-ATS")["endpointAddress"])
Issue-Key: iot.visibility-timeout-job-execution-timeout-expires|run#3|PublishedUrlRejected|jobs_data = ctx.client("iot-jobs-data", endpoint_url=ctx.endpoint_url(endpoint))
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# iot DescribeEndpoint publishes a real-AWS hostname (<id>-ats.iot.<region>.amazonaws.com) instead of an address on the emulator's own listener, so the whole IoT data plane is unreachable (1 cause, 10 checks)

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpoint

## Description
Cause 1 of 1: DescribeEndpoint announces an endpoint address that is not an address the emulator can be reached on, so every client that follows the announced endpoint leaves the emulator entirely.

AWS: Smithy com.amazonaws.iot#DescribeEndpoint — GET /endpoint, input member endpointType (@length 0..128), output member endpointAddress, modelled errors InternalFailureException (500), InvalidRequestException (400), ThrottlingException (400), UnauthorizedException (401). The endpointAddress member documents the form `identifier.iot.region.amazonaws.com`, and the operation's contract is "Returns or creates a unique endpoint specific to the account making the call" — i.e. the address returned is the address at which that account's IoT data/jobs/commands data plane actually answers. On real AWS the announced host is a live host: a client that constructs an `iot-jobs-data` (or `iot-data`) client with `endpoint_url=<endpointAddress>` reaches the Jobs/Commands data plane and its calls succeed. In an emulator the equivalent requirement is that the announced address resolves to the emulator itself — the emulator's own host and port (e.g. `http(s)://<own host>:<port>/<resource path>`, or a per-resource listener) — because that is the only place the data plane is served. The host/port/path values are environment-specific and are not part of the requirement; what is required is that the announced address is one the emulator serves.

Emulator: DescribeEndpoint(endpointType="iot:Data-ATS") answers HTTP 200 with a literal public-AWS hostname of the form `<identifier>-ats.iot.<region>.amazonaws.com` (observed e.g. `jzdx8mw2jjhboa-ats.iot.us-west-2.amazonaws.com`), synthesized purely from the emulator account id and the signed region. The same handler likewise renders `<id>.iot.<region>.amazonaws.com`, `<id>.credentials.iot.<region>.amazonaws.com` and `<id>.jobs.iot.<region>.amazonaws.com` for the other endpoint types. None of those hosts is served by the emulator, so the harness refuses the published URL before any data-plane call is attempted: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>-ats.iot.<region>.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`.

Why: a client that does exactly what the AWS documentation and every AWS SDK sample prescribe — call DescribeEndpoint, then build the IoT data/jobs client against the returned endpointAddress — is sent to a host outside the emulator (real AWS, or nothing at all, depending on egress). The entire IoT jobs/commands data plane is therefore unreachable through its documented discovery path: all ten of this service's command-execution, job-execution-timeout, OTA-delivery, ordering and package-configuration checks stop at the discovery step and never exercise the behaviour they test. It also means a correct fix to any jobs/commands behaviour cannot be observed through the supported client-construction path.

Fix: the address rendering in DescribeEndpoint's handler — `EndpointType::address()` and the `endpoint_address` it returns must be derived from the emulator's own advertised listener (host, port, scheme, and any per-resource path prefix the front door routes), the way other emulated discovery operations publish their data-plane URLs, rather than from the hard-coded `*.amazonaws.com` templates. The front door must also route whatever address is published to the iot-data / iot-jobs-data (Jobs and Commands) handlers, so a client built from the announced endpoint reaches them. This is a different defect from the endpointType-validation cause in open issue 20261106T201842Z-iot-refusal-fidelity-10-net-new-causes-createpolicy.md (Cause 5 of 10, which is about refusing `iot:Jobs` and unknown types); that issue does not cover the host the address names and carries none of these ten checks' replay keys.

Traces: iot.command-execution-delete-non-terminal-rejected, iot.command-execution-state-transitions, iot.concurrent-command-execution-creation-respects-limit, iot.delete-command-execution-non-terminal-rejected, iot.e2e-ota-update-package-job-delivery, iot.job-execution-in-progress-timeout-transitions, iot.jobs-data-step-timeout-times-out-one-thing-while-a-reset-twin-survives, iot.ordering-list-command-executions-chronological, iot.update-package-configuration-auto-versioning-applied, iot.visibility-timeout-job-execution-timeout-expires.

## Evidence
Expected: DescribeEndpoint(endpointType="iot:Data-ATS") returns an endpointAddress whose host is served by the emulator, so that `ctx.client("iot-jobs-data", endpoint_url=<endpointAddress>)` can be constructed and its StartCommandExecution / StartNextPendingJobExecution / UpdateJobExecution calls reach the data plane (on real AWS: a live `<id>-ats.iot.<region>.amazonaws.com` host).

Actual: HTTP 200 with endpointAddress `<identifier>-ats.iot.<region>.amazonaws.com` — a public AWS host the emulator does not serve (observed value `jzdx8mw2jjhboa-ats.iot.us-west-2.amazonaws.com`; the identifier, region and port differ per run and are not part of the requirement). Building a client against it is refused before any call:

  File "context.py", line 572, in endpoint_route
    return self._published_target(url, "endpoint_url"), {}
  File "context.py", line 586, in _published_target
    raise PublishedUrlRejected(...)
  simparity.published.PublishedUrlRejected: endpoint_url was given https://jzdx8mw2jjhboa-ats.iot.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:39355/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

All 10 iot checks in the suite fail identically at this step (0/10 passed). Source of the hard-coded hosts: services/service-com-amazonaws-iot-awsiotservice/src/ops/describe_endpoint.rs lines 183-192 (`EndpointType::address`, `format!("{identifier}-ats.iot.{region}.amazonaws.com")` and siblings) and line 349 (`let endpoint_address = endpoint_type.address(&identifier, &region);`).

## Steps to Reproduce
1. Call iot DescribeEndpoint(endpointType="iot:Data-ATS") against the emulator.
2. Observe endpointAddress is `<identifier>-ats.iot.<region>.amazonaws.com` — a host outside the emulator, on no listener it owns.
3. Construct an iot-jobs-data (or iot-data) client with endpoint_url set to that address, as the AWS docs and SDK samples prescribe, and issue any data-plane call (e.g. StartCommandExecution with a commandArn from CreateCommand and a targetArn from CreateThing).
4. The client never reaches the emulator: the harness refuses the published URL (PublishedUrlRejected), and in a sandbox without egress the call fails on name resolution/connection instead.
5. Replay the listed checks (suite simparity-behavioral-aws-v1): every one stops at step 3.
