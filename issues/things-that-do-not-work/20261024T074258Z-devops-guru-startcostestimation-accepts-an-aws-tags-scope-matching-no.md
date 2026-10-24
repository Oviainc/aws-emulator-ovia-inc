Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-guru.cost-estimation-lifecycle-completes
Issue-Shape-Id: com.amazonaws.devopsguru#CapstoneControlPlaneService
Issue-Key: devops-guru.cost-estimation-lifecycle-completes|_client_error#3|AssertionError|raise AssertionError(f"real AWS refuses StartCostEstimation scoped to a tag no resource carries, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# StartCostEstimation accepts an AWS_TAGS scope matching no tagged resource and returns 200 instead of ResourceNotFoundException (404)

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCostEstimation

## Description
Cause 1 of 1: StartCostEstimation does not validate that the requested ResourceCollection tag scope resolves to anything, so an un-analyzable scope is accepted instead of refused.

AWS: StartCostEstimation (Smithy: PUT /cost-estimation, input StartCostEstimationRequest with required member ResourceCollection -> CostEstimationResourceCollectionFilter; modeled errors include ResourceNotFoundException with httpError 404, plus ValidationException 400 and ConflictException 409). Real AWS refuses a request whose Tags scope (AppBoundaryKey / TagValues) corresponds to no actual tagged AWS resource: the call fails with error code ResourceNotFoundException and HTTP 404, and no cost estimation is created for that scope. Recorded behavior of the refusal message is "Invalid tag in the request" (example text; the requirement is the 404 status and the ResourceNotFoundException code).

Emulator: the request succeeds. The emulator answers HTTP 200 with an empty JSON body ({}), i.e. it accepts the scope and starts an estimation over a scope containing nothing to analyze, with no error raised at all. Replay output: "real AWS refuses StartCostEstimation scoped to a tag no resource carries, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".

Why: a client that uses the refusal to detect a mis-typed AppBoundaryKey or a tag value that was never applied gets a false positive. It believes an estimation is running against the intended resources, then either polls GetCostEstimation forever or reads a fabricated zero-cost estimate for an empty scope and concludes the workload is free. Negative-path code that expects to catch ResourceNotFoundException never runs, so tag-scoping typos silently produce meaningless cost data instead of an actionable error.

Fix: StartCostEstimation's request validation / handler in the devops-guru service crate (services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice). Before any estimation state is created, resolve each CostEstimationResourceCollectionFilter.Tags entry against the tag-scoped resources known to the emulator's state and return the modeled ResourceNotFoundException (HTTP 404) when the scope resolves to nothing; ensure no cost-estimation record is persisted for a refused scope, so a following GetCostEstimation does not report the rejected AppBoundaryKey.

Traces: devops-guru.cost-estimation-lifecycle-completes (the check also asserts, as a positive control, that UPDATE/GET of an AWS_TAGS resource collection round-trips; that part is not implicated here).

## Evidence
Expected (real AWS): StartCostEstimation with ResourceCollection={"Tags":[{"AppBoundaryKey":"<unique>","TagValues":["sim2real-nonexistent"]}]} -> ClientError, HTTP 404, Error.Code == "ResourceNotFoundException" (recorded Error.Message example: "Invalid tag in the request"), and no cost estimate referencing that AppBoundaryKey afterwards.

Actual (emulator): HTTP 200, empty body, no exception. Harness assertion: AssertionError: real AWS refuses StartCostEstimation scoped to a tag no resource carries, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '2', ...}, 'RetryAttempts': 0}}

Model reference: StartCostEstimation errors = [AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400].

## Steps to Reproduce
1. Boot the emulator and create a devops-guru client.
2. Pick an AppBoundaryKey that was never registered via UpdateResourceCollection, e.g. "Devops-guru-sim2real-cost-<unique>".
3. Call start_cost_estimation(ResourceCollection={"Tags": [{"AppBoundaryKey": <that key>, "TagValues": ["sim2real-nonexistent"]}]}).
4. Observe HTTP 200 with an empty body instead of a 404 ResourceNotFoundException.
5. Call get_cost_estimation() and observe whether any state now references the rejected AppBoundaryKey (it must not).
