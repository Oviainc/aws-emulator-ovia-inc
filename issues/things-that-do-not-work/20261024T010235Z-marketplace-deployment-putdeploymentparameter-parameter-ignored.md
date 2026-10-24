Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# expirationDate is stored but never enforced, and an update silently clears it

_Filed: 2026-10-24T01:02:35Z_
_Operation: PutDeploymentParameter | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutDeploymentParameterRequest$expirationDate` is documented as "The date when deployment parameters expire and are scheduled for deletion." The handler copies it onto the record (`record.expiration_date`) but NO read path ever consults it: after the expiration date has passed the parameter remains fully addressable — `ListTagsForResource` keeps returning 200 with its tag map, `TagResource`/`UntagResource` keep succeeding, and a repeat `PutDeploymentParameter` for the same (catalog, productId, agreementId, name) still resolves to the same `deploymentParameterId` rather than minting a new resource. Expected: a parameter whose expirationDate is in the past is expired/deleted, so the tagging operations answer the service's not-found failure and a subsequent put creates a fresh resource.

Second symptom of the same dropped-semantics class: on the update-in-place branch the handler assigns `record.expiration_date = input.expiration_date` unconditionally, so an update that omits `expirationDate` (an optional member) silently erases an expiration the create had set — the request said nothing about expiry yet the resource's scheduled deletion disappears.

## Evidence

Model: `smithy_query(query="shape",shape="PutDeploymentParameterRequest")` -> member `expirationDate` (Timestamp, date-time) documented "The date when deployment parameters expire and are scheduled for deletion." Handler: services/.../src/ops/put_deployment_parameter.rs line 310 (`record.expiration_date = input.expiration_date;`) and line 346 (create path) are the ONLY uses; `grep -r expiration` over the crate shows no other read — state.rs only declares the field (line 184) and prints it in Debug (line 204), and no handler compares it against `SystemTime::now()`. Tests: tests/all/deployment_parameters.rs passes `expirationDate` in one lifecycle row (lines 311, 365-366) but asserts nothing about expiry; no test covers a past expirationDate or an update that omits it.

## Steps to Reproduce

1. Provision a target and PutDeploymentParameter with "expirationDate" set to a timestamp in the past (e.g. 2020-01-01T00:00:00Z).
2. GET /tags/{resourceArn} for the returned ARN — observe 200 with the tag map, instead of the resource being expired/gone.
3. PutDeploymentParameter again for the same catalog/productId/agreementId/name with "expirationDate" set to a future date, then put a third time OMITTING expirationDate.
4. Observe that the stored expiration is silently cleared by the third call even though the request said nothing about expiry.
