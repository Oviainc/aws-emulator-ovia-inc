Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-deployment.put-deployment-parameter-nonexistent-agreement-not-found marketplace-deployment.put-deployment-parameter-catalog-validation-rejected marketplace-deployment.put-deployment-parameter-name-pattern-violation-rejected marketplace-deployment.put-deployment-parameter-secret-string-too-long-rejected
Issue-Shape-Id: com.amazonaws.marketplacedeployment#AWSMPDeploymentParametersService
Issue-Key: marketplace-deployment.put-deployment-parameter-nonexistent-agreement-not-found|_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: marketplace-deployment.put-deployment-parameter-catalog-validation-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: marketplace-deployment.put-deployment-parameter-name-pattern-violation-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: marketplace-deployment.put-deployment-parameter-secret-string-too-long-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# PutDeploymentParameter fails open: a never-provisioned productId/agreementId is auto-provisioned and answered 200 instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeploymentParameter

## Description
Cause 1 of 1: PutDeploymentParameter does not resolve the (catalog, productId, agreementId) target it is aimed at. It RECORDS the target named by the request and then serves the write, so any syntactically valid identifier triple succeeds and a deployment parameter is minted against an agreement that was never provisioned.

Failing check ids for this cause: marketplace-deployment.put-deployment-parameter-nonexistent-agreement-not-found, marketplace-deployment.put-deployment-parameter-catalog-validation-rejected, marketplace-deployment.put-deployment-parameter-name-pattern-violation-rejected, marketplace-deployment.put-deployment-parameter-secret-string-too-long-rejected.

AWS: PutDeploymentParameter is 'targeted by catalog and agreementId' for the product its URI addresses, and models ResourceNotFoundException ('The specified resource wasn't found.') at HTTP 404 (Smithy: PutDeploymentParameter errors include ResourceNotFoundException, errorKind client, httpError 404; http POST /catalogs/{catalog}/products/{productId}/deployment-parameters, success 200). An agreement/product is created entirely out of band of this API — the model declares no operation that creates either — so the service RESOLVES the target rather than accepting it. Recorded behavior: a well-formed request whose productId/agreementId name nothing the account holds is refused 404 with code ResourceNotFoundException; the recorded message, as an example, is 'ProductId: Request references a resource which does not exist.' The requirement here is the 404 status and the ResourceNotFoundException code (and that the unresolved target is named first, productId before agreementId); the exact sentence is an example of the recorded text.

Emulator: HTTP 200 with a freshly minted resourceArn/deploymentParameterId and an empty tags map, for every triple. Replay evidence: put_deployment_parameter(catalog='AWSMarketplace', productId=<unique never-provisioned>, agreementId=<unique never-provisioned>, deploymentParameter={name,secretString}) returned HTTPStatusCode 200 with deploymentParameterId 'dp-0000000000000001' and resourceArn 'arn:aws:aws-marketplace:us-east-1:...:deployment-parameter/dp-0000000000000001'. The handler states this as a deliberate design: 'the write ESTABLISHES the target it names' and calls state.provision_target(catalog, product_id, agreement_id) after storing the record; the state's holds_target() predicate exists but is never consulted as a gate on the write.

This is a REGRESSION of the fixed issue 20261022T052035Z-marketplace-deployment-putdeploymentparameter-fails-open-nonexistent-product-agreement-is-created.md, reintroduced by the remedy chosen for the fixed issue 20261024T010235Z-marketplace-deployment-putdeploymentparameter-wrong-behavior.md ('A fresh emulator holds no agreements, so every PutDeploymentParameter is a 404'). That issue's real gap was that no target was ever reachable over the wire; auto-provisioning on write removes the 404 for EVERY target instead, which is the opposite error. The two must be reconciled by making targets exist up front (default/fixture-seeded agreements in the state, or a seeding seam a client can reach) while the write continues to RESOLVE its target and refuse an unheld one.

Why: the existence check is the operation's only authorization-shaped gate, so a client gets a 200 and a resourceArn for an agreement it does not hold. Code that branches on ResourceNotFoundException to distinguish 'not entitled / not fulfilled yet' from 'written' never takes that branch, and a Quick Launch / CloudFormation dynamic-reference flow validated against the emulator will appear to provision secrets for agreements that do not exist, then fail only against real AWS. It also makes every negative test of this operation untrustworthy: all four suite checks use the unprovisioned-target 404 as their positive control, so the emulator's fail-open masks whatever the later catalog-pattern, catalog-allowed-value, name-pattern and oversized-secretString assertions would have found — those assertions were never reached and their state is unknown.

Fix: the operation's handler plus its state. The handler must resolve the target against the agreement store before minting or updating anything and return the modelled ResourceNotFoundException (404) when the store holds no agreement in that catalog covering that product, naming the unresolved identifier (productId first, then agreementId); provision_target must stop being the write's own precondition-satisfier, and the store must instead carry targets established before the request (default-seeded records or a reachable seeding seam) so that a held target still yields the documented 200 create-or-update. The catalog allowed-value check must keep running first, so a bad catalog stays a 400 ValidationException and does not become a 404.

Traces: marketplace-deployment.put-deployment-parameter-nonexistent-agreement-not-found (asserts 404 ResourceNotFoundException for a never-provisioned target, and that it differs from the malformed-catalog ValidationException); marketplace-deployment.put-deployment-parameter-catalog-validation-rejected; marketplace-deployment.put-deployment-parameter-name-pattern-violation-rejected; marketplace-deployment.put-deployment-parameter-secret-string-too-long-rejected (the last three use the same 404 as their positive control and fail on it before reaching their own assertion). Code: services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/ops/put_deployment_parameter.rs lines 269-285 (the comment declaring the target is not resolved) and line 405 (store.provision_target(...)); services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/state.rs lines 485-526 (provision_target and the unused holds_target) and seed_agreements at ~line 819.

## Evidence
Expected (AWS, per the Smithy model and the recording): HTTP 404, error code ResourceNotFoundException, for PutDeploymentParameter against a productId/agreementId the account does not hold. Recorded message example: 'ProductId: Request references a resource which does not exist.'

Actual (emulator): HTTP 200. Replay output for marketplace-deployment.put-deployment-parameter-nonexistent-agreement-not-found:
  AssertionError: expected the call to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'agreementId': 'no-such-agreement-469175bb629e', 'deploymentParameterId': 'dp-0000000000000001', 'resourceArn': 'arn:aws:aws-marketplace:us-east-1:000000000000:deployment-parameter/dp-0000000000000001', 'tags': {}}
The same 200 is returned for the other three checks' positive controls (productId 'prod-<unique>', agreementId 'agr-<unique>').

Smithy: PutDeploymentParameter errors = AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400); ResourceNotFoundException = {message: required}, httpError 404. No operation in com.amazonaws.marketplacedeployment creates a product or an agreement, so the target is a precondition the service resolves, never one the write creates.

Source: src/ops/put_deployment_parameter.rs documents 'the write ESTABLISHES the target it names' and calls store.provision_target(catalog, product_id, agreement_id) (line 405) after inserting the record; src/state.rs defines holds_target() (line 522) but no call site gates the write on it.

Related filed issues: 20261022T052035Z-marketplace-deployment-putdeploymentparameter-fails-open-nonexistent-product-agreement-is-created.md (fixed; this is the same defect returning) and 20261024T010235Z-marketplace-deployment-putdeploymentparameter-wrong-behavior.md (fixed; its chosen remedy is what reintroduced the fail-open).

## Steps to Reproduce
1. Start the emulator fresh, with no in-process agreement seeding.
2. Create a botocore client for 'marketplace-deployment' in us-east-1.
3. Call put_deployment_parameter(catalog='AWSMarketplace', productId='no-such-product-<random>', agreementId='no-such-agreement-<random>', deploymentParameter={'name': 'sim2real-dp-<random>', 'secretString': 's3cr3t-value-1'}) — identifiers that satisfy the ^[A-Za-z0-9_/-]+$ pattern but were never provisioned.
4. Observe HTTP 200 with a minted deploymentParameterId and resourceArn. Expected: HTTP 404 with error code ResourceNotFoundException naming the unresolved productId.
5. Confirm the write really landed: call list_tags_for_resource(resourceArn=<the returned ARN>) and observe it resolves, i.e. a deployment parameter now exists for an agreement that was never provisioned.
6. Re-run the four checks in trace_ids: each fails at its first assertion, the ResourceNotFoundException positive control, so their catalog-pattern / catalog-allowed-value / name-pattern / oversized-secretString assertions are never evaluated.
