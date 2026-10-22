Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaigns.concurrent-pause-campaign-single-winner connectcampaigns.concurrent-start-campaign-single-winner connectcampaigns.concurrent-stop-campaign-single-winner connectcampaigns.create-campaign-dialer-bandwidth-range-not-enforced connectcampaigns.create-campaign-invalid-connect-instance-id-rejected connectcampaigns.create-campaign-missing-connect-instance-rejected connectcampaigns.create-campaign-name-too-long-rejected connectcampaigns.delete-instance-onboarding-job-no-job-not-found connectcampaigns.describe-nonexistent-campaign-not-found connectcampaigns.get-campaign-state-batch-after-delete connectcampaigns.get-campaign-state-batch-empty-list-rejected connectcampaigns.get-campaign-state-batch-too-many-ids-rejected connectcampaigns.get-connect-instance-config-not-onboarded connectcampaigns.instance-onboarding-job-lifecycle connectcampaigns.list-campaigns-max-results-exceeds-limit-is-accepted connectcampaigns.list-campaigns-max-results-out-of-range-rejected connectcampaigns.list-tags-for-resource-nonexistent-arn-not-found connectcampaigns.put-dial-request-batch-idempotent-retry connectcampaigns.resume-initialized-campaign-rejected connectcampaigns.resume-stopped-campaign-rejected connectcampaigns.start-campaign-after-stop-rejected connectcampaigns.start-campaign-from-stopped-state-succeeds connectcampaigns.start-instance-onboarding-job-bad-instance-not-found connectcampaigns.stop-campaign-from-paused-state connectcampaigns.stop-stopped-campaign-twice-is-idempotent connectcampaigns.tag-resource-reflected-in-describe-campaign connectcampaigns.untag-resource-nonexistent-key-is-noop connectcampaigns.update-campaign-dialer-config-from-agentless-rejected
Issue-Shape-Id: com.amazonaws.connectcampaigns#AmazonConnectCampaignService
Issue-Key: connectcampaigns|StartInstanceOnboardingJob|AmbiguousService
Issue-Key: connectcampaigns.create-campaign-name-too-long-rejected|run#8|AssertionError|assert code == "ValidationException", f"{code}: {message}"
Issue-Key: connectcampaigns.describe-nonexistent-campaign-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: connectcampaigns|GetCampaignStateBatch|AmbiguousService
Issue-Key: connectcampaigns.instance-onboarding-job-lifecycle|run#10|AssertionError|assert before == not_found, before
Issue-Key: connectcampaigns.list-campaigns-max-results-exceeds-limit-is-accepted|run#3|AssertionError|assert code is None, f"expected maxResults=51 to be accepted, got {code}: {message}"
Issue-Key: connectcampaigns|ListCampaigns|AmbiguousService
Issue-Key: connectcampaigns.list-tags-for-resource-nonexistent-arn-not-found|run#5|AssertionError|assert code == "NotFoundException", f"{code}: {message}"
Issue-Key: connectcampaigns.start-instance-onboarding-job-bad-instance-not-found|run#8|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Commit: b755dbbf9c35939a76222e1fb1150f3e98238502

# ConnectCampaigns is entirely unreachable: router resolves services by SigV4 signing name, so ConnectCampaigns and ConnectCampaignsV2 both claim 'connect-campaigns' and every request is refused with AmbiguousService

> _Note: this issue was identified against the code base at commit `b755dbbf9c35939a76222e1fb1150f3e98238502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartInstanceOnboardingJob

## Description
Cause 1 of 1: the emulator's request router selects the owning service by SigV4 signing name / endpoint prefix rather than by the service's restJson1 route (HTTP method + URI template), and two emulated services legitimately share that name. Every ConnectCampaigns (v1) request is therefore rejected before dispatch with an unmodeled `AmbiguousService` error, and no ConnectCampaigns operation can be exercised at all.

AWS: ConnectCampaigns v1 (Smithy: AmazonConnectCampaignService, version 2021-01-30, protocol restJson1, aws.api#service sdkId=ConnectCampaigns, arnNamespace/sigv4Name = 'connect-campaigns', 22 operations) and ConnectCampaigns V2 (Smithy: AmazonConnectCampaignServiceV2, version 2024-04-23, restJson1, sdkId=ConnectCampaignsV2, sigv4Name = 'connect-campaigns', 37 operations) share one signing name, one arnNamespace and one endpoint hostname family (endpointRuleSet urlTemplates `https://connect-campaigns.{Region}.{dnsSuffix}` in BOTH models). In real AWS that is not ambiguous: the two API versions are disambiguated by their distinct restJson1 HTTP routes (v1 e.g. `POST /campaigns`, `GET /campaigns/{id}/state`, `POST /connect-instance/{connectInstanceId}/onboarding`; v2 uses its own URI set), so a correctly signed v1 call is served by the v1 API and returns that operation's modeled response. There is NO `AmbiguousService` error in either model; the service-common errors are AccessDeniedException, InternalServerException and ValidationException, plus per-operation ResourceNotFoundException / ConflictException / InvalidStateException / ThrottlingException.

Emulator: every request signed for 'connect-campaigns' is answered with error code `AmbiguousService` and the message (example) "2 emulated services all claim requests for 'connect-campaigns': connect-campaigns, connect-campaigns; the request is rejected rather than served by an arbitrary one". botocore classifies it as retryable, so the SDK burns its retries and surfaces "(reached max retries: 3)". This happens for every operation the suite touches: StartInstanceOnboardingJob, GetInstanceOnboardingJobStatus, DeleteInstanceOnboardingJob, GetConnectInstanceConfig, CreateCampaign, DescribeCampaign, UpdateCampaignDialerConfig, ListCampaigns, Start/Stop/Pause/ResumeCampaign, GetCampaignState, GetCampaignStateBatch, PutDialRequestBatch, TagResource, UntagResource, ListTagsForResource.

Why: ConnectCampaigns is 0% functional against the emulator. A client gets an error code that exists in no AWS model, so no SDK error-handling branch matches it; positive paths never return a campaign, and negative paths that should assert a specific modeled failure (ResourceNotFoundException for an unknown campaign id or an un-onboarded instance, ValidationException for a too-long campaign name or an out-of-range maxResults, NotFoundException for an unknown resource ARN) all collapse into the same router refusal. The retry classification also multiplies latency by 4x on every call. Downstream, every state-machine, batch-shape, tagging and idempotency behavior of the service is untestable.

Fix: the shared request router / service-resolution layer, not any ConnectCampaigns operation handler. Resolution must not be keyed on the SigV4 signing name alone when several loaded services declare the same one; for restJson1 services sharing a signing name, dispatch on the matched HTTP route (method + URI pattern) of each candidate's operation set, and only fail when no candidate route matches. `AmbiguousService` must never be a response an SDK can observe for a legitimately-signed request. Note this is the same router defect family as several already-open issues naming other signing-name collisions (e.g. 'Lambda is unreachable: four emulated services all claim the lambda signing name', 'GreengrassV2: every request rejected with AmbiguousService because Greengrass v1 and v2 share endpointPrefix greengrass', 'Kinesis Analytics V2 is entirely unreachable'); none of those issues carries ConnectCampaigns' replay keys, so this one must be closed on its own checks.

Traces: all 28 non-deferred failing connectcampaigns checks listed in trace_ids (the suite's other 33 connectcampaigns failures are separately blocked by the unimplemented Connect CreateContactFlow / ListContactFlows scaffolds and are already filed).

## Evidence
Expected (AWS, restJson1 route-based dispatch):
- `POST /connect-instance/{connectInstanceId}/onboarding` (StartInstanceOnboardingJob) against an unknown Connect instance -> HTTP 404, error code `ResourceNotFoundException`.
- `GET /campaigns/{id}` (DescribeCampaign) for a never-created id -> HTTP 404, `ResourceNotFoundException`.
- `POST /campaigns` (CreateCampaign) with a name longer than the modeled max -> HTTP 400, `ValidationException`.
- `POST /campaigns-state` (GetCampaignStateBatch) with an empty / oversized campaignIds list -> HTTP 400, `ValidationException`.
- `GET /tags/{arn}` (ListTagsForResource) for a nonexistent ARN -> HTTP 404, `NotFoundException`.
- `GET /campaigns-summary?maxResults=51` (ListCampaigns) -> accepted (no error).
- Lifecycle: CreateCampaign -> state `Initialized`; Start/Pause/Resume/Stop drive Running/Paused/Running/Stopped via GetCampaignState.

Actual (emulator): every one of those requests is answered with error code `AmbiguousService`, message e.g. "2 emulated services all claim requests for 'connect-campaigns': connect-campaigns, connect-campaigns; the request is rejected rather than served by an arbitrary one", and the SDK reports "(reached max retries: 3)" because the code is treated as retryable.

Representative replay output:
- connectcampaigns.start-instance-onboarding-job-bad-instance-not-found [mismatch]: AssertionError: expected ResourceNotFoundException, got AmbiguousService
- connectcampaigns.describe-nonexistent-campaign-not-found [mismatch]: AssertionError: expected ResourceNotFoundException, got AmbiguousService
- connectcampaigns.create-campaign-name-too-long-rejected [mismatch]: AssertionError: AmbiguousService: 2 emulated services all claim requests for 'connect-campaigns'...
- connectcampaigns.list-campaigns-max-results-exceeds-limit-is-accepted [mismatch]: AssertionError: expected maxResults=51 to be accepted, got AmbiguousService
- connectcampaigns.list-tags-for-resource-nonexistent-arn-not-found [mismatch]: AssertionError: AmbiguousService (expected NotFoundException)
- connectcampaigns.instance-onboarding-job-lifecycle [mismatch]: AssertionError: ('AmbiguousService', "2 emulated services all claim requests for 'connect-campaigns'...")
- the remaining 22 checks [error]: botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the StartInstanceOnboardingJob / GetCampaignStateBatch / ListCampaigns operation (reached max retries: 3).

Model evidence for the collision (both services loaded in this emulator and both bootstrapped):
- services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice: sdkId=ConnectCampaigns, sigv4Name='connect-campaigns', arnNamespace='connect-campaigns', restJson1.
- services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2: sdkId=ConnectCampaignsV2, sigv4Name='connect-campaigns', arnNamespace='connect-campaigns', restJson1.

## Steps to Reproduce
1. Boot the emulator from mainline with both the connectcampaigns and connectcampaignsv2 service crates loaded (the default build).
2. Point a botocore/boto3 `connectcampaigns` client at the emulator endpoint with any credentials and region.
3. Call ANY ConnectCampaigns operation, e.g. `client.list_campaigns(maxResults=51)` or `client.describe_campaign(id=str(uuid.uuid4()))` or `client.start_instance_onboarding_job(connectInstanceId='00000000-0000-0000-0000-000000000000', encryptionConfig={'enabled': False})`.
4. Observe a ClientError with code `AmbiguousService` and the message "2 emulated services all claim requests for 'connect-campaigns': connect-campaigns, connect-campaigns; the request is rejected rather than served by an arbitrary one", after 3 SDK retries, instead of the operation's modeled success or modeled error (ResourceNotFoundException / ValidationException / NotFoundException).
5. Re-replay the checks in trace_ids to confirm a fix.
