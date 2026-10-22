Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-registry-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-registry-lifecycle schemas-code-binding-lifecycle schemas-discover-schema-from-events schemas-registry-resource-policy schemas-schema-errors schemas-schema-lifecycle
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|DescribeRegistry|4xx->4xx|$.Code
Issue-Key: schemas|DescribeRegistry|4xx->4xx|$.Message
Issue-Key: schemas|DescribeCodeBinding|4xx->4xx|$.Code
Issue-Key: schemas|DescribeCodeBinding|4xx->4xx|$.Message
Issue-Key: schemas|GetDiscoveredSchema|4xx->4xx|$.Code
Issue-Key: schemas|GetDiscoveredSchema|4xx->4xx|$.Message
Issue-Key: schemas|CreateRegistry|4xx->4xx|$.Code
Issue-Key: schemas|DeleteRegistry|4xx->4xx|$.Code
Issue-Key: schemas|DeleteRegistry|4xx->4xx|$.Message
Issue-Key: schemas|GetResourcePolicy|4xx->4xx|$.Code
Issue-Key: schemas|GetResourcePolicy|4xx->4xx|$.Message
Issue-Key: schemas|PutResourcePolicy|4xx->4xx|$.Code
Issue-Key: schemas|PutResourcePolicy|4xx->4xx|$.Message
Issue-Key: schemas|UpdateSchema|4xx->4xx|$.Code
Issue-Key: schemas|UpdateSchema|4xx->4xx|$.Message
Issue-Key: schemas|DescribeSchema|4xx->4xx|$.Code
Issue-Key: schemas|DescribeSchema|4xx->4xx|$.Message
Issue-Commit: e556c3079ace9bb74a6c235df6f1f92b13f370bd

# schemas error bodies put the exception shape name in Code instead of the AWS error code

> _Note: this issue was identified against the code base at commit `e556c3079ace9bb74a6c235df6f1f92b13f370bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRegistry

## Description
Every modelled schemas error structure (NotFoundException, ConflictException, BadRequestException, PreconditionFailedException) has two required members, `Code` ("The error code.") and `Message`. On AWS the exception *shape name* travels in the `x-amzn-errortype` header while `Code` carries the service's error code — `ResourceNotFound`, `ResourceAlreadyExist`, `MalformedRequest`, `Conflict`. The emulator instead copies the shape name into `Code` on every error path. HTTP statuses are correct throughout; only the `Code` value diverges (message texts also differ, but those are examples, not requirements, except where noted). This bundle carries NINE causes, one per diverging operation, and all nine must be closed.

Smithy (services/service-com-amazonaws-schemas-schemas/smithy-model/com-amazonaws-schemas-schemas.json): NotFoundException/ConflictException/BadRequestException/PreconditionFailedException each = {Code: required __string "The error code.", Message: required __string}, httpError 404/409/400/412.

Cause 1 of 9 — DescribeRegistry on a missing registry (fixture schemas-registry-lifecycle, step setup#1; same cluster also covers teardown#9).
  AWS: 404 with Code="ResourceNotFound" and x-amzn-errortype=NotFoundException.
  Emulator: 404 with Code="NotFoundException".
  Why: clients that branch on the `Code` field (the SDK error-code accessor for this service) see an unknown code and cannot distinguish a missing registry from any other 404.
  Fix: the schemas error-rendering path that builds NotFoundException for DescribeRegistry — set Code to the service error code, leave the shape name to x-amzn-errortype.

Cause 2 of 9 — DeleteRegistry on a missing registry (schemas-registry-lifecycle#teardown#10).
  AWS: 404, Code="ResourceNotFound". Emulator: Code="NotFoundException".
  Why/Fix: as cause 1, in the DeleteRegistry handler's not-found error.

Cause 3 of 9 — CreateRegistry on an existing registry (schemas-registry-lifecycle#task#3).
  AWS: 409, Code="ResourceAlreadyExist" (ConflictException). Emulator: 409, Code="ConflictException".
  Why: idempotent create-or-reuse logic keyed on the conflict code misfires.
  Fix: CreateRegistry's conflict error construction.

Cause 4 of 9 — DescribeSchema on a deleted schema (schemas-schema-lifecycle#task#11).
  AWS: 404, Code="ResourceNotFound". Emulator: Code="NotFoundException".
  Fix: DescribeSchema not-found error construction.

Cause 5 of 9 — UpdateSchema on a missing schema (schemas-schema-errors#task#5).
  AWS: 404, Code="ResourceNotFound". Emulator: Code="NotFoundException".
  Fix: UpdateSchema not-found error construction.

Cause 6 of 9 — DescribeCodeBinding with no binding for the requested language (schemas-code-binding-lifecycle#task#3).
  AWS: 404, Code="ResourceNotFound". Emulator: Code="NotFoundException".
  Fix: DescribeCodeBinding not-found error construction.

Cause 7 of 9 — GetResourcePolicy for a registry with no policy (schemas-registry-resource-policy#task#2).
  AWS: 404, Code="ResourceNotFound". Emulator: Code="NotFoundException".
  Fix: GetResourcePolicy not-found error construction.

Cause 8 of 9 — PutResourcePolicy with a stale RevisionId (schemas-registry-resource-policy#task#6).
  AWS: 412, Code="Conflict" (PreconditionFailedException). Emulator: 412, Code="PreconditionFailedException".
  Why: optimistic-concurrency retry loops keyed on Code=="Conflict" never retry.
  Fix: PutResourcePolicy's revision-mismatch error construction.

Cause 9 of 9 — GetDiscoveredSchema with a non-JSON event (schemas-discover-schema-from-events#task#2).
  AWS: 400, Code="MalformedRequest" (BadRequestException); the message is built from the request and names the failing event index, e.g. "Fail to parse event index: [0]. Reason: Failed to parse JSON content".
  Emulator: 400, Code="BadRequestException".
  Fix: GetDiscoveredSchema input validation's error construction.

Traces: see the per-cause fixture#step above; the representative is schemas-registry-lifecycle#setup#1 and the other eight are listed in trace_ids.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (score 8.7%, 0 state_reset_failures):
- schemas-registry-lifecycle#setup#1 DescribeRegistry 404->404: $.Code value_mismatch (expected 'ResourceNotFound', got 'NotFoundException')
- schemas-registry-lifecycle#teardown#10 DeleteRegistry 404->404: $.Code expected 'ResourceNotFound', got 'NotFoundException'
- schemas-registry-lifecycle#task#3 CreateRegistry 409->409: $.Code expected 'ResourceAlreadyExist', got 'ConflictException'
- schemas-schema-lifecycle#task#11 DescribeSchema 404->404: $.Code expected 'ResourceNotFound', got 'NotFoundException'
- schemas-schema-errors#task#5 UpdateSchema 404->404: $.Code expected 'ResourceNotFound', got 'NotFoundException'
- schemas-code-binding-lifecycle#task#3 DescribeCodeBinding 404->404: $.Code expected 'ResourceNotFound', got 'NotFoundException'
- schemas-registry-resource-policy#task#2 GetResourcePolicy 404->404: $.Code expected 'ResourceNotFound', got 'NotFoundException'
- schemas-registry-resource-policy#task#6 PutResourcePolicy 412->412: $.Code expected 'Conflict', got 'PreconditionFailedException'
- schemas-discover-schema-from-events#task#2 GetDiscoveredSchema 400->400: $.Code expected 'MalformedRequest', got 'BadRequestException'
Model evidence: smithy_query(query="shape", shape="NotFoundException,ConflictException,BadRequestException,PreconditionFailedException", model_path="services/service-com-amazonaws-schemas-schemas/smithy-model/com-amazonaws-schemas-schemas.json") shows Code as a required member documented as "The error code." distinct from the shape name.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Replay fixture schemas-registry-lifecycle step setup#1 (DescribeRegistry on a registry that does not exist) and read the JSON error body: $.Code is 'NotFoundException' where the recording has 'ResourceNotFound'.
3. Repeat for each fixture#step in trace_ids: schemas-registry-lifecycle#task#3 (expect Code=ResourceAlreadyExist), schemas-registry-resource-policy#task#6 (expect Code=Conflict), schemas-discover-schema-from-events#task#2 (expect Code=MalformedRequest), and the remaining not-found paths (expect Code=ResourceNotFound).
4. In every case the HTTP status already matches; only $.Code diverges.
