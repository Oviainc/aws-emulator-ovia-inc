Issue-Source: trace-sweep-divergence
Issue-Trace: evs-explore-missing-environment
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: evs-explore-missing-environment
Issue-Shape-Id: com.amazonaws.evs#AmazonElasticVMwareService
Issue-Key: evs|GetEnvironment|4xx->4xx|$.message
Issue-Key: evs|GetEnvironment|4xx->4xx|status
Issue-Key: evs|ListEnvironmentVlans|4xx->4xx|$.message
Issue-Key: evs|ListEnvironmentVlans|4xx->4xx|status
Issue-Key: evs|DeleteEnvironment|4xx->4xx|$.message
Issue-Key: evs|DeleteEnvironment|4xx->4xx|status
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# Environment-not-found errors return HTTP 404 instead of the recorded 400

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEnvironment, ListEnvironmentVlans, DeleteEnvironment

## Description
Every evs operation that looks up an environment by id answers a missing environment with HTTP 404, while the recorded service answers HTTP 400; the body text also differs from the recorded not-found phrasing. One shared defect in how the evs not-found error is mapped to a wire status, observed on three operations.

Shared AWS basis: Smithy declares ResourceNotFoundException @error(client) with @httpError(404) and required members message, resourceId, resourceType, and binds it to GetEnvironment, ListEnvironmentVlans and DeleteEnvironment (Smithy: GetEnvironment/ListEnvironmentVlans/DeleteEnvironment errors = [ResourceNotFoundException, ValidationException]). The RECORDED wire status for this awsJson1_0 service is 400, not the modelled 404 — the recording is the authority for the status, so 400 is the requirement and the @httpError(404) trait must not be used verbatim for this protocol. The recorded not-found message is built from the request: it names the requested environment id, in the form 'Environment <environmentId> was not found' (recorded example: 'Environment env-0000000000 was not found'). The error code/__type matched in every step below, so only the status and the message rule diverge.

Cause 1 of 3: GetEnvironment on an unknown environment id.
AWS: HTTP 400, ResourceNotFoundException, message 'Environment env-0000000000 was not found'.
Emulator: HTTP 404 with message 'Environment env-0000000000 does not exist.'

Cause 2 of 3: ListEnvironmentVlans on an unknown environment id.
AWS: HTTP 400, ResourceNotFoundException, message 'Environment env-0000000000 was not found'.
Emulator: HTTP 404 with message 'Environment env-0000000000 does not exist.'

Cause 3 of 3: DeleteEnvironment on an unknown environment id.
AWS: HTTP 400, ResourceNotFoundException, message 'Environment env-0000000000 was not found'.
Emulator: HTTP 404 with message 'Environment env-0000000000 does not exist.'

Why: SDKs and proxies classify on the HTTP status before the body. A 404 is treated by generic HTTP middleware (and by retry/circuit-breaker layers) as a routing or endpoint problem rather than a modelled client error, so a client that correctly handles AWS's 400 ResourceNotFoundException can surface 'unknown endpoint'/'operation not found' instead of 'environment does not exist'. The message rule is request-derived, so a client or test asserting the AWS wording of the not-found text also fails.

Fix: the evs error-serializer / not-found mapping shared by these three handlers — emit the not-found error with the status the service actually returns on the wire (400) instead of deriving it from @httpError(404), and render the message with the recorded request-derived template that names the environment id.

Traces: evs-explore-missing-environment#task#1 (GetEnvironment), evs-explore-missing-environment#task#4 (ListEnvironmentVlans), evs-explore-missing-environment#task#6 (DeleteEnvironment).

## Evidence
Fixture evs-explore-missing-environment.
Step task#1 (GetEnvironment): expected status 400, got 404; $.message expected 'Environment env-0000000000 was not found', got 'Environment env-0000000000 does not exist.'
Step task#4 (ListEnvironmentVlans): expected status 400, got 404; same $.message divergence.
Step task#6 (DeleteEnvironment): expected status 400, got 404; same $.message divergence.
Error code/__type matched in all three; only status and message diverged.

## Steps to Reproduce
1. Boot the emulator and replay fixture evs-explore-missing-environment.
2. Steps task#1/task#4/task#6 call GetEnvironment, ListEnvironmentVlans and DeleteEnvironment with environmentId env-0000000000, which was never created.
3. Each response carries HTTP 404 and the message 'Environment <id> does not exist.'; the recording shows HTTP 400 with 'Environment <id> was not found'.
