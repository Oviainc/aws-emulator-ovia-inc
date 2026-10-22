Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-catalog-batch-describe-entities-unknown-ids-v2
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-catalog-batch-describe-entities-unknown-ids-v2 marketplace-catalog-describe-entity-id-not-found marketplace-catalog-describe-entity-not-found-v2 marketplace-catalog-list-entities-filter-by-entity-id-success marketplace-catalog-list-entities-sorted-then-describe-first-v2 marketplace-catalog-list-product-entities-multiple-types-v2 marketplace-catalog-list-entities-validation-errors-v2
Issue-Shape-Id: com.amazonaws.marketplacecatalog#AWSMPSeymour
Issue-Key: marketplace-catalog|BatchDescribeEntities|2xx->5xx|$.EntityDetails
Issue-Key: marketplace-catalog|BatchDescribeEntities|2xx->5xx|$.Errors
Issue-Key: marketplace-catalog|BatchDescribeEntities|2xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|BatchDescribeEntities|2xx->5xx|status
Issue-Key: marketplace-catalog|DescribeEntity|4xx->5xx|$.Message
Issue-Key: marketplace-catalog|DescribeEntity|4xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|DescribeEntity|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: marketplace-catalog|DescribeEntity|4xx->5xx|status
Issue-Key: marketplace-catalog|ListEntities|2xx->5xx|$.EntitySummaryList
Issue-Key: marketplace-catalog|ListEntities|2xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|ListEntities|2xx->5xx|status
Issue-Key: marketplace-catalog|ListEntities|4xx->5xx|$.Message
Issue-Key: marketplace-catalog|ListEntities|4xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|ListEntities|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: marketplace-catalog|ListEntities|4xx->5xx|status
Issue-Commit: 668d747e62a7a896ef2863c29b158e984b097a40

# Marketplace Catalog entity read and list operations refused as AmbiguousService by the router

> _Note: this issue was identified against the code base at commit `668d747e62a7a896ef2863c29b158e984b097a40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDescribeEntities

## Description
This bundle has 5 causes, covering the entity read/list surface of AWS Marketplace Catalog. All five share one root cause in the shared request router, and every one must be closed before submitting.

SHARED ROOT CAUSE: the router resolves an incoming request to an emulated service by its sigv4 signing name. Smithy (services/service-com-amazonaws-marketplacecatalog-awsmpseymour/smithy-model/com-amazonaws-marketplacecatalog-awsmpseymour.json, query=service_info) gives Marketplace Catalog sigv4Name "aws-marketplace" and arnNamespace "aws-marketplace", while its endpointPrefix is "catalog.marketplace" and its protocol is restJson1. Every AWS Marketplace-family service shares that signing name but has a distinct endpointPrefix (Marketplace Reporting: endpointPrefix "reporting-marketplace"; Marketplace Entitlement Service: endpointPrefix "entitlement.marketplace"), so the router finds seven claimants for 'aws-marketplace', refuses to pick one, and answers HTTP 500 with x-amzn-errortype AmbiguousService and body {"__type":"AmbiguousService","message":"7 emulated services all claim requests for 'aws-marketplace': aws-marketplace, ...; the request is rejected rather than served by an arbitrary one"}. No AWS model defines an AmbiguousService error. Two further defects ride along: the refusal is serialized with content-type application/x-amz-json-1.1 (awsJson1_1) although restJson1 documents are application/json; and because the request never routes, no handler, no input validation and no state lookup runs, so the service is entirely unreachable (1 of 38 recorded steps passed, score 2.6%).

Why (common to all five): a client gets a 500 server-fault instead of the modeled 200/422/404, so SDKs treat it as retryable and retry requests that cannot succeed; entity discovery and entity reads return no payload and no modeled error at all, which breaks every Marketplace Catalog workload at its first call.

Fix (common to all five): the shared request router / service-registry claim resolution — disambiguate claimants by endpointPrefix and the request Host header/URI (POST /BatchDescribeEntities, GET /DescribeEntity, POST /ListEntities are distinctive restJson1 bindings) instead of by sigv4 signing name alone; and the refusal serializer must emit the target service's protocol (restJson1: application/json plus x-amzn-errortype). Once routing lands, the per-operation handlers, validation and serializers must produce the modeled responses below.

Cause 1 of 5: BatchDescribeEntities (POST /BatchDescribeEntities) where none of the requested entity ids exists. AWS: HTTP 200, content-type application/json, with BOTH members present — EntityDetails as an empty map and Errors as a map keyed by EntityId whose values carry ErrorCode "ResourceNotFoundException" and an ErrorMessage naming the missing entity (Smithy: BatchDescribeEntitiesResponse.EntityDetails map, .Errors map "with EntityId as the key and errorDetail as the value"; the operation's modeled errors are only AccessDenied/InternalService/Throttling/Validation, so a missing entity is NOT a top-level error). Recorded example per key: {"ErrorCode": "ResourceNotFoundException", "ErrorMessage": "Requested entity '<id>' does not exist."}. Emulator: HTTP 500 AmbiguousService, both EntityDetails and Errors missing, content-type application/x-amz-json-1.1. Why/Fix: as the shared cause — the batch handler must degrade per entity into the Errors map rather than failing the call. Traces: marketplace-catalog-batch-describe-entities-unknown-ids-v2 step task#1.

Cause 2 of 5: DescribeEntity (GET /DescribeEntity) with an entity id AWS rejects as malformed. AWS: HTTP 422 ValidationException (Smithy: ValidationException errorKind client, httpError 422; DescribeEntityRequest.EntityId required, httpQuery "entityId", @length 1..255), content-type application/json, with a Message built from the request naming the rejected id and the entity-id pattern the service enforces. Recorded behavior only for the exact pattern — the recording shows AWS enforcing a stricter server-side pattern than the model's ^[\w\-]+$; recorded example "[Requested entity id 'a' is invalid. It should match with ^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$.]". The requirement is the 422 status and the ValidationException code; the message text is an example. Emulator: HTTP 500 AmbiguousService, no validation performed. Traces: marketplace-catalog-describe-entity-id-not-found step task#1.

Cause 3 of 5: DescribeEntity for a well-formed entity id that does not exist. AWS: HTTP 404 ResourceNotFoundException (Smithy: DescribeEntity errors ResourceNotFoundException @httpError 404, plus ResourceNotSupportedException @httpError 415), application/json, Message naming the missing entity — recorded example "Requested entity 'prod-sim2realbehavdeidnf00000001' does not exist." Emulator: HTTP 500 AmbiguousService. Why/Fix: as the shared cause — the 422-before-lookup vs 404-after-lookup split must survive the fix. Traces: marketplace-catalog-describe-entity-id-not-found step task#2; the same cluster also covers marketplace-catalog-describe-entity-not-found-v2 step task#1.

Cause 4 of 5: ListEntities (POST /ListEntities) with nothing matching. AWS: HTTP 200, application/json, body carrying EntitySummaryList as an empty array (Smithy: ListEntitiesResponse.EntitySummaryList; NextToken null when there is no more result). This holds for id-filtered, sorted and multi-entity-type listings alike. Emulator: HTTP 500 AmbiguousService, EntitySummaryList missing. Why/Fix: as the shared cause plus empty-collection serialization in the list handler. Traces: marketplace-catalog-list-entities-filter-by-entity-id-success step task#1; this one cluster spans 7 recorded steps, so the fixer must also replay marketplace-catalog-list-entities-sorted-then-describe-first-v2 step task#1 and marketplace-catalog-list-product-entities-multiple-types-v2 steps task#1 through task#5.

Cause 5 of 5: ListEntities with an unsupported sort field. AWS: HTTP 422 ValidationException (Smithy: ListEntities errors include ValidationException @httpError 422; ListEntitiesRequest.Sort / EntityTypeSort), application/json, with a Message built from the request naming the rejected sort field and the permitted set — recorded example "[Requested sort by 'NotARealSortField' is invalid. It should be one of '[]'.]" The requirement is the 422 status and the ValidationException code; the message text is an example. Emulator: HTTP 500 AmbiguousService, no validation performed. Traces: marketplace-catalog-list-entities-sorted-then-describe-first-v2 step task#2; this cluster also covers marketplace-catalog-list-entities-validation-errors-v2 steps task#1 and task#2, which must be replayed too.

Related open issues (they do NOT cover these steps): '20261021T234042Z-osis-router-refuses-every-sigv4-iam-request-as-ambiguousservice' and '20261021T234924Z-featurestore-runtime-sagemaker-router-refuses-every-featurestore-runtime-sagemaker-request-as' report the same router defect for the 'iam' and 'sagemaker' signing-name collisions and carry no replay key for any marketplace-catalog fixture.

## Evidence
Replay of simparity-wire-aws-v1: emulator_up=true, bootstrap_signal=false, state_reset_failures=0, 18 fixtures replayed, 38 steps, 1 passed, 37 mismatched, score 2.6%. All five clusters in this bundle report actual_status 500 with the AmbiguousService body.

Per-cause expected vs actual:
- BatchDescribeEntities: status expected 200, got 500; $.EntityDetails expected {}, got None; $.Errors expected {"sim2real-probe-marketplace-catalog-batchentity-263f1e09b": {"ErrorCode": "ResourceNotFoundException", "ErrorMessage": "Requested entity 'sim2real-probe-marketplace-catalog-batchentity-263f1e09b' does not exist."}, "sim2real-probe-marketplace-catalog-batchentity-263f1e09a": {"ErrorCode": "ResourceNotFoundException", "ErrorMessage": "Requested entity 'sim2real-probe-marketplace-catalog-batchentity-263f1e09a' does not exist."}}, got None; $headers.content-type expected 'application/json', got 'application/x-amz-json-1.1'.
- DescribeEntity (malformed id): status expected 422, got 500; $.Message expected "[Requested entity id 'a' is invalid. It should match with ^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$.]", got None; $headers.x-amzn-errortype got 'AmbiguousService'.
- DescribeEntity (not found): status expected 404, got 500; $.Message expected "Requested entity 'prod-sim2realbehavdeidnf00000001' does not exist.", got None.
- ListEntities (empty result): status expected 200, got 500; $.EntitySummaryList expected [], got None. Cluster affects 7 steps: marketplace-catalog-list-entities-filter-by-entity-id-success#task#1, marketplace-catalog-list-entities-sorted-then-describe-first-v2#task#1, marketplace-catalog-list-product-entities-multiple-types-v2#task#1..#task#5.
- ListEntities (bad sort): status expected 422, got 500; $.Message expected "[Requested sort by 'NotARealSortField' is invalid. It should be one of '[]'.]", got None. Cluster affects marketplace-catalog-list-entities-sorted-then-describe-first-v2#task#2, marketplace-catalog-list-entities-validation-errors-v2#task#1 and #task#2.

Every actual body is {"__type": "AmbiguousService", "message": "7 emulated services all claim requests for 'aws-marketplace': aws-marketplace, aws-marketplace, aws-marketplace, aws-marketplace, aws-marketplace, aws-marketplace, aws-marketplace; the request is rejected rather than served by an arbitrary one"}.

Smithy evidence: service_info — protocol restJson1, endpointPrefix "catalog.marketplace", sigv4Name "aws-marketplace". BatchDescribeEntities: POST /BatchDescribeEntities, output BatchDescribeEntitiesResponse{EntityDetails map, Errors map}, modeled errors AccessDenied(403)/InternalService(500)/Throttling(429)/Validation(422) only. DescribeEntity: GET /DescribeEntity, EntityId required @length 1..255 @pattern ^[\w\-]+$, errors ResourceNotFound(404), ResourceNotSupported(415), Validation(422). ListEntities: POST /ListEntities, output EntitySummaryList, errors ResourceNotFound(404), Validation(422). Sibling collision: Marketplace Reporting and Marketplace Entitlement Service both have sigv4Name "aws-marketplace" with endpointPrefixes "reporting-marketplace" and "entitlement.marketplace". No AWS model defines 'AmbiguousService'.

## Steps to Reproduce
1. Clone the repo and boot the emulator (/__admin/health answers 200 and the data listener accepts).
2. Point an SDK client for Marketplace Catalog (sigv4 signing name 'aws-marketplace', endpointPrefix 'catalog.marketplace', protocol restJson1) at the emulator's data listener.
3. Call BatchDescribeEntities with two EntityRequest entries whose entity ids do not exist. Observe HTTP 500, content-type application/x-amz-json-1.1, x-amzn-errortype AmbiguousService, body {"__type":"AmbiguousService",...} instead of HTTP 200 application/json with EntityDetails {} and an Errors map keyed by each EntityId carrying ErrorCode ResourceNotFoundException.
4. Call DescribeEntity(Catalog="AWSMarketplace", EntityId="a") and expect HTTP 422 ValidationException naming the rejected entity id; call DescribeEntity with a well-formed but unknown prod-... id and expect HTTP 404 ResourceNotFoundException. Both answer the same 500 AmbiguousService refusal.
5. Call ListEntities(Catalog="AWSMarketplace", EntityType="AmiProduct") with a filter matching nothing and expect HTTP 200 with EntitySummaryList []; call ListEntities with Sort.SortBy="NotARealSortField" and expect HTTP 422 ValidationException. Both answer the same 500 refusal, confirming the service is unreachable rather than mis-implemented per operation.
6. Replay the wire corpus simparity-wire-aws-v1 and re-run marketplace-catalog-batch-describe-entities-unknown-ids-v2, marketplace-catalog-describe-entity-id-not-found, marketplace-catalog-describe-entity-not-found-v2, marketplace-catalog-list-entities-filter-by-entity-id-success, marketplace-catalog-list-entities-sorted-then-describe-first-v2, marketplace-catalog-list-product-entities-multiple-types-v2 and marketplace-catalog-list-entities-validation-errors-v2 to confirm the fix.
