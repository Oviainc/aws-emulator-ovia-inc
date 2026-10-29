Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: service-catalog.provision-unassociated-product-rejected
Issue-Shape-Id: com.amazonaws.servicecatalog#AWS242ServiceCatalogService
Issue-Key: service-catalog.provision-unassociated-product-rejected|run#10|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Commit: ff63bbd76f191efb9689374a4f98215607f4bd70

# service-catalog ProvisionProduct is an unimplemented 501 stub: provisioning an unassociated product answers NotImplemented instead of ResourceNotFoundException "No launch paths found for resource: <ProductId>" (1 cause)

> _Note: this issue was identified against the code base at commit `ff63bbd76f191efb9689374a4f98215607f4bd70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ProvisionProduct

## Description
Cause 1 of 1: ProvisionProduct is a scaffolded stub, so the whole provisioning data plane of Service Catalog is unreachable — including its modelled rejection of a product that sits in no portfolio.

AWS: ProvisionProduct resolves the product's launch path before it provisions anything. A product that is in no portfolio (and therefore has no launch path for the calling principal) is refused with HTTP 400 and error code `ResourceNotFoundException`, whose message is `No launch paths found for resource: <ProductId>` — the fixed text is `No launch paths found for resource: ` and the variable part is the `ProductId` from the request (e.g. a `prod-…` id). It is NOT a validation error about the missing `PathId`/`PathName`, even though the model marks those optional-but-required-when-ambiguous. The refusal must have no effect: no provisioned product named by `ProvisionedProductName` is created, so a subsequent SearchProvisionedProducts filtered on that exact name returns empty. Once the product is associated with a portfolio and the calling principal is associated with that portfolio, the byte-identical request succeeds and returns a RecordDetail. Smithy: ProvisionProduct models errors DuplicateResourceException (HTTP 400), InvalidParametersException (HTTP 400) and ResourceNotFoundException (HTTP 400, "The specified resource was not found.", member Message @length 1..1024); required input members are ProvisionToken (@idempotencyToken) and ProvisionedProductName. The specific message text above is recorded behavior only (measured against real AWS in us-west-2, per the check's own header).

Emulator: the call is answered with HTTP 501 and error code `NotImplemented`, message `operation ProvisionProduct is scaffolded but not implemented in this emulator`. services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/src/ops/provision_product.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("ProvisionProduct"))`, so neither the negative path, the effect-absent property, nor the positive control after the portfolio/principal grant can be exercised.

Why: a client cannot provision anything through this emulator at all, and code that distinguishes a governance gap (ResourceNotFoundException, retried after associating a portfolio) from a transport/server fault sees an unmodelled 501 `NotImplemented` that no Service Catalog SDK error-handling branch matches. Error-classification logic, waiters keyed on RecordDetail, and teardown that terminates provisioned products all break rather than taking the documented refusal path.

Fix: the operation's handler — services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/src/ops/provision_product.rs. It must resolve the product's launch paths from Service Catalog state (portfolio-product association plus the calling principal's portfolio association), return ResourceNotFoundException with the message above when none resolve and create no provisioned-product record in that case, and on success record a provisioned product and return its RecordDetail. Delete the SCAFFOLDED marker line once implemented.

Traces: service-catalog.provision-unassociated-product-rejected — assertion `ProvisionProduct on an unassociated product answered 'NotImplemented', expected 'ResourceNotFoundException'`.

## Evidence
Check: service-catalog.provision-unassociated-product-rejected (verdict: mismatch).

Expected (real AWS, measured us-west-2): HTTP 400, Error.Code `ResourceNotFoundException`, Error.Message `No launch paths found for resource: <ProductId>` (fixed text `No launch paths found for resource: `, variable part = the ProductId from the request). No provisioned product is created by the refused call. The identical request succeeds after the product and the calling principal are both associated with a portfolio.

Actual (emulator): HTTP 501, Error.Code `NotImplemented`, Error.Message `operation ProvisionProduct is scaffolded but not implemented in this emulator`.

AssertionError from the replay:
  assert code == "ResourceNotFoundException"
  AssertionError: ProvisionProduct on an unassociated product answered 'NotImplemented', expected 'ResourceNotFoundException'

Source of the stub: services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/src/ops/provision_product.rs lines 4-6 (SCAFFOLDED marker) and line 192 (`Err(crate::scaffolded::Scaffolded::not_implemented("ProvisionProduct"))`).

## Steps to Reproduce
1. Create an S3 bucket and put a small CloudFormation template at a key; use its URL as LoadTemplateFromURL.
2. servicecatalog:CreateProduct with ProductType=CLOUD_FORMATION_TEMPLATE and ProvisioningArtifactParameters{Name=v1,Type=CLOUD_FORMATION_TEMPLATE,Info={LoadTemplateFromURL:<url>}}; capture ProductViewDetail.ProductViewSummary.ProductId and ProvisioningArtifactDetail.Id.
3. Do NOT associate the product with any portfolio.
4. Call servicecatalog:ProvisionProduct with that ProductId, ProvisioningArtifactId, a fresh ProvisionedProductName, a fresh ProvisionToken and ProvisioningParameters=[{Key:Env,Value:prod}].
5. Observe HTTP 501 / NotImplemented. Expected: HTTP 400 ResourceNotFoundException with message "No launch paths found for resource: <ProductId>".
6. Then SearchProvisionedProducts with Filters={SearchQuery:["name:<ProvisionedProductName>"]} and AccessLevelFilter={Key:Account,Value:self} — must be empty; and after CreatePortfolio + AssociateProductWithPortfolio + AssociatePrincipalWithPortfolio the same ProvisionProduct request must succeed and return a RecordDetail.
