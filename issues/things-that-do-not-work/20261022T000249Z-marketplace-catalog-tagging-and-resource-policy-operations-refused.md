Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-catalog-resource-policy-error-lifecycle-v2
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-catalog-resource-policy-error-lifecycle-v2 marketplace-catalog-list-tags-for-resource-arn-validation-boundary marketplace-catalog-tag-resource-error-lifecycle-v2
Issue-Shape-Id: com.amazonaws.marketplacecatalog#AWSMPSeymour
Issue-Key: marketplace-catalog|GetResourcePolicy|2xx->5xx|$.Policy
Issue-Key: marketplace-catalog|GetResourcePolicy|2xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|GetResourcePolicy|2xx->5xx|status
Issue-Key: marketplace-catalog|PutResourcePolicy|4xx->5xx|$.Message
Issue-Key: marketplace-catalog|PutResourcePolicy|4xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|PutResourcePolicy|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: marketplace-catalog|PutResourcePolicy|4xx->5xx|status
Issue-Key: marketplace-catalog|DeleteResourcePolicy|2xx->5xx|$
Issue-Key: marketplace-catalog|DeleteResourcePolicy|2xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|DeleteResourcePolicy|2xx->5xx|status
Issue-Key: marketplace-catalog|ListTagsForResource|4xx->5xx|$.Message
Issue-Key: marketplace-catalog|ListTagsForResource|4xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|ListTagsForResource|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: marketplace-catalog|ListTagsForResource|4xx->5xx|status
Issue-Key: marketplace-catalog|TagResource|4xx->5xx|$.Message
Issue-Key: marketplace-catalog|TagResource|4xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|TagResource|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: marketplace-catalog|TagResource|4xx->5xx|status
Issue-Key: marketplace-catalog|UntagResource|4xx->5xx|$.Message
Issue-Key: marketplace-catalog|UntagResource|4xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|UntagResource|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: marketplace-catalog|UntagResource|4xx->5xx|status
Issue-Commit: 668d747e62a7a896ef2863c29b158e984b097a40

# Marketplace Catalog tagging and resource-policy operations refused as AmbiguousService by the router

> _Note: this issue was identified against the code base at commit `668d747e62a7a896ef2863c29b158e984b097a40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
This bundle has 8 causes, covering the resource-policy and tagging surface of AWS Marketplace Catalog. All eight share one root cause in the shared request router, and every one must be closed before submitting.

SHARED ROOT CAUSE: the router resolves an incoming request to an emulated service by its sigv4 signing name. Smithy (services/service-com-amazonaws-marketplacecatalog-awsmpseymour/smithy-model/com-amazonaws-marketplacecatalog-awsmpseymour.json, query=service_info) gives Marketplace Catalog sigv4Name "aws-marketplace" and arnNamespace "aws-marketplace", while its endpointPrefix is "catalog.marketplace" and its protocol is restJson1. Every AWS Marketplace-family service shares that signing name but has a distinct endpointPrefix (Marketplace Reporting: endpointPrefix "reporting-marketplace"; Marketplace Entitlement Service: endpointPrefix "entitlement.marketplace"), so the router finds seven claimants for 'aws-marketplace', refuses to pick one, and answers HTTP 500 with x-amzn-errortype AmbiguousService and body {"__type":"AmbiguousService","message":"7 emulated services all claim requests for 'aws-marketplace': aws-marketplace, ...; the request is rejected rather than served by an arbitrary one"}. No AWS model defines an AmbiguousService error. Two further defects ride along: the refusal is serialized with content-type application/x-amz-json-1.1 (awsJson1_1) although restJson1 documents are application/json; and because the request never routes, no handler, no input validation, no authorization check and no state lookup runs, so the service is entirely unreachable (1 of 38 recorded steps passed, score 2.6%).

Why (common to all eight): a client gets a 500 server-fault instead of the modeled 200/403/404/422, so SDKs classify it as retryable and retry requests that cannot succeed; policy reads/writes and tag reads/writes return no payload and no modeled error, and an authorization denial is indistinguishable from a server fault.

Fix (common to all eight): the shared request router / service-registry claim resolution — disambiguate claimants by endpointPrefix and the request Host header/URI (GET /GetResourcePolicy, POST /PutResourcePolicy, DELETE /DeleteResourcePolicy, POST /ListTagsForResource, POST /TagResource, POST /UntagResource are distinctive restJson1 bindings) instead of by sigv4 signing name alone; and the refusal serializer must emit the target service's protocol (restJson1: application/json plus x-amzn-errortype). Once routing lands, each operation's handler, its validation, its authorization check and its serializer must produce the modeled responses below.

Cause 1 of 8: GetResourcePolicy (GET /GetResourcePolicy) for an entity with a policy attached. AWS: HTTP 200, content-type application/json, body carrying Policy as the JSON policy document (Smithy: GetResourcePolicyResponse.Policy -> ResourcePolicyJson, @length 1..10240) — the recorded value '{}' is an example of the document, the requirement is that the member is returned. Emulator: HTTP 500 AmbiguousService, Policy missing, content-type application/x-amz-json-1.1. Traces: marketplace-catalog-resource-policy-error-lifecycle-v2 step task#1; the same cluster also covers step task#4 of that fixture (the read-back after a mutation), which must be replayed too.

Cause 2 of 8: PutResourcePolicy (POST /PutResourcePolicy) against an entity that does not exist. AWS: HTTP 404 ResourceNotFoundException (Smithy: PutResourcePolicy errors ResourceNotFoundException, errorKind client, httpError 404), application/json, Message naming the missing entity — recorded example "Requested entity 'prod-b2c3d4e5f6g7h' does not exist." Emulator: HTTP 500 AmbiguousService. Traces: marketplace-catalog-resource-policy-error-lifecycle-v2 step task#2.

Cause 3 of 8: DeleteResourcePolicy (DELETE /DeleteResourcePolicy) for an existing entity. AWS: HTTP 200, application/json, with an EMPTY JSON object body (Smithy: DeleteResourcePolicyResponse has no members). Emulator: HTTP 500 and, instead of {}, the AmbiguousService error document {"__type": "AmbiguousService", "message": "..."} with content-type application/x-amz-json-1.1. Traces: marketplace-catalog-resource-policy-error-lifecycle-v2 step task#3.

Cause 4 of 8: ListTagsForResource (POST /ListTagsForResource) with a malformed resource ARN. AWS: HTTP 422 ValidationException (Smithy: ValidationException errorKind client, httpError 422; ListTagsForResourceRequest.ResourceArn required, target ResourceARN, @length 1..255, @pattern ^arn:[\w+=/,.@-]+:aws-marketplace:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$), application/json. The message RULE is known — it is built from the request and names the rejected ARN and that modeled pattern; recorded example "[Requested resource arn 'arn:aws:aws-marketplace' is invalid. It should match with ^arn:[\\w+=/,.@-]+:aws-marketplace:[\\w+=/,.@-]*:[0-9]+:[\\w+=,.@-]+(/[\\w+=,.@-]+)*$.]" Emulator: HTTP 500 AmbiguousService, no validation performed. Traces: marketplace-catalog-list-tags-for-resource-arn-validation-boundary step task#2.

Cause 5 of 8: ListTagsForResource for a well-formed resource ARN the caller is not authorized to read. AWS: HTTP 403 AccessDeniedException (Smithy: AccessDeniedException errorKind client, httpError 403, doc "Access is denied. HTTP status code: 403"; it is a service-common error bound to ListTagsForResource), application/json, x-amzn-errortype naming that error. The recorded Message is IAM/account-specific ("User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: aws-marketplace:ListTagsForResource on resource: ... because no resource-based policy allows the aws-marketplace:ListTagsForResource action") and is an example only — the requirement is the 403 status and the AccessDeniedException code. Emulator: HTTP 500, x-amzn-errortype AmbiguousService. Traces: marketplace-catalog-list-tags-for-resource-arn-validation-boundary step task#3.

Cause 6 of 8: TagResource (POST /TagResource) against an entity that does not exist. AWS: HTTP 404 ResourceNotFoundException (Smithy: TagResource errors ResourceNotFoundException @httpError 404), application/json, x-amzn-errortype recorded in the SDK's Code:namespace form 'ResourceNotFoundException:com.amazonaws.services.marketplacecatalog.model', Message naming the missing entity — recorded example "Requested entity 'prod-c3d4e5f6g7h8i' does not exist." Emulator: HTTP 500 AmbiguousService. Traces: marketplace-catalog-tag-resource-error-lifecycle-v2 step task#1.

Cause 7 of 8: ListTagsForResource against an entity that does not exist (distinct from cause 4's malformed ARN and from cause 5's denial). AWS: HTTP 404 ResourceNotFoundException (Smithy: ListTagsForResource errors ResourceNotFoundException @httpError 404), application/json, Message naming the missing entity — recorded example "Requested entity 'prod-c3d4e5f6g7h8i' does not exist." Emulator: HTTP 500 AmbiguousService. Why/Fix: as the shared cause — the 422 (malformed ARN) / 403 (denied) / 404 (absent) three-way split must all survive the fix. Traces: marketplace-catalog-tag-resource-error-lifecycle-v2 step task#2.

Cause 8 of 8: UntagResource (POST /UntagResource) against an entity that does not exist. AWS: HTTP 404 ResourceNotFoundException (Smithy: UntagResource errors ResourceNotFoundException @httpError 404), application/json, x-amzn-errortype 'ResourceNotFoundException:com.amazonaws.services.marketplacecatalog.model', Message naming the missing entity — recorded example "Requested entity 'prod-c3d4e5f6g7h8i' does not exist." Emulator: HTTP 500 AmbiguousService. Traces: marketplace-catalog-tag-resource-error-lifecycle-v2 step task#3.

Related open issues (they do NOT cover these steps): '20261021T234042Z-osis-router-refuses-every-sigv4-iam-request-as-ambiguousservice' and '20261021T234924Z-featurestore-runtime-sagemaker-router-refuses-every-featurestore-runtime-sagemaker-request-as' report the same router defect for the 'iam' and 'sagemaker' signing-name collisions and carry no replay key for any marketplace-catalog fixture, so fixing those alone will not close these eight causes.

## Evidence
Replay of simparity-wire-aws-v1: emulator_up=true, bootstrap_signal=false, state_reset_failures=0, 18 fixtures replayed, 38 steps, 1 passed, 37 mismatched, score 2.6%. All eight clusters in this bundle report actual_status 500 with the AmbiguousService body.

Per-cause expected vs actual:
- GetResourcePolicy: status expected 200, got 500; $.Policy expected '{}', got None; $headers.content-type expected 'application/json', got 'application/x-amz-json-1.1'. Cluster affects marketplace-catalog-resource-policy-error-lifecycle-v2#task#1 and #task#4.
- PutResourcePolicy: status expected 404, got 500; $.Message expected "Requested entity 'prod-b2c3d4e5f6g7h' does not exist.", got None; $headers.x-amzn-errortype got 'AmbiguousService'.
- DeleteResourcePolicy: status expected 200, got 500; $ expected {}, got {"__type": "AmbiguousService", "message": "7 emulated services all claim requests for 'aws-marketplace': aws-marketplace, aws-marketplace, aws-marketplace, aws-marketplace, aws-marketplace, aws-marketplace, aws-marketplace; the request is rejected rather than served by an arbitrary one"}; content-type as above.
- ListTagsForResource (malformed ARN): status expected 422, got 500; $.Message expected "[Requested resource arn 'arn:aws:aws-marketplace' is invalid. It should match with ^arn:[\\w+=/,.@-]+:aws-marketplace:[\\w+=/,.@-]*:[0-9]+:[\\w+=,.@-]+(/[\\w+=,.@-]+)*$.]", got None.
- ListTagsForResource (denied): status expected 403, got 500; $.Message expected 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: aws-marketplace:ListTagsForResource on resource: sim2real-behav-1b187cbd960a because no resource-based policy allows the aws-marketplace:ListTagsForResource action', got None.
- TagResource (unknown entity): status expected 404, got 500; $.Message expected "Requested entity 'prod-c3d4e5f6g7h8i' does not exist.", got None; $headers.x-amzn-errortype expected 'ResourceNotFoundException:com.amazonaws.services.marketplacecatalog.model', got 'AmbiguousService'.
- ListTagsForResource (unknown entity): status expected 404, got 500; same Message/errortype diffs.
- UntagResource (unknown entity): status expected 404, got 500; same Message/errortype diffs.

Smithy evidence: service_info — sdkId "Marketplace Catalog", protocol restJson1, endpointPrefix "catalog.marketplace", sigv4Name "aws-marketplace", arnNamespace "aws-marketplace". GetResourcePolicyResponse.Policy -> ResourcePolicyJson @length 1..10240; DeleteResourcePolicyResponse has no members; ListTagsForResourceRequest.ResourceArn required @length 1..255 @pattern ^arn:[\w+=/,.@-]+:aws-marketplace:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$; errors ResourceNotFoundException httpError 404, ValidationException httpError 422, AccessDeniedException httpError 403 (service-common, bound to every operation). Sibling collision confirmed: Marketplace Reporting (sigv4Name aws-marketplace, endpointPrefix reporting-marketplace) and Marketplace Entitlement Service (sigv4Name aws-marketplace, endpointPrefix entitlement.marketplace). No AWS model defines 'AmbiguousService'.

Failing fixtures/steps for this bundle: marketplace-catalog-resource-policy-error-lifecycle-v2#task#1, #task#2, #task#3, #task#4; marketplace-catalog-list-tags-for-resource-arn-validation-boundary#task#2 and #task#3; marketplace-catalog-tag-resource-error-lifecycle-v2#task#1, #task#2 and #task#3.

## Steps to Reproduce
1. Clone the repo and boot the emulator (/__admin/health answers 200 and the data listener accepts).
2. Point an SDK client for Marketplace Catalog (sigv4 signing name 'aws-marketplace', endpointPrefix 'catalog.marketplace', protocol restJson1) at the emulator's data listener.
3. Call GetResourcePolicy(ResourceArn=<an existing entity ARN>) and expect HTTP 200 application/json with a Policy member. Observe HTTP 500, content-type application/x-amz-json-1.1, x-amzn-errortype AmbiguousService and body {"__type":"AmbiguousService","message":"7 emulated services all claim requests for 'aws-marketplace': ..."}.
4. Call PutResourcePolicy against an entity ARN that does not exist and expect HTTP 404 ResourceNotFoundException; call DeleteResourcePolicy for an existing entity and expect HTTP 200 with an empty JSON object body. Both answer the same 500 refusal.
5. Call ListTagsForResource(ResourceArn="arn:aws:aws-marketplace") and expect HTTP 422 ValidationException naming the ResourceArn pattern; call it for a well-formed ARN the caller may not read and expect HTTP 403 AccessDeniedException; call it, TagResource and UntagResource for a well-formed entity ARN that does not exist and expect HTTP 404 ResourceNotFoundException in each case. All answer the identical 500 AmbiguousService refusal, confirming the service is unreachable rather than mis-implemented per operation.
6. Replay the wire corpus simparity-wire-aws-v1 and re-run marketplace-catalog-resource-policy-error-lifecycle-v2 (all four steps), marketplace-catalog-list-tags-for-resource-arn-validation-boundary and marketplace-catalog-tag-resource-error-lifecycle-v2 to confirm the fix.
