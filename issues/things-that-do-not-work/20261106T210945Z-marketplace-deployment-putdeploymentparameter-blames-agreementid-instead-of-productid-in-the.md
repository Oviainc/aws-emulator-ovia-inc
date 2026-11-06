Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-deployment.put-deployment-parameter-nonexistent-agreement-not-found marketplace-deployment.put-deployment-parameter-catalog-validation-rejected marketplace-deployment.put-deployment-parameter-name-pattern-violation-rejected marketplace-deployment.put-deployment-parameter-secret-string-too-long-rejected
Issue-Shape-Id: com.amazonaws.marketplacedeployment#AWSMPDeploymentParametersService
Issue-Key: marketplace-deployment.put-deployment-parameter-nonexistent-agreement-not-found|run#11|AssertionError|assert message == "ProductId: Request references a resource which does not exist.", message
Issue-Key: marketplace-deployment.put-deployment-parameter-catalog-validation-rejected|run#8|AssertionError|assert message == "ProductId: Request references a resource which does not exist.", message
Issue-Key: marketplace-deployment.put-deployment-parameter-name-pattern-violation-rejected|run#7|AssertionError|assert control_message == (
Issue-Key: marketplace-deployment.put-deployment-parameter-secret-string-too-long-rejected|run#7|AssertionError|assert control_message == (
Issue-Commit: 0c248ee895f56d52d4a992168585afb25fa43396

# PutDeploymentParameter blames AgreementId instead of ProductId in the ResourceNotFoundException message for an unresolved target

> _Note: this issue was identified against the code base at commit `0c248ee895f56d52d4a992168585afb25fa43396`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeploymentParameter

## Description
Cause 1 of 1: the ResourceNotFoundException message for a PutDeploymentParameter whose (catalog, productId, agreementId) target is not provisioned names the wrong request member.

AWS: PutDeploymentParameter (POST /catalogs/{catalog}/products/{productId}/deployment-parameters; Smithy: input members catalog @httpLabel, productId @httpLabel, agreementId, all @required; modeled error ResourceNotFoundException, httpError 404, doc "The specified resource wasn't found"). Recorded behavior: for a well-formed request naming a productId/agreementId pair the account was never provisioned with, AWS answers HTTP 404 ResourceNotFoundException whose message is "ProductId: Request references a resource which does not exist." — i.e. the resource addressed by the URI path (the product) is the segment AWS reports as unresolved first, and the blamed member is spelled in UpperCamel ("ProductId"). The message template is the fixed text "<Member>: Request references a resource which does not exist.", where the only variable part is the name of the unresolved member; nothing from the request's ids or the emulator's state appears in it. The model itself says nothing about which segment is blamed (recorded behavior only for the member choice); the status 404 and the code ResourceNotFoundException are modeled and the emulator already gets those right.

Emulator: for the same request it answers HTTP 404 ResourceNotFoundException with message "AgreementId: Request references a resource which does not exist." The ordering is deliberate in the code: state.rs `unresolved_target` returns Some("AgreementId") whenever the store holds no agreement of that id in the catalog, and only returns Some("ProductId") once the agreement resolves but does not cover the product. A request naming neither a held agreement nor a held product — the normal case for a fresh, never-provisioned pair — therefore gets "AgreementId" where AWS says "ProductId". The catalog checks (pattern 400 and unsupported-catalog 400) and the 404 status/code are correct; only the blamed member in the message diverges.

Why: a client that branches on, logs, or surfaces the blamed member is told the agreement does not exist when AWS tells it the product does not exist, so it retries or escalates against the wrong identifier (re-resolving an agreement that is fine, instead of the product id). Any test or runbook asserting AWS's exact message text fails against the emulator. This refusal is on the primary create path of the operation, so every unprovisioned-target call is affected. It also hides the checks that run after the target resolution in the suite: three of the four failing checks never reach their own assertions (catalog allowed-value message, deploymentParameter.name pattern message, oversized secretString ForbiddenException), because this positive-control message assertion fires first — those behaviors are unverified until this is fixed.

Fix: the target-resolution order / blamed-member selection in this service's state, i.e. the function that picks which segment of (catalog, productId, agreementId) is reported unresolved, consumed by the PutDeploymentParameter handler before it mints or updates a record. The product must be judged before the agreement so that a request resolving neither is blamed on ProductId; the agreement stays the blamed member only when the product itself resolves and the agreement does not cover it (if a recording ever shows that case). The message text itself is already correct apart from the member name.

Traces: marketplace-deployment.put-deployment-parameter-nonexistent-agreement-not-found, marketplace-deployment.put-deployment-parameter-catalog-validation-rejected, marketplace-deployment.put-deployment-parameter-name-pattern-violation-rejected, marketplace-deployment.put-deployment-parameter-secret-string-too-long-rejected. Code citations: services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/state.rs lines 658-687 (`unresolved_target`, returns "AgreementId" first) and lines 1103-1116 (`target_not_found`, builds the message); services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/ops/put_deployment_parameter.rs lines 324-331 (handler call site). No existing open issue covers this: the open marketplace-deployment issues are 20261024T161623Z-marketplace-deployment-putdeploymentparameter-event-propagation.md (secretString not materialised in Secrets Manager) and 20261028T133444Z-marketplace-deployment-listtagsforresource-maintainability--r2.md (character-class maintainability), neither of which concerns the not-found blame member.

## Evidence
All four failing checks call PutDeploymentParameter(catalog="AWSMarketplace", productId=<unique, never provisioned>, agreementId=<unique, never provisioned>, deploymentParameter={name, secretString}).

Expected (AWS, per the replayed checks): HTTP 404, Error.Code = "ResourceNotFoundException", Error.Message = "ProductId: Request references a resource which does not exist."

Actual (emulator): HTTP 404, Error.Code = "ResourceNotFoundException" (correct), Error.Message = "AgreementId: Request references a resource which does not exist."

Assertion from the suite, e.g. check put-deployment-parameter-nonexistent-agreement-not-found line 45:
  assert message == "ProductId: Request references a resource which does not exist.", message
  AssertionError: AgreementId: Request references a resource which does not exist.

Message anatomy: fixed text = "<Member>: Request references a resource which does not exist."; the only variable part is <Member>, the name of the unresolved request member (AWS reports "ProductId" for a pair that resolves to nothing). No per-run value (id, timestamp, request id) appears in the message, so the divergence is purely in the blamed member.

Source of the divergence (src/state.rs, `unresolved_target`):
  None => Some("AgreementId"),
  Some(agreement) if !agreement.covers(product_id) => Some("ProductId"),
  Some(_) => None,

## Steps to Reproduce
1. Boot the emulator and create a botocore client for "marketplace-deployment" pinned to region us-east-1.
2. Call put_deployment_parameter(catalog="AWSMarketplace", productId="prod-<unique>", agreementId="agr-<unique>" (neither provisioned in the store), deploymentParameter={"name": "sim2real-dp-<unique>", "secretString": "s3cr3t-value-1"}).
3. Observe the ClientError: Error.Code is ResourceNotFoundException at HTTP 404 (correct) but Error.Message is "AgreementId: Request references a resource which does not exist." where AWS returns "ProductId: Request references a resource which does not exist."
4. Re-replay the four listed checks; the three non-ordering checks should then proceed past their positive control to their own catalog/name-pattern/oversized-secret assertions.
