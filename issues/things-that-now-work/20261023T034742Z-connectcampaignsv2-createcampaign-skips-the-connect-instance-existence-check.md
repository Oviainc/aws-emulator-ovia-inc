Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaignsv2.create-campaign-invalid-instance-id-pattern-rejected connectcampaignsv2.put-outbound-request-batch-invalid-client-token-pattern-rejected connectcampaignsv2.instance-onboarding-job-transitions-to-succeeded connectcampaignsv2.list-tags-for-resource-invalid-arn-format-rejected connectcampaignsv2.tag-resource-nonexistent-arn-not-found
Issue-Shape-Id: com.amazonaws.connectcampaignsv2#AmazonConnectCampaignServiceV2
Issue-Key: connectcampaignsv2.create-campaign-invalid-instance-id-pattern-rejected|_error#2|AssertionError|raise AssertionError("expected the call to raise a ClientError, but it succeeded")
Issue-Key: connectcampaignsv2.put-outbound-request-batch-invalid-client-token-pattern-rejected|run#7|AssertionError|assert positive["Code"] == "ResourceNotFoundException", (
Issue-Key: connectcampaignsv2.instance-onboarding-job-transitions-to-succeeded|run#5|AssertionError|assert err["Code"] == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {err['Code']}"
Issue-Key: connectcampaignsv2.list-tags-for-resource-invalid-arn-format-rejected|run#6|AssertionError|assert control["Code"] == "NotFoundException", (
Issue-Key: connectcampaignsv2.tag-resource-nonexistent-arn-not-found|run#6|AssertionError|assert err["Code"] == "NotFoundException", f"expected NotFoundException, got {err['Code']}"
Issue-Commit: d66da8d621774a402898e98aa83287c30e78f3fd

# ConnectCampaignsV2: CreateCampaign skips the Connect-instance existence check, PutOutboundRequestBatch over-rejects a valid batch with ValidationException, and GetConnectInstanceConfig / TagResource / ListTagsForResource answer 501 NotImplemented

> _Note: this issue was identified against the code base at commit `d66da8d621774a402898e98aa83287c30e78f3fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCampaign

## Description
Five distinct root causes in the ConnectCampaignsV2 emulator (crate services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2). All five are reached — the service itself IS routed and answering (unlike the already-open ConnectCampaigns *v1* router issue '20261022T145747Z-connectcampaigns-is-entirely-unreachable-router-resolves-services-by', which concerns service token `connectcampaigns`, not `connectcampaignsv2`, and does not cover any of these). The two checks blocked by the unimplemented StartInstanceOnboardingJob / GetCampaignStateBatch scaffolds are excluded here; they are filed separately.

Cause 1 of 5 — CreateCampaign accepts a connectInstanceId that belongs to no onboarded Connect instance.
AWS: Smithy CreateCampaign (PUT /v2/campaigns) declares connectInstanceId (InstanceId, len 1..256, pattern ^[-_.a-zA-Z0-9]+$, required) and models ResourceNotFoundException (HTTP 404). Recorded behavior: a syntactically valid but nonexistent/non-onboarded instance id is refused with HTTP 404, error code ResourceNotFoundException (recorded message example: "Connect Instance does not exist or is not enabled to use Campaigns."), i.e. the create fails the instance existence/onboarding check before any campaign is stored.
Emulator: the call SUCCEEDS and returns a campaign id/arn. services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/ops/create_campaign.rs stores the record unconditionally (only ServiceQuotaExceededException / InternalServerException are ever returned); nothing consults an instance-config store.
Why: a client that provisions a campaign against a wrong or un-onboarded instance id gets a 200 and a campaign id that AWS would never have minted, so error-handling branches for ResourceNotFoundException are dead and the emulator's campaign store diverges from what the account actually holds. It also makes the pattern-validation negative class indistinguishable from "CreateCampaign rejects nothing".
Fix: CreateCampaign's handler/validation — gate the create on the ConnectCampaignsV2 instance-config state (the same state GetConnectInstanceConfig / onboarding own) and return the modeled ResourceNotFoundException (404) when the instance is unknown or not onboarded for Campaigns.
Traces: connectcampaignsv2.create-campaign-invalid-instance-id-pattern-rejected (AssertionError: expected the call to raise a ClientError, but it succeeded).

Cause 2 of 5 — PutOutboundRequestBatch rejects a modeled-valid batch with ValidationException before the handler runs.
AWS: Smithy PutOutboundRequestBatch (PUT /v2/campaigns/{id}/outbound-requests); OutboundRequest requires clientToken (ClientToken, len 0..200, pattern ^[a-zA-Z0-9_\-.]*$), expirationTime (TimeStamp, date-time) and channelSubtypeParameters (union); TelephonyChannelSubtypeParameters requires destinationPhoneNumber (len 0..20, pattern ^[\d\-+]*$) and attributes (Attributes map, no minimum size). Modeled errors include ResourceNotFoundException (404) and ValidationException (400). Recorded behavior: a batch of one request with a UUID clientToken, a future expirationTime, destinationPhoneNumber "+15555550100" and an EMPTY attributes map against an unknown campaign id clears shape validation and is refused with HTTP 404 ResourceNotFoundException (recorded message example: "Campaign: <id> doesn't exist").
Emulator: the identical request is refused with ValidationException instead, so the request never reaches the campaign lookup. Hypothesis: the pre-handler constraint walk rejects a member the model permits — most likely the required-but-empty `attributes` map being treated as absent, or the date-time expirationTime / the channelSubtypeParameters union discriminator. Note also that even past validation the operation is still a scaffold stub (src/ops/put_outbound_request_batch.rs returns Scaffolded::not_implemented, HTTP 501 NotImplemented), so the 404 lookup path has to be written as part of this fix.
Why: every data-plane dial submission is rejected as a client error, and the rejection is a 400 the client cannot act on; a client cannot distinguish a malformed request from an unknown campaign, and the genuine malformed-clientToken negative class is untestable because valid and invalid inputs get the same code.
Fix: the shared input-validation layer for this operation's input shape (stop rejecting modeled-valid members), plus PutOutboundRequestBatch's handler so an unknown campaign id yields the modeled ResourceNotFoundException (404) and a known one yields per-request successfulRequests/failedRequests.
Traces: connectcampaignsv2.put-outbound-request-batch-invalid-client-token-pattern-rejected (AssertionError: positive control (pattern-valid clientToken) expected ... ResourceNotFoundException, got ValidationException).

Cause 3 of 5 — GetConnectInstanceConfig is an unimplemented scaffold (501 NotImplemented).
AWS: Smithy GetConnectInstanceConfig (GET /v2/connect-instance/{connectInstanceId}/config) models ResourceNotFoundException (HTTP 404) and returns connectInstanceConfig once the instance is onboarded. Recorded behavior: before onboarding the call 404s with code ResourceNotFoundException (recorded message example: "ConnectInstanceConfig not found <instanceId>."); after onboarding it returns the persisted encryptionConfig.
Emulator: answers error code NotImplemented ("operation GetConnectInstanceConfig is scaffolded but not implemented in this emulator") — see src/ops/get_connect_instance_config.rs, which still carries the SCAFFOLDED marker and returns Scaffolded::not_implemented.
Why: NotImplemented exists in no AWS model, so no SDK error branch matches it; a client cannot probe whether an instance is onboarded, and the whole onboarding workflow (and Cause 1's existence check) has no readable state.
Fix: implement GetConnectInstanceConfig in its handler file against the instance-config state, returning the modeled 404 ResourceNotFoundException when no config exists and the stored config otherwise.
Traces: connectcampaignsv2.instance-onboarding-job-transitions-to-succeeded (AssertionError: expected ResourceNotFoundException, got NotImplemented).

Cause 4 of 5 — TagResource is an unimplemented scaffold (501 NotImplemented).
AWS: Smithy TagResource (POST /v2/tags/{arn}) requires arn (Arn, len 20..500, ARN pattern) and tags, and models a 404 not-found error (model: ResourceNotFoundException). Recorded behavior for a well-formed, correct-account `campaign/<uuid>` ARN that matches no campaign: HTTP 404 with error code NotFoundException (recorded message example: "Resource was not found"), and nothing is persisted. (The recording's wire code NotFoundException differs from the model's shape name ResourceNotFoundException; the recorded code is what a client sees.)
Emulator: answers error code NotImplemented — src/ops/tag_resource.rs still returns Scaffolded::not_implemented("TagResource").
Why: tagging is unusable and the 404 negative class is unobservable; clients retrying on an unmodeled code loop or fail opaquely.
Fix: implement TagResource in its handler file — parse the ARN, resolve the campaign in service state, 404 when absent, otherwise merge the tags into the stored record.
Traces: connectcampaignsv2.tag-resource-nonexistent-arn-not-found (AssertionError: expected NotFoundException, got NotImplemented).

Cause 5 of 5 — ListTagsForResource is an unimplemented scaffold (501 NotImplemented).
AWS: Smithy ListTagsForResource (GET /v2/tags/{arn}) models a 404 not-found error and a 400 ValidationException. Recorded behavior: a well-formed `campaign/<uuid>` ARN that matches nothing -> HTTP 404, code NotFoundException (message example "Resource was not found"); an ARN in the caller's own partition/region/account whose resource path is not a recognized connect-campaigns resource type -> HTTP 400, code BadRequestException (message example "Invalid input resource arn"), i.e. resource-path parsing fails before any existence check.
Emulator: answers error code NotImplemented — src/ops/list_tags_for_resource.rs still returns Scaffolded::not_implemented, so neither the 404 nor the 400 class exists.
Why: tag reads are impossible and the two distinct rejection classes (malformed resource path vs unknown resource) collapse into one unmodeled code.
Fix: implement ListTagsForResource in its handler file with ARN-shape parsing first (400 for an unrecognized resource path), then existence (404), then the stored tag map.
Traces: connectcampaignsv2.list-tags-for-resource-invalid-arn-format-rejected (AssertionError: expected NotFoundException for well-formed nonexistent arn, got NotImplemented).

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from mainline: connectcampaignsv2 scored 0/25 (0%). Twenty of those failures are blocked by the already-filed StartInstanceOnboardingJob / GetCampaignStateBatch scaffolds; the five below are the net-new causes.

Cause 1 — CreateCampaign(name=<unique>, connectInstanceId=<random uuid>):
  Expected (AWS): HTTP 404, Error.Code=ResourceNotFoundException, message e.g. "Connect Instance does not exist or is not enabled to use Campaigns."
  Actual (emulator): HTTP 200 with {id, arn} — check aborted with "AssertionError: expected the call to raise a ClientError, but it succeeded".

Cause 2 — PutOutboundRequestBatch(id=<random uuid>, outboundRequests=[{clientToken=<uuid>, expirationTime=now+1h, channelSubtypeParameters={telephony:{destinationPhoneNumber:'+15555550100', attributes:{}}}}]):
  Expected (AWS): HTTP 404, Error.Code=ResourceNotFoundException, message e.g. "Campaign: <id> doesn't exist".
  Actual (emulator): Error.Code=ValidationException — "AssertionError: positive control (pattern-valid clientToken) expected to reach the campaign lookup and fail with ResourceNotFoundException, got ValidationException". (The handler itself is still a 501 stub, so the 404 path does not exist either.)

Cause 3 — GetConnectInstanceConfig(connectInstanceId=<a Connect instance with no onboarding record>):
  Expected (AWS): HTTP 404, Error.Code=ResourceNotFoundException, message e.g. "ConnectInstanceConfig not found <instanceId>."
  Actual (emulator): HTTP 501, Error.Code=NotImplemented, "operation GetConnectInstanceConfig is scaffolded but not implemented in this emulator" — "AssertionError: expected ResourceNotFoundException, got NotImplemented".

Cause 4 — TagResource(arn='arn:aws:connect-campaigns:<region>:<account>:campaign/<uuid>', tags={owner:...}):
  Expected (AWS): HTTP 404, Error.Code=NotFoundException, message e.g. "Resource was not found".
  Actual (emulator): Error.Code=NotImplemented — "AssertionError: expected NotFoundException, got NotImplemented".

Cause 5 — ListTagsForResource(arn='arn:aws:connect-campaigns:<region>:<account>:campaign/<uuid>') and ListTagsForResource(arn='arn:aws:connect-campaigns:<region>:<account>:not-a-real-resource-path'):
  Expected (AWS): 404 NotFoundException for the first; 400 BadRequestException ("Invalid input resource arn") for the second.
  Actual (emulator): Error.Code=NotImplemented for the first — "AssertionError: expected NotFoundException for well-formed nonexistent arn, got NotImplemented"; the 400 class was never reached.

Source evidence (commit as cloned from mainline):
- src/ops/create_campaign.rs: handle() inserts the campaign with no instance lookup; only ServiceQuotaExceededException/InternalServerException are returned.
- src/ops/put_outbound_request_batch.rs:172: Err(crate::scaffolded::Scaffolded::not_implemented("PutOutboundRequestBatch")).
- src/ops/get_connect_instance_config.rs:4, src/ops/tag_resource.rs:4 and :183, src/ops/list_tags_for_resource.rs:4: "SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented".
- Model: services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/smithy-model/com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2.json (CreateCampaign errors include ResourceNotFoundException 404; PutOutboundRequestBatch errors include ResourceNotFoundException 404; Attributes map has no minimum size; TagResource/ListTagsForResource error sets include a 404).

## Steps to Reproduce
1. Clone mainline and boot the emulator with the connectcampaignsv2 service crate loaded (default build).
2. Point a boto3/botocore `connectcampaignsv2` client at the emulator endpoint (any credentials, any region).
3. Cause 1: client.create_campaign(name='x', connectInstanceId=str(uuid.uuid4())) -> observe HTTP 200 and a minted id instead of a 404 ResourceNotFoundException.
4. Cause 2: client.put_outbound_request_batch(id=str(uuid.uuid4()), outboundRequests=[{'clientToken': str(uuid.uuid4()), 'expirationTime': datetime.now(timezone.utc)+timedelta(hours=1), 'channelSubtypeParameters': {'telephony': {'destinationPhoneNumber': '+15555550100', 'attributes': {}}}}]) -> observe ValidationException instead of a 404 ResourceNotFoundException.
5. Cause 3: client.get_connect_instance_config(connectInstanceId='00000000-0000-0000-0000-000000000000') -> observe NotImplemented (501) instead of a 404 ResourceNotFoundException.
6. Cause 4: client.tag_resource(arn='arn:aws:connect-campaigns:us-east-1:123456789012:campaign/'+str(uuid.uuid4()), tags={'owner':'x'}) -> observe NotImplemented instead of a 404 NotFoundException.
7. Cause 5: client.list_tags_for_resource(arn=<same ARN>) -> observe NotImplemented instead of a 404 NotFoundException; then call it with arn='arn:aws:connect-campaigns:us-east-1:123456789012:not-a-real-resource-path' -> should be a 400 BadRequestException.
8. Re-replay the five checks in trace_ids to confirm all five causes are closed.
