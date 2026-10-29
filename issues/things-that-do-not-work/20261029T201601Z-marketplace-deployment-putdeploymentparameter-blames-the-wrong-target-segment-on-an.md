Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-deployment.put-deployment-parameter-tags-exceed-max-rejected
Issue-Shape-Id: com.amazonaws.marketplacedeployment#AWSMPDeploymentParametersService
Issue-Key: marketplace-deployment.put-deployment-parameter-tags-exceed-max-rejected|run#5|AssertionError|assert control_message == (
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# PutDeploymentParameter blames the wrong target segment on an unresolved write target, and refuses an over-50-entry tags map with the wrong error identity

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeploymentParameter

## Description
Two net-new root causes in `PutDeploymentParameter`'s refusal paths, both exercised by the single failing check `marketplace-deployment.put-deployment-parameter-tags-exceed-max-rejected`. Neither is covered by the four open marketplace-deployment issues (the closest, `20261029T061857Z-marketplace-deployment-putdeploymentparameter-wrong-behavior--r2.md`, is about the response `tags` member carrying entries `TagsMap` cannot represent, and says nothing about either refusal below).

Cause 1 of 2: an unresolved write target is blamed on `ProductId` where AWS blames `AgreementId`.

AWS: `PutDeploymentParameter` models `ResourceNotFoundException` with `httpError 404` (Smithy: PutDeploymentParameter errors -> ResourceNotFoundException, errorKind client, httpError 404; doc "The specified resource wasn't found."), and the operation's own doc states the targeting order: "Creates or updates a deployment parameter and is targeted by catalog and agreementId" — the agreement is the target, the product is the URI's addressed resource. Recorded behavior (the check's positive control) shows that for a request whose `productId` AND `agreementId` both name nothing the account holds, AWS answers 404 `ResourceNotFoundException` with the message `"AgreementId: Request references a resource which does not exist."`. Message template: the fixed text is `"<Member>: Request references a resource which does not exist."`; the only variable part is `<Member>`, the PascalCase name of the target segment that did not resolve — and when the target as a whole does not resolve, AWS names `AgreementId`, not the product. No value from the request (no id, no ARN) appears in the message.

Emulator: answers the same 404/`ResourceNotFoundException` but with `"ProductId: Request references a resource which does not exist."`. `state.rs` `unresolved_target` deliberately judges the PRODUCT first ("The product is judged FIRST and the agreement second, so a request naming neither is blamed on the product") and only returns `"AgreementId"` when some agreement of the catalog already covers the product; `target_not_found` then formats the sentence from that member. So for the common case — a request naming neither a held product nor a held agreement — the emulator blames the product and AWS blames the agreement, and the precedence is inverted.

Why: a client that reads the member name out of the not-found message (or a test/automation that branches on it) is told its `productId` is wrong when AWS says its `agreementId` is wrong. It sends the caller to fix the wrong request member, and any assertion on AWS's wording fails even though the status and error code match.

Fix: the target-resolution precedence in the service's state layer — the function that picks which segment of `(catalog, productId, agreementId)` to blame — and, through it, `PutDeploymentParameter`'s handler target check. The agreement must be the segment named when the target does not resolve; blaming the product can only be correct where AWS does so.

Traces: marketplace-deployment.put-deployment-parameter-tags-exceed-max-rejected (the check's positive control call, which asserts `AgreementId` and received `ProductId`).

Cause 2 of 2: a `tags` map past 50 entries is refused with the modelled `ValidationException` instead of the front-end's `BadRequestException`.

AWS: `PutDeploymentParameterRequest$tags` targets `TagsMap` with `@length max 50` (Smithy: PutDeploymentParameter input member `tags` -> TagsMap, length max 50). Recorded behavior only: the request-validation layer in front of the handler enforces that maxProperties bound and returns it as an UNMODELLED `BadRequestException` at HTTP 400 with the message `"The resultant tag set must not have more than 50 user tags."` — not the handler's own `ValidationException`. Message template: that sentence is entirely fixed text; `50` is the documented per-resource tag maximum the member restates, not a value echoed from the request, and the request's tag count does not appear.

Emulator: this assertion is reached only after Cause 1 is fixed (the check aborts on the control above), so it was not observed on the wire in this replay; it is read off the code, which has no path that can produce it. A 51-entry `tags` map violates the member's modelled `@length max 50` and is refused by the centralized constraint walk that runs ahead of `PutDeploymentParameter::handle`, i.e. error code `ValidationException` at 400; if it ever reached the handler, `state.rs` `validate_tags_member` returns the modelled `ValidationException` with the message "the request carries 51 tags; a deployment parameter holds at most 50" (and `merge_tags` the analogous "the request would leave the resource with N tags; …"). The service DOES own a `BadRequestException` identity (`scaffolded.rs` `BAD_REQUEST_CODE`/`BAD_REQUEST_STATUS`), but it is wired only to the tagging front-end's resource-ARN parse, never to the tags-count bound. Hypothesis: the over-count bound was implemented as one of this service's own modelled input rules rather than as the front-door request-validation rule the recording shows it to be.

Why: the code is client-visible on restJson1 — the error SHAPE is resolved from `x-amzn-errortype`/`__type`, so an SDK unmarshals the modelled `ValidationException` into a typed error and `BadRequestException` into a generic service error. A caller's `catch ValidationException` fires where AWS would not, and a caller matching `BadRequestException` sees nothing; the message also differs from AWS's fixed sentence.

Fix: the request-validation layer in front of `PutDeploymentParameter` (where the `tags` maxProperties bound is enforced) and the error identity it answers with, plus the tags-count branch of the service's tag validation so the two cannot disagree. The emulator's existing `BadRequestException` channel is the identity to emit; it must not be the modelled `ValidationException`.

Traces: marketplace-deployment.put-deployment-parameter-tags-exceed-max-rejected (the check's second call, 51 well-formed key/value pairs, asserting code `BadRequestException` and that exact sentence).

## Evidence
Failing check: `marketplace-deployment.put-deployment-parameter-tags-exceed-max-rejected` [mismatch]

Replay output:
```
File "check_marketplace_deployment_put_deployment_parameter_tags_exceed_max_rejected.py", line 44, in run
    assert control_message == (
AssertionError: ProductId: Request references a resource which does not exist.
```

Cause 1 — expected vs actual on the positive-control call (catalog=AWSMarketplace, fabricated productId and agreementId, well-formed one-entry tags map):
- expected: HTTP 404, code `ResourceNotFoundException`, message `AgreementId: Request references a resource which does not exist.`
- actual:   HTTP 404, code `ResourceNotFoundException`, message `ProductId: Request references a resource which does not exist.`
Status and code agree; the blamed member diverges. Code producing it: `src/state.rs` `unresolved_target` ("The product is judged FIRST and the agreement second, so a request naming neither is blamed on the product", returning `"AgreementId"` only when `product_covered`) and `target_not_found`, which formats `"{member}: Request references a resource which does not exist."`; called from `src/ops/put_deployment_parameter.rs:299-306`.

Cause 2 — expected vs actual on the 51-tag call (same fabricated target, `tags` = 51 well-formed pairs):
- expected: HTTP 400, code `BadRequestException`, message `The resultant tag set must not have more than 50 user tags.`
- actual:   HTTP 400, code `ValidationException` — from the centralized constraint walk for `tags` `@length max 50` (the member is modelled `TagsMap`, length max 50), or from `src/state.rs` `validate_tags_member`'s over-count branch, whose message is "the request carries {n} tags; a deployment parameter holds at most 50" (and `merge_tags`'s "the request would leave the resource with {n} tags; a deployment parameter holds at most 50"). Not observed on the wire this replay: the check aborts on Cause 1 first. `src/scaffolded.rs` shows the correct identity exists but is bound only to the resource-ARN parse: `BAD_REQUEST_CODE = "BadRequestException"`, `BAD_REQUEST_STATUS = 400`, documented as "the tagging front-end's own ARN parse".

## Steps to Reproduce
1. Point a botocore `marketplace-deployment` client at the emulator, region us-east-1 (the service serves only us-east-1).
2. Cause 1: call `PutDeploymentParameter` with catalog="AWSMarketplace", a productId that no held agreement covers (e.g. `prod-<random>`), an agreementId the account does not hold (e.g. `agr-<random>`), deploymentParameter={name:"<random>", secretString:"s3cr3t-value-1"} and a well-formed one-entry tags map such as {"one-tag":"under-the-limit"}.
   Observe HTTP 404 `ResourceNotFoundException` with message "ProductId: Request references a resource which does not exist."; AWS answers the same status and code with "AgreementId: Request references a resource which does not exist." Neither the product nor the agreement resolves, so the blamed segment must be the agreement.
3. Cause 2: repeat the same call with `tags` set to 51 well-formed pairs ({f"key{i}": f"value{i}" for i in range(51)}).
   Observe a 400 whose error code is `ValidationException`; AWS answers HTTP 400 with the code `BadRequestException` and the message "The resultant tag set must not have more than 50 user tags." (Reachable over the wire only once step 2 is fixed, since the check asserts the control first.)
4. Re-replay `marketplace-deployment.put-deployment-parameter-tags-exceed-max-rejected`; both assertions must pass in one run — the check proves the excess-tags request is rejected for its tag count and not merely for the unresolved target, so the two refusals must carry the distinct identities AWS gives them.
