Issue-Source: trace-sweep-divergence
Issue-Trace: servicecatalog-constraint-lifecycle
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicecatalog-constraint-lifecycle servicecatalog-portfolio-lifecycle servicecatalog-portfolio-principal-association servicecatalog-product-lifecycle servicecatalog-provisioning-artifact-lifecycle servicecatalog-service-action-lifecycle servicecatalog-tag-option-lifecycle
Issue-Shape-Id: com.amazonaws.servicecatalog#AWS242ServiceCatalogService
Issue-Key: servicecatalog|DescribeConstraint|4xx->5xx|$.Message
Issue-Key: servicecatalog|DescribeConstraint|4xx->5xx|$.__type
Issue-Key: servicecatalog|DescribeConstraint|4xx->5xx|status
Issue-Key: servicecatalog|DescribePortfolio|4xx->5xx|$.Message
Issue-Key: servicecatalog|DescribePortfolio|4xx->5xx|$.__type
Issue-Key: servicecatalog|DescribePortfolio|4xx->5xx|status
Issue-Key: servicecatalog|CreatePortfolio|4xx->5xx|$.__type
Issue-Key: servicecatalog|CreatePortfolio|4xx->5xx|$.message
Issue-Key: servicecatalog|CreatePortfolio|4xx->5xx|status
Issue-Key: servicecatalog|ListPrincipalsForPortfolio|4xx->5xx|$.Message
Issue-Key: servicecatalog|ListPrincipalsForPortfolio|4xx->5xx|$.__type
Issue-Key: servicecatalog|ListPrincipalsForPortfolio|4xx->5xx|status
Issue-Key: servicecatalog|CreateProduct|4xx->5xx|$.Message
Issue-Key: servicecatalog|CreateProduct|4xx->5xx|$.__type
Issue-Key: servicecatalog|CreateProduct|4xx->5xx|status
Issue-Key: servicecatalog|DescribeProduct|4xx->5xx|$.Message
Issue-Key: servicecatalog|DescribeProduct|4xx->5xx|$.__type
Issue-Key: servicecatalog|DescribeProduct|4xx->5xx|status
Issue-Key: servicecatalog|ListProvisioningArtifacts|4xx->5xx|$.Message
Issue-Key: servicecatalog|ListProvisioningArtifacts|4xx->5xx|$.__type
Issue-Key: servicecatalog|ListProvisioningArtifacts|4xx->5xx|status
Issue-Key: servicecatalog|DescribeServiceAction|4xx->5xx|$.Message
Issue-Key: servicecatalog|DescribeServiceAction|4xx->5xx|$.__type
Issue-Key: servicecatalog|DescribeServiceAction|4xx->5xx|status
Issue-Key: servicecatalog|CreateTagOption|4xx->5xx|$.Message
Issue-Key: servicecatalog|CreateTagOption|4xx->5xx|$.__type
Issue-Key: servicecatalog|CreateTagOption|4xx->5xx|status
Issue-Key: servicecatalog|DescribeTagOption|4xx->5xx|$.Message
Issue-Key: servicecatalog|DescribeTagOption|4xx->5xx|$.__type
Issue-Key: servicecatalog|DescribeTagOption|4xx->5xx|status
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# ServiceCatalog error-path operations return 500 AmbiguousService instead of modeled 400 errors

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConstraint

## Description
SHARED FAULT (10 causes below): every ServiceCatalog request is refused by the router before any handler runs, so each modeled client error becomes HTTP 500 with __type 'AmbiguousService' and no Message/message member. The refusal message reads (example) "2 emulated services all claim requests for 'servicecatalog': servicecatalog, servicecatalog; the request is rejected rather than served by an arbitrary one". Two emulated services declare the same sigv4 signing name: Smithy service_info gives sigv4Name 'servicecatalog' for AWS242ServiceCatalogService (2015-12-10, awsJson1_1, endpointPrefix 'servicecatalog') and ALSO for AWS242AppRegistry (2020-06-24, restJson1, endpointPrefix 'servicecatalog-appregistry'). The default can_handle seam claims any request whose resolved route key equals the signing name, so both claim the request and the router rejects it. This issue bundles the ten error-path (expected-400) clusters; the five success-path clusters are filed as the companion issue 'ServiceCatalog success-path operations return 500 AmbiguousService ...'. In every cause below, the required behavior is HTTP 400 with the modeled error code in __type; the recorded messages are quoted as EXAMPLES except where the rule is stated.

Cause 1 of 10: DescribeConstraint, unknown id. AWS: Smithy DescribeConstraint declares exactly one error, ResourceNotFoundException (errorKind client, httpError 400); the recording shows HTTP 400, __type 'ResourceNotFoundException', Message example 'Constraint cons-zzzzzzzzzzzzz not found.'. Emulator: HTTP 500, __type 'AmbiguousService', Message absent. Why: a client that catches ResourceNotFoundException to detect a missing constraint instead sees a retryable server fault and either retries a request that can never succeed or aborts. Fix: the service's request-claim seam (can_handle in services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/src/service.rs, and the matching seam in the AppRegistry crate) must disambiguate the shared signing name by protocol, host endpoint prefix, or X-Amz-Target service prefix; the operation's lookup must then raise ResourceNotFoundException for an unknown id. Traces: servicecatalog-constraint-lifecycle#task#12.

Cause 2 of 10: DescribePortfolio, unknown id. AWS: Smithy DescribePortfolio declares ResourceNotFoundException (client, 400); recording shows 400 / __type 'ResourceNotFoundException', Message example 'There are no local or default portfolios with id port-zzzzzzzzzzzzz'. Emulator: 500 / 'AmbiguousService', no Message. Why: missing-portfolio handling is indistinguishable from a server outage. Fix: claim seam as Cause 1, then the DescribePortfolio handler's state lookup. Traces: servicecatalog-portfolio-lifecycle#task#5.

Cause 3 of 10: CreatePortfolio, over-long DisplayName. AWS: Smithy CreatePortfolio.DisplayName targets PortfolioDisplayName with @length min 1 max 100; a value of 103 characters violates it. The recording shows HTTP 400 with __type 'ValidationException' and a constraint message built from the request (member 'displayName', 'Member must have length less than or equal to 100') — the requirement is the 400 plus ValidationException plus a message naming the offending member and its constraint; the exact recorded text is an example. ValidationException itself is not in the operation's modeled error list (modeled: InvalidParametersException, LimitExceededException, TagOptionNotMigratedException), so its code is recorded behavior only. Emulator: HTTP 500, __type 'AmbiguousService', message 'ServiceCatalog request rejected as ambiguous' (the router refusal text). Why: input validation is never performed, so a client cannot distinguish a bad request it must fix from a server-side failure it should retry. Fix: claim seam as Cause 1, then CreatePortfolio input validation (length constraint on DisplayName). Traces: servicecatalog-portfolio-lifecycle#task#6.

Cause 4 of 10: ListPrincipalsForPortfolio, unknown portfolio. AWS: Smithy ListPrincipalsForPortfolio declares InvalidParametersException and ResourceNotFoundException (both client, 400); recording shows 400 / 'ResourceNotFoundException', Message example 'Portfolio not found: port-zzzzzzzzzzzzz'. Emulator: 500 / 'AmbiguousService'. Why: a paginating client cannot tell an absent portfolio from a transport failure. Fix: claim seam as Cause 1, then the handler's portfolio lookup. Traces: servicecatalog-portfolio-principal-association#task#9.

Cause 5 of 10: CreateProduct, neither SourceConnection nor ProvisioningArtifactParameters.Info. AWS: Smithy CreateProduct declares InvalidParametersException (client, 400); ProvisioningArtifactParameters and SourceConnection are both optional members whose mutual requirement the model does not express, so the rule is recorded behavior only: HTTP 400, __type 'InvalidParametersException', Message example 'SourceConnection (for Repo products) or ProvisioningArtifactParameters.Info cannot both be null'. Emulator: 500 / 'AmbiguousService', no Message. Why: a client sending an incomplete product definition gets a 500 and may retry forever instead of correcting its request. Fix: claim seam as Cause 1, then CreateProduct's cross-member validation. Traces: servicecatalog-product-lifecycle#task#3 (cluster also covers servicecatalog-product-lifecycle#task#4).

Cause 6 of 10: DescribeProduct, unknown id. AWS: Smithy DescribeProduct declares InvalidParametersException and ResourceNotFoundException (both client, 400); recording shows 400 / 'ResourceNotFoundException', Message example 'prod-zzzzzzzzzzzzz does not exist or access was denied'. Emulator: 500 / 'AmbiguousService'. Why: not-found is reported as a server fault, breaking exists/absent probes. Fix: claim seam as Cause 1, then the DescribeProduct lookup. Traces: servicecatalog-product-lifecycle#task#8.

Cause 7 of 10: ListProvisioningArtifacts, unknown product. AWS: Smithy ListProvisioningArtifacts declares InvalidParametersException and ResourceNotFoundException (both client, 400); recording shows 400 / 'ResourceNotFoundException', Message example 'Product prod-zzzzzzzzzzzzz not found.'. Emulator: 500 / 'AmbiguousService'. Why: same not-found-as-outage confusion, on a list operation clients drive in loops. Fix: claim seam as Cause 1, then the handler's product lookup. Traces: servicecatalog-product-lifecycle#task#9 (cluster also covers servicecatalog-provisioning-artifact-lifecycle#task#9).

Cause 8 of 10: DescribeServiceAction, unknown id. AWS: Smithy DescribeServiceAction declares ResourceNotFoundException (client, 400); recording shows 400 / 'ResourceNotFoundException', Message example 'Service action act-zzzzzzzzzzzzz not found'. Emulator: 500 / 'AmbiguousService'. Why: a client cannot detect a deleted or never-created service action. Fix: claim seam as Cause 1, then the DescribeServiceAction lookup. Traces: servicecatalog-service-action-lifecycle#task#4.

Cause 9 of 10: CreateTagOption, duplicate key/value. AWS: Smithy CreateTagOption declares DuplicateResourceException, LimitExceededException and TagOptionNotMigratedException (all client, 400); re-creating an existing key/value pair is recorded as HTTP 400 with __type 'DuplicateResourceException', Message example 'TagOption with key <key> and value <value> already exists'. Emulator: 500 / 'AmbiguousService'. Why: an idempotent create-if-absent caller cannot recognise the already-exists case and treats it as a server error. Fix: claim seam as Cause 1, then CreateTagOption's uniqueness check in state. Traces: servicecatalog-tag-option-lifecycle#task#3.

Cause 10 of 10: DescribeTagOption, unknown id. AWS: Smithy DescribeTagOption declares ResourceNotFoundException and TagOptionNotMigratedException (both client, 400); recording shows 400 / 'ResourceNotFoundException', Message example 'TagOption with ID tag-zzzzzzzzzzzzz not found'. Emulator: 500 / 'AmbiguousService'. Why: not-found is unreportable, so TagOption cleanup/verification logic cannot proceed. Fix: claim seam as Cause 1, then the DescribeTagOption lookup. Traces: servicecatalog-tag-option-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200; bootstrap_signal false, state_reset_failures 0, 79 steps, 4 passed, 30 mismatched, 45 blocked, score 5.1%). Expected vs actual, per cause:
1) DescribeConstraint, servicecatalog-constraint-lifecycle#task#12: expected 400, got 500; $.__type expected 'ResourceNotFoundException', got 'AmbiguousService'; $.Message missing (expected 'Constraint cons-zzzzzzzzzzzzz not found.').
2) DescribePortfolio, servicecatalog-portfolio-lifecycle#task#5: expected 400, got 500; __type 'ResourceNotFoundException' -> 'AmbiguousService'; Message missing.
3) CreatePortfolio, servicecatalog-portfolio-lifecycle#task#6: expected 400, got 500; __type 'ValidationException' -> 'AmbiguousService'; $.message expected "1 validation error detected: Value '<103 chars>' at 'displayName' failed to satisfy constraint: Member must have length less than or equal to 100", got "2 emulated services all claim requests for 'servicecatalog': servicecatalog, servicecatalog; the request is rejected rather than served by an arbitrary one".
4) ListPrincipalsForPortfolio, servicecatalog-portfolio-principal-association#task#9: expected 400, got 500; __type -> 'AmbiguousService'; Message missing.
5) CreateProduct, servicecatalog-product-lifecycle#task#3: expected 400, got 500; __type 'InvalidParametersException' -> 'AmbiguousService'; Message missing (cluster also covers #task#4).
6) DescribeProduct, servicecatalog-product-lifecycle#task#8: expected 400, got 500; __type 'ResourceNotFoundException' -> 'AmbiguousService'.
7) ListProvisioningArtifacts, servicecatalog-product-lifecycle#task#9: expected 400, got 500; __type 'ResourceNotFoundException' -> 'AmbiguousService' (cluster also covers servicecatalog-provisioning-artifact-lifecycle#task#9).
8) DescribeServiceAction, servicecatalog-service-action-lifecycle#task#4: expected 400, got 500; __type 'ResourceNotFoundException' -> 'AmbiguousService'.
9) CreateTagOption, servicecatalog-tag-option-lifecycle#task#3: expected 400, got 500; __type 'DuplicateResourceException' -> 'AmbiguousService'.
10) DescribeTagOption, servicecatalog-tag-option-lifecycle#task#6: expected 400, got 500; __type 'ResourceNotFoundException' -> 'AmbiguousService'.
Smithy evidence for the collision: sigv4Name 'servicecatalog' on both AWS242ServiceCatalogService (endpointPrefix 'servicecatalog', awsJson1_1) and AWS242AppRegistry (endpointPrefix 'servicecatalog-appregistry', restJson1). The default claim seam (services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/src/service.rs, can_handle, lines 76-84) matches the resolved route key against the signing name only.

## Steps to Reproduce
1) Boot the emulator from this checkout.
2) Replay fixture servicecatalog-constraint-lifecycle and observe step task#12 (DescribeConstraint with id cons-zzzzzzzzzzzzz): HTTP 500 with __type 'AmbiguousService' instead of HTTP 400 ResourceNotFoundException.
3) Replay the other bundled steps, one per cause: servicecatalog-portfolio-lifecycle#task#5, servicecatalog-portfolio-lifecycle#task#6, servicecatalog-portfolio-principal-association#task#9, servicecatalog-product-lifecycle#task#3, servicecatalog-product-lifecycle#task#8, servicecatalog-product-lifecycle#task#9, servicecatalog-service-action-lifecycle#task#4, servicecatalog-tag-option-lifecycle#task#3, servicecatalog-tag-option-lifecycle#task#6 — each returns 500 AmbiguousService.
4) Equivalent by hand: POST / with credential scope signing name 'servicecatalog' and X-Amz-Target 'AWS242ServiceCatalogService.DescribeConstraint' and body {"Id":"cons-zzzzzzzzzzzzz"}; the router refuses with 500 AmbiguousService before the handler runs.
5) Narrow the claim seam so only one service claims an awsJson1_1 'AWS242ServiceCatalogService.*' request, then re-replay all ten steps above and confirm each returns 400 with its modeled error code.
