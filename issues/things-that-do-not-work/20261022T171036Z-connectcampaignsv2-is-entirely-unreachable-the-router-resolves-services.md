Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaignsv2.campaign-lifecycle-start-pause-resume-stop-transitions connectcampaignsv2.concurrent-create-campaign-same-name-one-winner connectcampaignsv2.create-campaign-invalid-instance-id-pattern-rejected connectcampaignsv2.create-campaign-nonexistent-instance-not-found connectcampaignsv2.create-campaign-persists-full-config connectcampaignsv2.delete-campaign-removes-resource connectcampaignsv2.delete-connect-instance-config-blocked-by-active-campaign-rejected connectcampaignsv2.delete-instance-onboarding-job-rejected-while-succeeded connectcampaignsv2.describe-campaign-nonexistent-id-not-found connectcampaignsv2.get-campaign-state-batch-invalid-id-pattern-rejected connectcampaignsv2.instance-onboarding-job-transitions-to-succeeded connectcampaignsv2.list-tags-for-resource-invalid-arn-format-rejected connectcampaignsv2.pause-initialized-campaign-rejected connectcampaignsv2.put-outbound-request-batch-invalid-client-token-pattern-rejected connectcampaignsv2.put-outbound-request-batch-on-initialized-campaign-rejected connectcampaignsv2.resume-initialized-campaign-rejected connectcampaignsv2.stop-initialized-campaign-rejected connectcampaignsv2.tag-resource-and-list-tags-round-trip connectcampaignsv2.tag-resource-nonexistent-arn-not-found connectcampaignsv2.update-campaign-channel-subtype-config-round-trip connectcampaignsv2.update-campaign-communication-limits-override-round-trip connectcampaignsv2.update-campaign-entry-limits-persists connectcampaignsv2.update-campaign-name-persists connectcampaignsv2.update-campaign-schedule-persists connectcampaignsv2.update-campaign-source-succeeds-while-initialized
Issue-Shape-Id: com.amazonaws.connectcampaignsv2#AmazonConnectCampaignServiceV2
Issue-Key: connectcampaignsv2|StartInstanceOnboardingJob|AmbiguousService
Issue-Key: connectcampaignsv2.create-campaign-invalid-instance-id-pattern-rejected|run#3|AssertionError|assert control["Code"] == "ResourceNotFoundException", (
Issue-Key: connectcampaignsv2|GetCampaignStateBatch|AmbiguousService
Issue-Key: connectcampaignsv2.instance-onboarding-job-transitions-to-succeeded|run#5|AssertionError|assert err["Code"] == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {err['Code']}"
Issue-Key: connectcampaignsv2.list-tags-for-resource-invalid-arn-format-rejected|run#6|AssertionError|assert control["Code"] == "NotFoundException", (
Issue-Key: connectcampaignsv2.put-outbound-request-batch-invalid-client-token-pattern-rejected|run#7|AssertionError|assert positive["Code"] == "ResourceNotFoundException", (
Issue-Key: connectcampaignsv2.tag-resource-nonexistent-arn-not-found|run#6|AssertionError|assert err["Code"] == "NotFoundException", f"expected NotFoundException, got {err['Code']}"
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# ConnectCampaignsV2 is entirely unreachable: the router resolves services by SigV4 signing name, so ConnectCampaigns v1 and V2 both claim 'connect-campaigns' and every V2 request is refused with an unmodeled AmbiguousService error

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartInstanceOnboardingJob

## Description
Cause 1 of 1: the emulator's request router selects the owning service by SigV4 signing name / endpoint prefix instead of by the service's restJson1 route (HTTP method + URI template). Two emulated services legitimately declare the same signing name 'connect-campaigns', so the router refuses the request before dispatch with an unmodeled `AmbiguousService` error. Every ConnectCampaignsV2 request is rejected this way, and not one V2 operation can be exercised: all 25 connectcampaignsv2 behavioral checks fail on the very first call they make (StartInstanceOnboardingJob or GetCampaignStateBatch).

AWS: ConnectCampaignsV2 (Smithy: AmazonConnectCampaignServiceV2, version 2024-04-23, protocol restJson1, aws.api#service sdkId=ConnectCampaignsV2, arnNamespace='connect-campaigns', sigv4Name='connect-campaigns', 37 operations) shares its signing name, its arnNamespace and its endpoint hostname family (endpointRuleSet urlTemplates `https://connect-campaigns.{Region}.{dnsSuffix}`) with ConnectCampaigns v1 (Smithy: AmazonConnectCampaignService, version 2021-01-30, restJson1, sdkId=ConnectCampaigns, same sigv4Name and arnNamespace). On real AWS that is not ambiguous: the two API versions are disambiguated by their disjoint restJson1 HTTP routes. V2's routes are all `/v2/`-prefixed — Smithy: StartInstanceOnboardingJob `PUT /v2/connect-instance/{connectInstanceId}/onboarding`, CreateCampaign `PUT /v2/campaigns`, GetCampaignStateBatch `POST /v2/campaigns-state`, PutOutboundRequestBatch `PUT /v2/campaigns/{id}/outbound-requests`, TagResource `POST /v2/tags/{arn}`, ListTagsForResource `GET /v2/tags/{arn}` — whereas v1's are unprefixed (`PUT /connect-instance/{connectInstanceId}/onboarding`, `PUT /campaigns`, `POST /campaigns-state`, `PUT /campaigns/{id}/dial-requests`, `POST /tags/{arn}`, `GET /tags/{arn}`). So a correctly signed V2 call is served by the V2 API and returns that operation's modeled response. There is NO `AmbiguousService` error in either model: the V2 service-common errors are AccessDeniedException (403), InternalServerException (500) and ValidationException (400), plus per-operation ResourceNotFoundException (404), ConflictException (409), InvalidCampaignStateException (409), ServiceQuotaExceededException (402) and ThrottlingException (429).

Emulator: every request signed for 'connect-campaigns' is answered with error code `AmbiguousService` and the message (example) "2 emulated services all claim requests for 'connect-campaigns': connect-campaigns, connect-campaigns; the request is rejected rather than served by an arbitrary one". botocore classifies the code as retryable, so the SDK burns its retry budget and surfaces "(reached max retries: 3)". Twenty checks never get past their first call (ClientError raised); five reach an assertion and report the router code where a modeled client error was expected, e.g. `expected ResourceNotFoundException, got AmbiguousService` and `expected NotFoundException, got AmbiguousService`.

Why: ConnectCampaignsV2 is 0% functional against the emulator. A client receives an error code that exists in no AWS model, so no SDK error-handling branch matches it and no retry/backoff decision is correct (the code is treated as retryable, multiplying call latency ~4x). Positive paths never return a campaign, an onboarding job or a tag set; negative paths that must assert a specific modeled failure (404 ResourceNotFoundException for an unknown campaign id, instance or ARN; 400 ValidationException for a pattern-invalid connectInstanceId/campaignId/clientToken; 409 for a state refusal) all collapse into the same pre-dispatch refusal. Every V2 state-machine, batch-shape, tagging, update and idempotency behavior is untestable.

Fix: the shared request router / service-resolution layer, not any ConnectCampaignsV2 operation handler. Service resolution must not be keyed on the SigV4 signing name alone when several loaded services declare the same one; for restJson1 services sharing a signing name, dispatch on the matched HTTP route (method + URI template) of each candidate's operation set — V1 and V2 routes here are disjoint — and fail only when no candidate route matches. `AmbiguousService` must never be an error an SDK can observe for a legitimately signed request.

Related already-open issues: the same router defect family is filed for other signing-name collisions, including "ConnectCampaigns is entirely unreachable: router resolves services by SigV4 signing name, so ConnectCampaigns and ConnectCampaignsV2 both claim 'connect-campaigns'" (20261022T145747Z-connectcampaigns-is-entirely-unreachable-router-resolves-services-by), plus the GreengrassV2, Kinesis Analytics V2, Chime SDK, Lex, Bedrock and Lambda collision issues. None of those carries the connectcampaignsv2 replay keys — the v1 issue is grounded exclusively in `connectcampaigns.*` checks — so this issue must be closed on the 25 connectcampaignsv2 checks in trace_ids.

Traces: all 25 failing connectcampaignsv2 checks, listed in trace_ids.

## Evidence
Expected (AWS, route-based dispatch of the /v2/ restJson1 routes):
- `PUT /v2/connect-instance/{connectInstanceId}/onboarding` (StartInstanceOnboardingJob) for an unknown Connect instance -> HTTP 404, error code `ResourceNotFoundException`; for a valid onboarded instance -> HTTP 200 with `connectInstanceOnboardingJobStatus`.
- `PUT /v2/campaigns` (CreateCampaign) with a pattern-invalid `connectInstanceId` (@pattern ^[-_.a-zA-Z0-9]+$) -> HTTP 400 `ValidationException`; with a well-formed but nonexistent instance -> HTTP 404 `ResourceNotFoundException`.
- `GET /v2/campaigns/{id}` (DescribeCampaign) for a never-created id -> HTTP 404 `ResourceNotFoundException`.
- `POST /v2/campaigns-state` (GetCampaignStateBatch) with a pattern-invalid campaign id -> HTTP 400 `ValidationException`.
- `GET /v2/tags/{arn}` / `POST /v2/tags/{arn}` (ListTagsForResource / TagResource) for a well-formed nonexistent ARN -> HTTP 404 `NotFoundException`.
- `PUT /v2/campaigns/{id}/outbound-requests` (PutOutboundRequestBatch) against a nonexistent campaign -> HTTP 404 `ResourceNotFoundException`; against an Initialized/Paused campaign -> HTTP 409 (ConflictException / InvalidCampaignStateException).

Actual (emulator): every one of those requests is answered with error code `AmbiguousService`, message e.g. "2 emulated services all claim requests for 'connect-campaigns': connect-campaigns, connect-campaigns; the request is rejected rather than served by an arbitrary one", and the SDK reports "(reached max retries: 3)" because the code is treated as retryable. The suite result is 0/25 passed (0.0%) for connectcampaignsv2: 20 errored, 5 mismatched.

Representative replay output:
- connectcampaignsv2.campaign-lifecycle-start-pause-resume-stop-transitions [error]: botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the StartInstanceOnboardingJob operation (reached max retries: 3): 2 emulated services all claim requests for 'connect-campaigns': connect-campaigns, connect-campaigns; the request is rejected rather than served by an arbitrary one
- connectcampaignsv2.get-campaign-state-batch-invalid-id-pattern-rejected [error]: same AmbiguousService refusal, on GetCampaignStateBatch
- connectcampaignsv2.create-campaign-invalid-instance-id-pattern-rejected [mismatch]: AssertionError: expected the well-formed-but-nonexistent control to get ResourceNotFoundException, got AmbiguousService
- connectcampaignsv2.instance-onboarding-job-transitions-to-succeeded [mismatch]: AssertionError: expected ResourceNotFoundException, got AmbiguousService
- connectcampaignsv2.list-tags-for-resource-invalid-arn-format-rejected [mismatch]: AssertionError: expected NotFoundException for well-formed nonexistent arn, got AmbiguousService
- connectcampaignsv2.tag-resource-nonexistent-arn-not-found [mismatch]: AssertionError: expected NotFoundException, got AmbiguousService
- connectcampaignsv2.put-outbound-request-batch-invalid-client-token-pattern-rejected [mismatch]: AssertionError: positive control (pattern-valid clientToken) expected to reach the campaign lookup and fail with ResourceNotFoundException, got AmbiguousService
- the remaining 18 checks [error]: identical AmbiguousService ClientError on their first call.

Model evidence for the collision (both services loaded and bootstrapped in this emulator):
- services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/smithy-model/com-amazonaws-connectcampaigns-amazonconnectcampaignservice.json: sdkId=ConnectCampaigns, sigv4Name='connect-campaigns', arnNamespace='connect-campaigns', restJson1, unprefixed URIs.
- services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/smithy-model/com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2.json: sdkId=ConnectCampaignsV2, sigv4Name='connect-campaigns', arnNamespace='connect-campaigns', restJson1, all URIs under `/v2/`.

## Steps to Reproduce
1. Boot the emulator from mainline with both the connectcampaigns and connectcampaignsv2 service crates loaded (the default build).
2. Point a botocore/boto3 `connectcampaignsv2` client at the emulator endpoint with any credentials and region.
3. Call ANY ConnectCampaignsV2 operation, e.g. `client.start_instance_onboarding_job(connectInstanceId='00000000-0000-0000-0000-000000000000', encryptionConfig={'enabled': False})`, or `client.get_campaign_state_batch(campaignIds=['not a valid id'])`, or `client.list_tags_for_resource(arn='arn:aws:connect-campaigns:us-east-1:123456789012:campaign/00000000-0000-0000-0000-000000000000')`.
4. Observe a ClientError with code `AmbiguousService` and message "2 emulated services all claim requests for 'connect-campaigns': connect-campaigns, connect-campaigns; the request is rejected rather than served by an arbitrary one", after 3 SDK retries, instead of the operation's modeled 200 response or modeled 400/404/409 error.
5. Confirm the routes are disjoint and therefore disambiguable: the V2 request path is `/v2/...` while every v1 route is unprefixed.
6. Re-replay the 25 checks in trace_ids to confirm a fix.
