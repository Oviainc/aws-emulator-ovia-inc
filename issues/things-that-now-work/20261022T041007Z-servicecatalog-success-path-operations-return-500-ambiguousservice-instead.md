Issue-Source: trace-sweep-divergence
Issue-Trace: servicecatalog-constraint-lifecycle
Issue-Step: setup#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicecatalog-constraint-lifecycle servicecatalog-provisioning-artifact-lifecycle servicecatalog-portfolio-lifecycle servicecatalog-portfolio-principal-association servicecatalog-product-lifecycle servicecatalog-service-action-lifecycle servicecatalog-tag-option-lifecycle
Issue-Shape-Id: com.amazonaws.servicecatalog#AWS242ServiceCatalogService
Issue-Key: servicecatalog|CreateProduct|2xx->5xx|status
Issue-Key: servicecatalog|CreatePortfolio|2xx->5xx|status
Issue-Key: servicecatalog|CreateProduct|2xx->5xx|$.ProductViewDetail
Issue-Key: servicecatalog|CreateProduct|2xx->5xx|$.ProvisioningArtifactDetail
Issue-Key: servicecatalog|CreateServiceAction|2xx->5xx|status
Issue-Key: servicecatalog|CreateTagOption|2xx->5xx|status
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# ServiceCatalog success-path operations return 500 AmbiguousService instead of modeled 200 results

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProduct

## Description
SHARED FAULT (5 causes below): every ServiceCatalog request is refused by the router before any handler runs, with HTTP 500 and an error envelope whose __type is 'AmbiguousService' and whose message reads (example) "2 emulated services all claim requests for 'servicecatalog': servicecatalog, servicecatalog; the request is rejected rather than served by an arbitrary one". Two emulated services declare the same sigv4 signing name: Smithy service_info for AWS242ServiceCatalogService (version 2015-12-10, awsJson1_1) gives sigv4Name 'servicecatalog', endpointPrefix 'servicecatalog'; Smithy service_info for AWS242AppRegistry (version 2020-06-24, restJson1) also gives sigv4Name 'servicecatalog', but endpointPrefix 'servicecatalog-appregistry'. The default can_handle seam claims a request whose resolved route key equals the service's signing name, so both services claim every 'servicecatalog'-signed request and the router refuses it. Nothing in the ServiceCatalog operation handlers is reached at all. This issue bundles the five success-path (expected-200) clusters; the ten error-path clusters are filed as the companion issue 'ServiceCatalog error-path operations return 500 AmbiguousService ...'. Fixing the routing claim is a prerequisite for both; the 45 blocked steps in this corpus are downstream of these failed setup steps.

Cause 1 of 5: CreateProduct (success). AWS: Smithy CreateProduct on AWS242ServiceCatalogService returns HTTP 200 with CreateProductOutput { ProductViewDetail, ProvisioningArtifactDetail, Tags }; its only modeled errors are InvalidParametersException, LimitExceededException and TagOptionNotMigratedException, all httpError 400. Emulator: HTTP 500, __type 'AmbiguousService', no output members. Why: a client cannot create a product at all, and the generated ProductId/ProvisioningArtifactId it needs for every later call never exist, so the whole catalog workflow stops at its first step. Fix: the service's request-claim seam (can_handle in services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/src/service.rs and the matching seam in the AppRegistry crate) must disambiguate the shared signing name — by wire protocol, by the host's endpoint prefix, or by the X-Amz-Target service prefix — so exactly one service claims an awsJson1_1 'AWS242ServiceCatalogService.*' request. Traces: servicecatalog-constraint-lifecycle#setup#3 (also servicecatalog-provisioning-artifact-lifecycle#setup#3).

Cause 2 of 5: CreatePortfolio (success). AWS: Smithy CreatePortfolio returns HTTP 200 with CreatePortfolioOutput { PortfolioDetail, Tags }; modeled errors are InvalidParametersException, LimitExceededException, TagOptionNotMigratedException (400). Emulator: HTTP 500 __type 'AmbiguousService'. Why: no portfolio can be created, so portfolio association, constraint and principal workflows have no resource to act on. Fix: same request-claim seam as Cause 1. Traces: servicecatalog-constraint-lifecycle#setup#4 (also servicecatalog-portfolio-lifecycle#setup#1, servicecatalog-portfolio-principal-association#setup#2).

Cause 3 of 5: CreateProduct response shape (success). AWS: Smithy CreateProductOutput carries ProductViewDetail (with ProductViewSummary{Id, ProductId, Name, Owner, ShortDescription, SupportEmail, Type, HasDefaultPath}, ProductARN, CreatedTime, Status) and ProvisioningArtifactDetail (Id, Name, Description, Type, Active, Guidance, CreatedTime). Emulator: HTTP 500 with neither member present (both reported missing by the replay). Why: an SDK caller gets a 500 where it expects a fully populated detail document, so it can neither read the product's identifiers nor the created provisioning artifact. Fix: same request-claim seam as Cause 1; once the request reaches the handler, the CreateProduct serializer must emit both output members. Traces: servicecatalog-product-lifecycle#task#5.

Cause 4 of 5: CreateServiceAction (success). AWS: Smithy CreateServiceAction returns HTTP 200 with CreateServiceActionOutput { ServiceActionDetail }; modeled errors InvalidParametersException, LimitExceededException (400). Emulator: HTTP 500 __type 'AmbiguousService'. Why: service actions cannot be created, so every later Describe/Update/Delete on that action has no subject. Fix: same request-claim seam as Cause 1. Traces: servicecatalog-service-action-lifecycle#setup#1.

Cause 5 of 5: CreateTagOption (success). AWS: Smithy CreateTagOption returns HTTP 200 with CreateTagOptionOutput { TagOptionDetail }; modeled errors DuplicateResourceException, LimitExceededException, TagOptionNotMigratedException (400). Emulator: HTTP 500 __type 'AmbiguousService'. Why: no TagOption can be created, so the TagOption lifecycle (duplicate detection, describe, list) is unusable. Fix: same request-claim seam as Cause 1. Traces: servicecatalog-tag-option-lifecycle#setup#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, data listener accepting; bootstrap_signal false, state_reset_failures 0). Expected vs actual, per cause:
1) CreateProduct, servicecatalog-constraint-lifecycle#setup#3: expected status 200, got 500 (cluster also covers servicecatalog-provisioning-artifact-lifecycle#setup#3).
2) CreatePortfolio, servicecatalog-constraint-lifecycle#setup#4: expected 200, got 500 (cluster also covers servicecatalog-portfolio-lifecycle#setup#1, servicecatalog-portfolio-principal-association#setup#2).
3) CreateProduct, servicecatalog-product-lifecycle#task#5: expected 200, got 500; $.ProductViewDetail missing (expected a ProductViewDetail with Status 'CREATED' and a ProductViewSummary of Type 'CLOUD_FORMATION_TEMPLATE'); $.ProvisioningArtifactDetail missing (expected Active true, Guidance 'DEFAULT', Name 'v1').
4) CreateServiceAction, servicecatalog-service-action-lifecycle#setup#1: expected 200, got 500.
5) CreateTagOption, servicecatalog-tag-option-lifecycle#setup#1: expected 200, got 500.
The refusal body observed on the sibling clusters identifies the cause verbatim: __type 'AmbiguousService', message "2 emulated services all claim requests for 'servicecatalog': servicecatalog, servicecatalog; the request is rejected rather than served by an arbitrary one". Smithy evidence for the collision: sigv4Name 'servicecatalog' on both AWS242ServiceCatalogService (endpointPrefix 'servicecatalog', awsJson1_1) and AWS242AppRegistry (endpointPrefix 'servicecatalog-appregistry', restJson1). Corpus totals: 79 steps, 4 passed, 30 mismatched, 45 blocked, score 5.1%.

## Steps to Reproduce
1) Boot the emulator from this checkout.
2) Replay fixture servicecatalog-constraint-lifecycle and observe step setup#3 (CreateProduct): HTTP 500 with __type 'AmbiguousService' instead of 200 with CreateProductOutput.
3) Replay the other bundled steps for the remaining causes: servicecatalog-constraint-lifecycle#setup#4 (CreatePortfolio), servicecatalog-product-lifecycle#task#5 (CreateProduct response shape), servicecatalog-service-action-lifecycle#setup#1 (CreateServiceAction), servicecatalog-tag-option-lifecycle#setup#1 (CreateTagOption) — each returns 500 AmbiguousService.
4) Equivalent by hand: POST / to the emulator's data listener with Authorization credential scope signing name 'servicecatalog' and X-Amz-Target 'AWS242ServiceCatalogService.CreateProduct' plus a valid awsJson1_1 body; the router refuses with 500 AmbiguousService before any handler runs.
5) After narrowing the claim seam so only one service claims the request, re-replay all five fixture steps listed above and confirm each returns its modeled 200 body.
