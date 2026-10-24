Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Boundary ValidationException body omits the model-required fieldName member

_Filed: 2026-10-24T01:02:35Z_
_Operation: PutDeploymentParameter | Class: error-response_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ValidationException` is modelled with BOTH `message` and `fieldName` as `@required`. The emulator emits this shape from two different paths and only one of them carries `fieldName`:

* the handler's own refusals (unsupported `catalog`, an unstorable tag map) build the modelled shape with `field_name` set — correct;
* every refusal produced by the centralized constraint walk (a `@pattern`, `@length` or missing-required violation on `catalog`, `productId`, `agreementId`, `deploymentParameter.name`, `deploymentParameter.secretString`, `clientToken`, `tags`, and likewise a missing required `tagKeys` on UntagResource) is rendered through `Service::override_refusal`, which can only carry a code and a message — so the response body is `{"message":"1 validation error detected: …"}` with NO `fieldName`.

Expected: HTTP 400 `ValidationException` whose body carries both required members, so an SDK deserializing the typed error sees the field the model declares and a client can branch on which member was wrong. Actual: the same error code with a body missing a required member, and two different body shapes for one logical failure depending on whether the handler or the boundary refused the request.

## Evidence

Model: `smithy_query(query="shape",shape="ValidationException")` -> `{message: required, fieldName: required ("The field name associated with the error.")}`, httpError 400. Handler/service: services/.../src/service.rs `override_refusal` (lines 400-421) returns `Refused { code: "ValidationException", message: constraint_envelope(violations)? }` — code and message only, no fieldName channel; contrast services/.../src/state.rs `unsupported_catalog()` and `merge_tags()` which do set `field_name`. Tests: tests/all/validation_messages.rs asserts only `error_code` and `message` for boundary refusals (lines 127-150), and tests/all/support.rs `field_name()` returns the sentinel "<no fieldName>" — so the missing member is unasserted; tests/all/catalog_allowed_values.rs line 104 asserts fieldName only for the handler-built refusal.

## Steps to Reproduce

1. POST /catalogs/AWSMarketplace1/products/prod-held/deployment-parameters with a valid body (catalog violates the ^[a-zA-Z_-]+$ pattern).
2. Observe HTTP 400 with x-amzn-errortype ValidationException and body {"message":"1 validation error detected: …"} — no "fieldName" member, although the model marks it @required.
3. Compare with POST /catalogs/NotARealCatalog/... (handler-built refusal), whose body does carry "fieldName":"catalog".
