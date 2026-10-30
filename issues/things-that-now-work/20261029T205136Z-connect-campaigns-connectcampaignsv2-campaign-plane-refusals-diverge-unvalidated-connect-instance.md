Issue-Source: trace-sweep-divergence
Issue-Trace: connectcampaigns-error-create-campaign-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcampaigns-error-create-campaign-validation connectcampaigns-error-operations-on-missing-campaign connectcampaigns-error-validation-boundaries
Issue-Shape-Id: com.amazonaws.connectcampaigns#AmazonConnectCampaignService
Issue-Key: connect-campaigns|CreateCampaign|4xx->2xx|$.message
Issue-Key: connect-campaigns|CreateCampaign|4xx->2xx|$headers.content-type
Issue-Key: connect-campaigns|CreateCampaign|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: connect-campaigns|CreateCampaign|4xx->2xx|status
Issue-Key: connect-campaigns|DescribeCampaign|4xx->4xx|$.message
Issue-Key: connect-campaigns|ListCampaigns|4xx->4xx|$.message
Issue-Key: connect-campaigns|ListCampaigns|4xx->4xx|$headers.content-type
Issue-Related: 20261029T183744Z-connect-campaigns-connectcampaignsv2-responses-carry-the-wrong-content-type-on.md
Issue-Commit: 4bd656ceb6ae5ffdb198db72ede94ac7bf108594

# ConnectCampaignsV2 campaign-plane refusals diverge: unvalidated Connect instance and wrong ValidationException/ResourceNotFoundException messages

> _Note: this issue was identified against the code base at commit `4bd656ceb6ae5ffdb198db72ede94ac7bf108594`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCampaign

## Description
THREE causes, all in the ConnectCampaignsV2 campaign plane (service token `connect-campaigns`), all on the refusal path: one refusal that is not produced at all, and two whose error message text is the emulator's own wording rather than the service's. A fixer must close all three before submitting.

None of these three is covered by the two open ConnectCampaigns issues. `20261029T183744Z-connect-campaigns-connectcampaignsv2-responses-carry-the-wrong-content-type-on.md` owns ONLY the `$headers.content-type` divergence that every reply of this service also carries (it is listed here in the evidence for completeness and must NOT be fixed again as part of this issue), and `20261022T145747Z-connectcampaigns-is-entirely-unreachable-router-resolves-services-by.md` is about the v1 ConnectCampaigns service being shadowed in the router. The body/status divergences below are net-new.

=== Cause 1 of 3: CreateCampaign accepts a campaign for a Connect instance that is not onboarded, answering 200 instead of a 400 ValidationException ===
Fixture connectcampaigns-error-create-campaign-validation, step task#1 (PUT /v2/campaigns).
AWS: CreateCampaign refuses a request whose `connectInstanceId` names a Connect instance that does not exist, or that exists but has not been onboarded/enabled for Campaigns. Smithy: CreateCampaign input member `connectInstanceId` is @required (target InstanceId, @length min 1 max 256, @pattern ^[-_.a-zA-Z0-9]+$); CreateCampaign models error ValidationException with errorKind client and httpError 400. The recording shows the refusal as HTTP 400 with `x-amzn-errortype: ValidationException` and body member `message` = the fixed literal "Connect Instance does not exist or is not enabled to use Campaigns." — this message has NO variable part: it does not echo the instance id, so it is reproduced verbatim for this error class. Note the syntactic constraints on the id are satisfied by the request; the refusal is a referential check against instance state, not a pattern/length check.
Emulator: answers HTTP 200 with a CreateCampaignResponse (an `id`/`arn` for a newly created campaign) and no `message` member and no `x-amzn-errortype` header. It performs no existence/onboarding check on `connectInstanceId` at all, so the campaign is created against an instance that was never onboarded.
Why: a client that relies on AWS to reject campaigns for un-onboarded instances gets a success and a campaign id back. Error-handling paths (catching ValidationException to prompt the caller to run StartInstanceOnboardingJob first) are never exercised, and the emulator's state is left holding a campaign that AWS would never have created — so every later call against that campaign id diverges too. A 200-for-400 is the worst direction of divergence: the failure surfaces far from its cause.
Fix: the CreateCampaign handler's request validation in the ConnectCampaignsV2 crate — before inserting the campaign, resolve `connectInstanceId` against the instance-config/onboarding state the service keeps (the same state GetConnectInstanceConfig and GetInstanceOnboardingJobStatus read) and, when it is absent or not enabled for Campaigns, return ValidationException (HTTP 400, `x-amzn-errortype: ValidationException`) with the fixed message above instead of creating the resource.

=== Cause 2 of 3: DescribeCampaign's ResourceNotFoundException message is the emulator's own wording ===
Fixture connectcampaigns-error-operations-on-missing-campaign, step task#1 (GET /v2/campaigns/{id}).
AWS: DescribeCampaign on an unknown campaign id returns ResourceNotFoundException. Smithy: DescribeCampaign models ResourceNotFoundException, errorKind client, httpError 404; input member `id` is @required and @httpLabel. The recorded message is "CampaignId not found <id>." — fixed text "CampaignId not found ", then the campaign id taken verbatim from the request's {id} path label, then a terminating ".". This is the campaign-id-lookup wording the service uses on the describe/delete path; the lifecycle operations on the same resource word it differently ("Campaign: <id> doesn't exist"), which is why the message cannot be produced by one shared helper string.
Emulator: returns the right status (404) but the message "No campaign with id <id> exists" — fixed text that matches no AWS error message for this class, with the id interpolated in a different position.
Why: the status is right, so a client's control flow works, but anything that surfaces, logs, matches or asserts on the service's message text sees wording AWS never produces. Recorded-response tests and user-facing error reporting diverge on every not-found describe.
Fix: the error-message construction for the not-found path of the DescribeCampaign handler (and the DeleteCampaign handler, which the recording shows uses the SAME "CampaignId not found <id>." wording) in the ConnectCampaignsV2 crate. It must be kept distinct from the lifecycle operations' "Campaign: <id> doesn't exist" wording rather than unified with it.

=== Cause 3 of 3: ListCampaigns reports an undecodable nextToken with its own wording instead of the base64 decode failure ===
Fixture connectcampaigns-error-validation-boundaries, step task#2 (POST /v2/campaigns-summary).
AWS: ListCampaigns' pagination token is an opaque base64-encoded value. Smithy: ListCampaigns input member `nextToken` targets NextToken (@length max 1000) and the operation models ValidationException (errorKind client, httpError 400); it is @paginated with inputToken/outputToken `nextToken`. A token that is within the length bound but is not valid base64 is refused as it fails to decode: HTTP 400, ValidationException, message "Illegal base64 character <hh>" — fixed text "Illegal base64 character " followed by the lowercase hex code of the first byte in the supplied token that is not a legal base64 character (in the recording the token begins "not-a-valid-...", whose first illegal character is '-' = 2d). The variable part is derived from the request's token, not from the recording.
Emulator: returns the correct 400 but message "nextToken <token> is not a valid page token" — its own wording, echoing the whole token, and it says nothing about the base64 decode that actually failed.
Why: a caller cannot distinguish "token is corrupt/not base64" from "token is structurally fine but stale", which is the distinction AWS's message draws; and any assertion on the refusal text diverges. The emulator also echoes the caller-supplied token into the message, which AWS does not do here.
Fix: the nextToken decoding/validation step of the ListCampaigns handler in the ConnectCampaignsV2 crate — decode the token as base64 first and, on failure, raise ValidationException with the "Illegal base64 character <hh>" message naming the offending character's hex code, reserving a separate message for a token that decodes but does not resolve.

Traces: connectcampaigns-error-create-campaign-validation#task#1 (Cause 1), connectcampaigns-error-operations-on-missing-campaign#task#1 (Cause 2), connectcampaigns-error-validation-boundaries#task#2 (Cause 3). All three must be re-replayed: they are three independent code paths and a fix to one leaves the others diverging.

## Evidence
Replay of trace set simparity-wire-aws-v1 (20 steps replayed, 0 passed). The three net-new clusters:

Cause 1 — connectcampaigns-error-create-campaign-validation#task#1, CreateCampaign, verdict mismatch:
  status: expected 400, got 200
  $.message: missing (expected 'Connect Instance does not exist or is not enabled to use Campaigns.', got None)
  $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)
  $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/json')   <-- owned by the open Content-Type issue, not part of this one
  Expected (AWS): 400 ValidationException, x-amzn-errortype: ValidationException, message = fixed literal 'Connect Instance does not exist or is not enabled to use Campaigns.' (no variable part).
  Actual (emulator): 200 with a CreateCampaignResponse body; the campaign is created.

Cause 2 — connectcampaigns-error-operations-on-missing-campaign#task#1, DescribeCampaign, verdict mismatch (status 404 -> 404):
  $.message: value_mismatch (expected 'CampaignId not found 9f1bce54-63f3-4d66-8792-854d41904074.', got 'No campaign with id 9f1bce54-63f3-4d66-8792-854d41904074 exists')
  Fixed text: 'CampaignId not found ' + <campaign id from the {id} path label> + '.'. The UUID shown is this recording's request value, not part of the requirement.
  (The sibling DeleteCampaign step task#10 shows the identical wording and is listed as owned by the Content-Type issue; fixing the message helper for both paths is in scope here.)

Cause 3 — connectcampaigns-error-validation-boundaries#task#2, ListCampaigns, verdict mismatch (status 400 -> 400):
  $.message: value_mismatch (expected 'Illegal base64 character 2d', got 'nextToken not-a-valid-pagination-token is not a valid page token')
  Fixed text: 'Illegal base64 character ' + <lowercase hex of the first non-base64 byte of the supplied nextToken>. '2d' is the hex for '-', the first illegal character of the token this recording sent.

For reference, the six other clusters in this corpus (GetCampaignState/PauseCampaign/StartCampaign/StopCampaign/DeleteCampaign task#2,3,5,6,10 and ListCampaigns task#1) are reported as owned by 20261029T183744Z-connect-campaigns-connectcampaignsv2-responses-carry-the-wrong-content-type-on.md and are deliberately NOT re-filed here.

## Steps to Reproduce
1. Boot the emulator from mainline and point a SigV4-signed ConnectCampaignsV2 client (signing name 'connect-campaigns') at the data listener.
2. Cause 1: PUT /v2/campaigns (CreateCampaign) with a syntactically valid `connectInstanceId` that was never onboarded (no StartInstanceOnboardingJob / no instance config) plus a valid `name`, `channelSubtypeConfig` and `source`. The emulator answers 200 with an `id`/`arn`; AWS answers 400 ValidationException, x-amzn-errortype: ValidationException, message 'Connect Instance does not exist or is not enabled to use Campaigns.'. Replay connectcampaigns-error-create-campaign-validation#task#1.
3. Cause 2: GET /v2/campaigns/<any-unknown-campaign-id> (DescribeCampaign). The emulator answers 404 with message 'No campaign with id <id> exists'; AWS answers 404 with 'CampaignId not found <id>.'. Repeat with DELETE /v2/campaigns/<same-unknown-id> (DeleteCampaign) — same AWS wording. Replay connectcampaigns-error-operations-on-missing-campaign#task#1.
4. Cause 3: POST /v2/campaigns-summary (ListCampaigns) with `nextToken` set to a short string that is not valid base64 (e.g. one containing '-'). The emulator answers 400 with message 'nextToken <token> is not a valid page token'; AWS answers 400 ValidationException with 'Illegal base64 character <hh>' naming the first illegal character's hex code. Replay connectcampaigns-error-validation-boundaries#task#2.
5. Re-replay all three fixture#step bindings above; all three must pass on status, x-amzn-errortype and $.message.
