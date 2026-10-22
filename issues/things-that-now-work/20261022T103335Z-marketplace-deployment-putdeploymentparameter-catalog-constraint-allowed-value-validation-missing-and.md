Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-deployment.put-deployment-parameter-catalog-validation-rejected marketplace-deployment.put-deployment-parameter-secret-string-too-long-rejected
Issue-Shape-Id: com.amazonaws.marketplacedeployment#AWSMPDeploymentParametersService
Issue-Key: marketplace-deployment.put-deployment-parameter-catalog-validation-rejected|run#13|AssertionError|assert code == "ValidationException", code
Issue-Key: marketplace-deployment.put-deployment-parameter-secret-string-too-long-rejected|run#10|AssertionError|assert code == "ForbiddenException", code
Issue-Commit: adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf

# PutDeploymentParameter: catalog constraint/allowed-value validation missing, and oversized secretString rejected with the wrong error class

> _Note: this issue was identified against the code base at commit `adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeploymentParameter

## Description
PutDeploymentParameter has two distinct input-rejection defects in the emulator. Both are net-new (no open issue mentions marketplace-deployment or PutDeploymentParameter; list_existing_issues for 'marketplace' and 'deployment' returned nothing). THIS ISSUE CONTAINS 2 ROOT CAUSES; both must be closed before submitting.

Cause 1 of 2: the `catalog` path member is never validated — neither its modeled pattern nor the single supported value — so malformed catalogs fall through to resource lookup.
AWS: Smithy (com.amazonaws.marketplacedeployment, PutDeploymentParameter) binds `catalog` as a required @httpLabel targeting shape `Catalog`, a string with @length min 1 / max 64 and @pattern ^[a-zA-Z_-]+$; ValidationException is a modeled error (httpError 400) and ResourceNotFoundException is httpError 404. Recorded behavior: a pattern-violating catalog ("AWS Marketplace!") is rejected with error code `ValidationException` and the standard constraint message built from the request, e.g. "1 validation error detected: Value 'AWS Marketplace!' at 'catalog' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z_-]+"; a pattern-valid but unsupported catalog ("NotTheRealMarketplace") is also rejected with `ValidationException`, example message 'Catalog: The provided Catalog is invalid. Provide a valid Catalog. Supported values are ["AWSMarketplace"].' The requirement in both cases is the code `ValidationException` and that catalog is checked BEFORE productId/agreementId existence; the message text for the pattern case follows the documented 'N validation error(s) detected' rule derived from the member name and its pattern, the allowed-value text is quoted as an example only.
Emulator: both calls return `ResourceNotFoundException` ("ProductId: Request references a resource which does not exist.") — i.e. the catalog value is accepted unchecked and the request proceeds to the product/agreement existence check. The positive control with catalog="AWSMarketplace" does return ResourceNotFoundException as expected, so the ordering layer exists but carries no catalog validation.
Why: a client that reads the error code cannot distinguish a typo'd/unsupported catalog from a genuinely missing entitlement. Retry logic treats a permanent 400-class input error as a 404 resource-propagation problem and retries forever, and the emulator is overly permissive — a caller can pass any catalog string (including one violating the modeled pattern) and never learn it is wrong.
Fix: input validation for PutDeploymentParameter — add the modeled @pattern/@length check on `catalog` plus the allowed-value check (["AWSMarketplace"]), both running before productId/agreementId resolution in the operation handler.
Traces: marketplace-deployment.put-deployment-parameter-catalog-validation-rejected.

Cause 2 of 2: an oversized secretString is rejected with the wrong error class (handler ValidationException instead of the front-door ForbiddenException).
AWS: Smithy models DeploymentParameterInput.secretString as required, @sensitive, @length min 1 / max 15000. Recorded behavior only for the over-limit path: a 15001-character secretString is rejected by the front door (body-size guard) with the unmodeled error code `ForbiddenException` at HTTP 403, message "Forbidden" — before the handler's own ValidationException path is reached. The requirement is the status 403 and the code `ForbiddenException`; "Forbidden" is the recorded message. The same recording's positive control (identical shape, in-limit secretString) still fails later with ResourceNotFoundException, so the rejection is specific to the oversized field.
Emulator: returns `ValidationException` for the 15001-character secretString, i.e. the request reaches the handler's length check instead of being refused one layer earlier.
Why: the request is rejected either way, but the class is wrong: 403/ForbiddenException is a non-retryable front-door refusal with no modeled shape, while 400/ValidationException is a modeled handler error. Clients that branch on ForbiddenException (or that parse the modeled ValidationException envelope and its fieldList) get the opposite of what AWS sends, and SDK error-type dispatch selects a different exception class than in production.
Fix: request-size/front-door guard ahead of the PutDeploymentParameter handler (the oversized-body check must run before handler-level field validation and emit the unmodeled 403 ForbiddenException), not the handler's secretString @length validation.
Traces: marketplace-deployment.put-deployment-parameter-secret-string-too-long-rejected.

## Evidence
Cause 1 — check marketplace-deployment.put-deployment-parameter-catalog-validation-rejected:
  put(catalog="AWSMarketplace")        -> expected ResourceNotFoundException : PASSED
  put(catalog="AWS Marketplace!")      -> expected ValidationException (pattern ^[a-zA-Z_-]+$ violated)
                                          actual   ResourceNotFoundException ("ProductId: Request references a resource which does not exist.")
  AssertionError: ResourceNotFoundException  (assert code == "ValidationException")
  The third leg (catalog="NotTheRealMarketplace" -> ValidationException, 'Catalog: The provided Catalog is invalid. Provide a valid Catalog. Supported values are ["AWSMarketplace"].') was never reached because the second leg already failed; it is unimplemented for the same reason (no catalog validation at all).

Cause 2 — check marketplace-deployment.put-deployment-parameter-secret-string-too-long-rejected:
  control put(secretString="s3cr3t-value-1")  -> ResourceNotFoundException : PASSED
  put(secretString="x"*15001)                 -> expected ForbiddenException, HTTP 403, message "Forbidden"
                                                 actual   ValidationException
  AssertionError: ValidationException  (assert code == "ForbiddenException")

Model cited (smithy_query on services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/smithy-model/com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice.json):
  PutDeploymentParameter POST /catalogs/{catalog}/products/{productId}/deployment-parameters
  catalog -> Catalog: required, httpLabel, length 1..64, pattern ^[a-zA-Z_-]+$
  deploymentParameter.secretString -> SecretString: required, sensitive, length 1..15000
  errors: ValidationException (400), ResourceNotFoundException (404), AccessDeniedException (403), ConflictException (409), ServiceQuotaExceededException (402), ThrottlingException (429), InternalServerException (500)

## Steps to Reproduce
Client: botocore client for "marketplace-deployment" pinned to region us-east-1 (the service serves only us-east-1).

Cause 1:
1. productId = any fabricated id (e.g. "prod-<unique>"), agreementId = any fabricated id (e.g. "agr-<unique>").
2. Call put_deployment_parameter(catalog="AWSMarketplace", productId=productId, agreementId=agreementId, deploymentParameter={"name": "<unique>", "secretString": "s3cr3t-value-1"}) and observe ResourceNotFoundException (positive control — proves not every call is refused).
3. Repeat the same call with catalog="AWS Marketplace!" (violates ^[a-zA-Z_-]+$). Expected error code ValidationException; emulator returns ResourceNotFoundException.
4. Repeat with catalog="NotTheRealMarketplace" (pattern-valid, not a supported catalog). Expected error code ValidationException; emulator returns ResourceNotFoundException.

Cause 2:
1. Call put_deployment_parameter(catalog="AWSMarketplace", productId="prod-<unique>", agreementId="agr-<unique>", deploymentParameter={"name": "<unique>", "secretString": "s3cr3t-value-1"}) and observe ResourceNotFoundException (control).
2. Repeat with deploymentParameter.secretString = "x" * 15001. Expected HTTP 403 with error code ForbiddenException; emulator returns error code ValidationException.

Replay both failing checks listed in trace_ids to confirm the fix.
