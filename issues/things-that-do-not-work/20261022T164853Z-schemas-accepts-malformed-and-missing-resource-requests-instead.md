Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-schema-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-schema-errors schemas-registry-resource-policy
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|CreateSchema|4xx->2xx|$.Code
Issue-Key: schemas|CreateSchema|4xx->2xx|$.Message
Issue-Key: schemas|CreateSchema|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: schemas|CreateSchema|4xx->2xx|status
Issue-Key: schemas|DescribeSchema|4xx->2xx|$.Code
Issue-Key: schemas|DescribeSchema|4xx->2xx|$.Message
Issue-Key: schemas|DescribeSchema|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: schemas|DescribeSchema|4xx->2xx|status
Issue-Key: schemas|DeleteSchema|4xx->2xx|$
Issue-Key: schemas|DeleteSchema|4xx->2xx|$headers.content-length
Issue-Key: schemas|DeleteSchema|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: schemas|DeleteSchema|4xx->2xx|status
Issue-Key: schemas|GetResourcePolicy|4xx->2xx|$.Code
Issue-Key: schemas|GetResourcePolicy|4xx->2xx|$.Message
Issue-Key: schemas|GetResourcePolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: schemas|GetResourcePolicy|4xx->2xx|status
Issue-Commit: e556c3079ace9bb74a6c235df6f1f92b13f370bd

# schemas accepts malformed and missing-resource requests instead of failing them

> _Note: this issue was identified against the code base at commit `e556c3079ace9bb74a6c235df6f1f92b13f370bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchema

## Description
Four error paths that AWS takes are not taken at all by the emulator: a malformed schema document is accepted, two operations on a non-existent schema succeed, and a deleted resource policy is still readable. This bundle carries FOUR causes and all four must be closed.

Cause 1 of 4 — CreateSchema accepts Content that is not valid JSON (fixture schemas-schema-errors, step task#2).
  AWS: rejects the request with 400 BadRequestException, Code="MalformedRequest", x-amzn-errortype=BadRequestException; the message is derived from the request and names the parse failure (recorded example: "Content is not valid JSON: invalid character 'o' in literal null (expecting 'u') at offset 2"). Smithy: CreateSchema.Content targets __stringMin1Max100000 and Type is the OpenApi3 enum, with BadRequestException modelled at httpError 400 — a Content body that is not parseable JSON for the declared Type cannot be stored.
  Emulator: returns 201 and stores the schema.
  Why: a client that posts a bad document believes it has a usable schema; every later DescribeSchema/ExportSchema/code-binding call against it then behaves differently from AWS, and the AWS-rejected name is left occupying the registry.
  Fix: CreateSchema input validation (parse Content for the declared Type before mutating state).

Cause 2 of 4 — DescribeSchema on a schema that was never created returns success (schemas-schema-errors#task#3).
  AWS: 404 NotFoundException, Code="ResourceNotFound".
  Emulator: 200 with a schema body.
  Hypothesis: this is the downstream effect of cause 1 — the step names the same schema the rejected CreateSchema used, so AWS has no such schema while the emulator does. It is reported as its own divergence cluster and must be re-replayed after cause 1 is fixed; if it still answers 200, DescribeSchema's existence check is independently broken.
  Fix: CreateSchema validation (cause 1) and DescribeSchema's existence check.

Cause 3 of 4 — DeleteSchema on a schema that was never created returns 204 (schemas-schema-errors#task#4).
  AWS: 404 NotFoundException, Code="ResourceNotFound", with a JSON error body (DeleteSchema's success code is 204 with an empty body, so the status is the only signal a client has).
  Emulator: 204, empty body.
  Hypothesis: downstream of cause 1, as in cause 2; re-replay after fixing it.
  Fix: DeleteSchema's existence check (plus cause 1).

Cause 4 of 4 — GetResourcePolicy still returns a policy after DeleteResourcePolicy (schemas-registry-resource-policy#task#8).
  AWS: once DeleteResourcePolicy (204) has run, GetResourcePolicy answers 404 NotFoundException with Code="ResourceNotFound" and x-amzn-errortype present. Smithy: DeleteResourcePolicy -> Unit at httpError-free 204; GetResourcePolicy models NotFoundException at 404.
  Emulator: 200 with the deleted policy still attached.
  Why: deletion is not observable — a caller cannot tell whether a registry's resource policy was removed, and policy-teardown assertions pass against a policy that should be gone.
  Fix: DeleteResourcePolicy's state mutation (remove the policy from the registry) and/or GetResourcePolicy's lookup.

Traces: schemas-schema-errors#task#2 (representative), schemas-schema-errors#task#3, schemas-schema-errors#task#4, schemas-registry-resource-policy#task#8.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, state_reset_failures=0):
- schemas-schema-errors#task#2 CreateSchema: status expected 400, got 201; $.Code missing (expected 'MalformedRequest'), $.Message missing, $headers.x-amzn-errortype missing (expected 'BadRequestException')
- schemas-schema-errors#task#3 DescribeSchema: status expected 404, got 200; $.Code missing (expected 'ResourceNotFound'); $headers.x-amzn-errortype missing (expected 'NotFoundException')
- schemas-schema-errors#task#4 DeleteSchema: status expected 404, got 204; $ empty_body (expected {'Code': 'ResourceNotFound', 'Message': ...}); $headers.x-amzn-errortype missing (expected 'NotFoundException')
- schemas-registry-resource-policy#task#8 GetResourcePolicy: status expected 404, got 200; $.Code missing (expected 'ResourceNotFound'), $.Message missing
Model evidence: smithy_query(query="operation", operation="CreateSchema,DescribeSchema,DeleteSchema,GetResourcePolicy,DeleteResourcePolicy") on the schemas model — CreateSchema 201 with BadRequestException(400); DescribeSchema/DeleteSchema/GetResourcePolicy all model NotFoundException(404).

## Steps to Reproduce
1. Replay fixture schemas-schema-errors against the emulator.
2. Step task#2 POSTs a CreateSchema whose Content is not valid JSON: the emulator answers 201 instead of 400 with Code=MalformedRequest.
3. Steps task#3/task#4 then find that schema: DescribeSchema answers 200 and DeleteSchema answers 204, where AWS answers 404 ResourceNotFound for both.
4. Replay fixture schemas-registry-resource-policy: after the DeleteResourcePolicy step, step task#8's GetResourcePolicy still answers 200 with the policy instead of 404 ResourceNotFound.
