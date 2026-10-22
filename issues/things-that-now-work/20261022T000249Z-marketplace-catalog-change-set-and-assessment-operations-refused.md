Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-catalog-cancel-changeset-not-found-no-side-effect-v1
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-catalog-cancel-changeset-not-found-no-side-effect-v1 marketplace-catalog-start-and-cancel-changeset-unknown-entity-v2 marketplace-catalog-describe-changeset-not-found-v2 marketplace-catalog-list-changesets-and-assessments-v2 marketplace-catalog-start-changeset-duplicate-change-type-rejected marketplace-catalog-start-changeset-validation-error-v2 marketplace-catalog-describe-assessment-identifier-validation-boundary marketplace-catalog-describe-assessment-not-found-v2
Issue-Shape-Id: com.amazonaws.marketplacecatalog#AWSMPSeymour
Issue-Key: marketplace-catalog|CancelChangeSet|4xx->5xx|$.Message
Issue-Key: marketplace-catalog|CancelChangeSet|4xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|CancelChangeSet|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: marketplace-catalog|CancelChangeSet|4xx->5xx|status
Issue-Key: marketplace-catalog|DescribeChangeSet|4xx->5xx|$.Message
Issue-Key: marketplace-catalog|DescribeChangeSet|4xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|DescribeChangeSet|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: marketplace-catalog|DescribeChangeSet|4xx->5xx|status
Issue-Key: marketplace-catalog|ListChangeSets|2xx->5xx|$.ChangeSetSummaryList
Issue-Key: marketplace-catalog|ListChangeSets|2xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|ListChangeSets|2xx->5xx|status
Issue-Key: marketplace-catalog|StartChangeSet|4xx->5xx|$.Message
Issue-Key: marketplace-catalog|StartChangeSet|4xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|StartChangeSet|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: marketplace-catalog|StartChangeSet|4xx->5xx|status
Issue-Key: marketplace-catalog|DescribeAssessment|4xx->5xx|$.Message
Issue-Key: marketplace-catalog|DescribeAssessment|4xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|DescribeAssessment|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: marketplace-catalog|DescribeAssessment|4xx->5xx|status
Issue-Key: marketplace-catalog|ListAssessments|2xx->5xx|$.AssessmentSummaryList
Issue-Key: marketplace-catalog|ListAssessments|2xx->5xx|$headers.content-type
Issue-Key: marketplace-catalog|ListAssessments|2xx->5xx|status
Issue-Commit: 668d747e62a7a896ef2863c29b158e984b097a40

# Marketplace Catalog change-set and assessment operations refused as AmbiguousService by the router

> _Note: this issue was identified against the code base at commit `668d747e62a7a896ef2863c29b158e984b097a40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelChangeSet

## Description
This bundle has 8 causes, covering the change-set and assessment operations of AWS Marketplace Catalog. All eight share one root cause in the shared request router, and every one must be closed before submitting.

SHARED ROOT CAUSE: the router resolves an incoming request to an emulated service by its sigv4 signing name. Smithy (services/service-com-amazonaws-marketplacecatalog-awsmpseymour/smithy-model/com-amazonaws-marketplacecatalog-awsmpseymour.json, query=service_info) gives Marketplace Catalog sigv4Name "aws-marketplace" and arnNamespace "aws-marketplace", while its endpointPrefix is "catalog.marketplace" and its protocol is restJson1. Every AWS Marketplace-family service shares that signing name but has a distinct endpointPrefix (Marketplace Reporting: sigv4Name "aws-marketplace", endpointPrefix "reporting-marketplace"; Marketplace Entitlement Service: sigv4Name "aws-marketplace", endpointPrefix "entitlement.marketplace"), so the router finds seven claimants for 'aws-marketplace', refuses to pick one, and answers HTTP 500 with x-amzn-errortype AmbiguousService and body {"__type":"AmbiguousService","message":"7 emulated services all claim requests for 'aws-marketplace': aws-marketplace, ...; the request is rejected rather than served by an arbitrary one"}. No AWS model defines an AmbiguousService error, and AWS never answers 500 to these requests. Two further defects ride along: the refusal is serialized with content-type application/x-amz-json-1.1 (awsJson1_1) although restJson1 documents are application/json; and because the request is never routed, no handler, no input validation and no state lookup runs at all, so the service is entirely unreachable (1 of 38 recorded steps passed, score 2.6%).

Why (common to all eight): a client gets a 500 server-fault instead of the modeled 2xx or 4xx client-fault, so SDKs classify it as retryable and retry requests that can never succeed; no ResourceNotFoundException / ValidationException / AccessDeniedException ever reaches the caller's error branch, and success paths return no payload at all. The wrong content-type and errortype also defeat restJson1 error-shape matching.

Fix (common to all eight): the shared request router / service-registry claim resolution — disambiguate claimants by endpointPrefix and the request Host header/URI (the restJson1 path bindings PATCH /CancelChangeSet, GET /DescribeChangeSet, POST /ListChangeSets, POST /StartChangeSet, POST /DescribeAssessment, POST /ListAssessments are themselves distinctive) rather than by sigv4 signing name alone; and the refusal serializer must emit the target service's protocol (restJson1: application/json plus x-amzn-errortype) instead of an awsJson1_1 __type body. Once routing lands, each operation's handler, its validation and its serializer must produce the modeled responses below.

Cause 1 of 8: CancelChangeSet (PATCH /CancelChangeSet) for a change set that does not exist. AWS: HTTP 404 with error ResourceNotFoundException (Smithy: CancelChangeSet errors include ResourceNotFoundException, errorKind client, httpError 404, doc "The specified resource wasn't found. HTTP status code: 404"), content-type application/json, x-amzn-errortype naming that error, and a Message built from the request naming the missing change set — recorded example "Requested change set 'sim2real9f3c8a1b2d4e5f60' does not exist." Emulator: HTTP 500, x-amzn-errortype AmbiguousService, content-type application/x-amz-json-1.1, no Message member. Why/Fix: as the shared cause. Traces: marketplace-catalog-cancel-changeset-not-found-no-side-effect-v1 step task#1; the same cluster also covers marketplace-catalog-start-and-cancel-changeset-unknown-entity-v2 step task#2.

Cause 2 of 8: DescribeChangeSet (GET /DescribeChangeSet) for a change set that does not exist. AWS: HTTP 404 ResourceNotFoundException (Smithy: DescribeChangeSet errors ResourceNotFoundException @httpError 404), application/json, Message naming the missing change set — recorded example "Requested change set 'sim2real9f3c8a1b2d4e5f60' does not exist." Emulator: HTTP 500 AmbiguousService as above. Why/Fix: as the shared cause. Traces: marketplace-catalog-cancel-changeset-not-found-no-side-effect-v1 step task#2; the same cluster also covers marketplace-catalog-describe-changeset-not-found-v2 step task#1.

Cause 3 of 8: ListChangeSets (POST /ListChangeSets) with nothing to list. AWS: HTTP 200, application/json, body carrying ChangeSetSummaryList as an empty array (Smithy: ListChangeSetsResponse.ChangeSetSummaryList, list of ChangeSetSummaryListItem; NextToken absent when there are no more results). Emulator: HTTP 500 AmbiguousService, ChangeSetSummaryList missing entirely. Why/Fix: as the shared cause — the list handler and its serializer must emit the empty collection rather than omitting it. Traces: marketplace-catalog-cancel-changeset-not-found-no-side-effect-v1 step task#3; the same cluster also covers marketplace-catalog-list-changesets-and-assessments-v2 step task#1.

Cause 4 of 8: StartChangeSet (POST /StartChangeSet) naming an entity that does not exist. AWS: HTTP 404 ResourceNotFoundException (Smithy: StartChangeSet errors ResourceNotFoundException @httpError 404), application/json, x-amzn-errortype recorded in the SDK's Code:namespace form 'ResourceNotFoundException:com.amazonaws.services.marketplacecatalog.model', Message naming the missing entity — recorded example "Requested entity 'prod-a1b2c3d4e5f6g' does not exist." Emulator: HTTP 500, x-amzn-errortype AmbiguousService. Why/Fix: as the shared cause. Traces: marketplace-catalog-start-and-cancel-changeset-unknown-entity-v2 step task#1; the same cluster also covers marketplace-catalog-start-changeset-duplicate-change-type-rejected step task#2.

Cause 5 of 8: StartChangeSet rejecting duplicate change types for one entity. AWS: HTTP 422 ValidationException (Smithy: ValidationException errorKind client, httpError 422, doc "An error occurred during validation. HTTP status code: 422"; StartChangeSet doc: "Within a single ChangeSet, you can't start the same change type against the same entity multiple times"), application/json. The message RULE is known from that documented constraint — it names the duplicated change type and the entity; recorded example "[Requested changes contain multiple changes for the same change type 'AddRevisions' on entity 'entity-id'. Entity - change type pairs should be unique.]" Emulator: HTTP 500 AmbiguousService, no validation performed. Why/Fix: as the shared cause, plus StartChangeSet input validation must enforce entity/change-type uniqueness across ChangeSet (RequestedChangeList, @length 1..20). Traces: marketplace-catalog-start-changeset-duplicate-change-type-rejected step task#1; the same cluster also covers marketplace-catalog-start-changeset-validation-error-v2 step task#1.

Cause 6 of 8: DescribeAssessment (POST /DescribeAssessment) with a malformed assessment identifier. AWS: HTTP 422 ValidationException (Smithy: DescribeAssessmentRequest.AssessmentIdentifier required, @length 1..2048, @pattern ^(assessment-[a-zA-Z0-9]+|arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*:[0-9]*:[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+)$), application/json. The message rule is built from the request plus the identifier pattern; recorded example "[Requested assessment identifier 'not-a-valid-identifier' is invalid. It should match with (^assessment-[a-zA-Z0-9]+$)|(^arn:...:assessment-[a-zA-Z0-9]+$).]" Emulator: HTTP 500 AmbiguousService. Why/Fix: as the shared cause, plus DescribeAssessment input validation. Traces: marketplace-catalog-describe-assessment-identifier-validation-boundary step task#1.

Cause 7 of 8: DescribeAssessment for a well-formed assessment identifier that does not exist. AWS: HTTP 404 ResourceNotFoundException (Smithy: DescribeAssessment errors ResourceNotFoundException @httpError 404), application/json, Message naming the missing assessment — recorded example "Requested assessment 'assessment-sim2realconv0000' does not exist." Emulator: HTTP 500 AmbiguousService. Why/Fix: as the shared cause — note the 422-vs-404 split (malformed identifier before store lookup, 404 after) must survive the fix. Traces: marketplace-catalog-describe-assessment-identifier-validation-boundary step task#2; the same cluster also covers marketplace-catalog-describe-assessment-not-found-v2 step task#1.

Cause 8 of 8: ListAssessments (POST /ListAssessments) with nothing to list. AWS: HTTP 200, application/json, body carrying AssessmentSummaryList as an empty array (Smithy: ListAssessmentsResponse.AssessmentSummaryList; NextToken null when there are no more results). Emulator: HTTP 500 AmbiguousService, AssessmentSummaryList missing. Why/Fix: as the shared cause plus empty-collection serialization. Traces: marketplace-catalog-list-changesets-and-assessments-v2 step task#2.

Related open issues (they do NOT cover these steps): '20261021T234042Z-osis-router-refuses-every-sigv4-iam-request-as-ambiguousservice', '20261021T234924Z-featurestore-runtime-sagemaker-router-refuses-every-featurestore-runtime-sagemaker-request-as' and '20261021T235106Z-medialive-channel-schedule-signal-map-checks-never-reach' report the same router defect for the 'iam' and 'sagemaker' signing-name collisions. None carries a replay key for any marketplace-catalog fixture, so fixing those will not close these eight causes; a fix that generalises claim resolution should close all of them.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true (/__admin/health answered 200 and the data listener accepts), bootstrap_signal=false, state_reset_failures=0, 18 fixtures replayed, 38 steps total, 1 passed, 37 mismatched, score 2.6%. All 21 divergence clusters are marketplace-catalog and all report actual_status 500 with the AmbiguousService body.

Per-cause expected vs actual (expected values are from the recordings; the statuses and error codes are the requirements, the Message strings are examples except where a rule is cited in the description):
- CancelChangeSet: status expected 404, got 500; $.Message expected "Requested change set 'sim2real9f3c8a1b2d4e5f60' does not exist.", got None; $headers.content-type expected 'application/json', got 'application/x-amz-json-1.1'; $headers.x-amzn-errortype got 'AmbiguousService'.
- DescribeChangeSet: status expected 404, got 500; same Message/header diffs.
- ListChangeSets: status expected 200, got 500; $.ChangeSetSummaryList expected [], got None; content-type as above.
- StartChangeSet (unknown entity): status expected 404, got 500; $.Message expected "Requested entity 'prod-a1b2c3d4e5f6g' does not exist.", got None; $headers.x-amzn-errortype expected 'ResourceNotFoundException:com.amazonaws.services.marketplacecatalog.model', got 'AmbiguousService'.
- StartChangeSet (duplicate change type): status expected 422, got 500; $.Message expected "[Requested changes contain multiple changes for the same change type 'AddRevisions' on entity 'entity-id'. Entity - change type pairs should be unique.]", got None.
- DescribeAssessment (malformed identifier): status expected 422, got 500; $.Message expected "[Requested assessment identifier 'not-a-valid-identifier' is invalid. It should match with (^assessment-[a-zA-Z0-9]+$)|(^arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*::[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+$).]", got None.
- DescribeAssessment (not found): status expected 404, got 500; $.Message expected "Requested assessment 'assessment-sim2realconv0000' does not exist.", got None.
- ListAssessments: status expected 200, got 500; $.AssessmentSummaryList expected [], got None.

Every cluster's actual body is {"__type": "AmbiguousService", "message": "7 emulated services all claim requests for 'aws-marketplace': aws-marketplace, aws-marketplace, aws-marketplace, aws-marketplace, aws-marketplace, aws-marketplace, aws-marketplace; the request is rejected rather than served by an arbitrary one"}.

Smithy evidence: service_info — sdkId "Marketplace Catalog", version 2018-09-17, protocol restJson1, endpointPrefix "catalog.marketplace", sigv4Name "aws-marketplace", arnNamespace "aws-marketplace", common_errors AccessDeniedException/InternalServiceException/ThrottlingException/ValidationException. Errors: ResourceNotFoundException httpError 404, ValidationException httpError 422, AccessDeniedException httpError 403. Sibling services confirming the collision: Marketplace Reporting (sigv4Name aws-marketplace, endpointPrefix reporting-marketplace), Marketplace Entitlement Service (sigv4Name aws-marketplace, endpointPrefix entitlement.marketplace). No AWS model defines an 'AmbiguousService' error.

Failing fixtures/steps for this bundle: marketplace-catalog-cancel-changeset-not-found-no-side-effect-v1#task#1, #task#2, #task#3; marketplace-catalog-describe-changeset-not-found-v2#task#1; marketplace-catalog-list-changesets-and-assessments-v2#task#1 and #task#2; marketplace-catalog-start-and-cancel-changeset-unknown-entity-v2#task#1 and #task#2; marketplace-catalog-start-changeset-duplicate-change-type-rejected#task#1 and #task#2; marketplace-catalog-start-changeset-validation-error-v2#task#1; marketplace-catalog-describe-assessment-identifier-validation-boundary#task#1 and #task#2; marketplace-catalog-describe-assessment-not-found-v2#task#1.

## Steps to Reproduce
1. Clone the repo and boot the emulator (/__admin/health answers 200 and the data listener accepts).
2. Point an SDK client for Marketplace Catalog (sigv4 signing name 'aws-marketplace', endpointPrefix 'catalog.marketplace', protocol restJson1) at the emulator's data listener.
3. Call CancelChangeSet(Catalog="AWSMarketplace", ChangeSetId="<id that does not exist>"). Observe HTTP 500, content-type application/x-amz-json-1.1, x-amzn-errortype AmbiguousService and body {"__type":"AmbiguousService","message":"7 emulated services all claim requests for 'aws-marketplace': ..."} instead of HTTP 404 application/json ResourceNotFoundException with a Message naming the missing change set.
4. Repeat for DescribeChangeSet (unknown id -> 404), ListChangeSets (-> 200 with ChangeSetSummaryList []), StartChangeSet with an entity id that does not exist (-> 404 ResourceNotFoundException), StartChangeSet with two changes of the same change type on one entity (-> 422 ValidationException), DescribeAssessment with AssessmentIdentifier="not-a-valid-identifier" (-> 422 ValidationException), DescribeAssessment with a well-formed but unknown assessment-... identifier (-> 404), and ListAssessments (-> 200 with AssessmentSummaryList []). Each returns the identical 500 AmbiguousService refusal, confirming the service is unreachable rather than mis-implemented per operation.
5. Replay the wire corpus simparity-wire-aws-v1 and re-run every fixture listed under the evidence section for this bundle to confirm the fix.
