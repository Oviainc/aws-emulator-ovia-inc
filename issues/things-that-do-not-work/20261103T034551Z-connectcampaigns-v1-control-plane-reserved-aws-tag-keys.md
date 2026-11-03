Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaigns.create-campaign-reserved-tag-key-rejected connectcampaigns.create-campaign-dialer-bandwidth-range-not-enforced connectcampaigns.create-campaign-invalid-connect-instance-id-rejected connectcampaigns.delete-connect-instance-config-active-campaign-rejected connectcampaigns.delete-connect-instance-config-active-campaign-conflict-rejected connectcampaigns.start-instance-onboarding-job-conflict-when-already-onboarded-rejected
Issue-Shape-Id: com.amazonaws.connectcampaigns#AmazonConnectCampaignService
Issue-Key: connectcampaigns.create-campaign-reserved-tag-key-rejected|_error_code_and_message#4|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: connectcampaigns|CreateCampaign|ValidationException
Issue-Key: connectcampaigns.create-campaign-invalid-connect-instance-id-rejected|_error_code_and_message#16|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: connectcampaigns.delete-connect-instance-config-active-campaign-rejected|run#8|AssertionError|assert code == "InvalidStateException", f"expected InvalidStateException, got {code}"
Issue-Key: connectcampaigns.delete-connect-instance-config-active-campaign-conflict-rejected|run#5|AssertionError|assert code == "InvalidStateException", code
Issue-Key: connectcampaigns.start-instance-onboarding-job-conflict-when-already-onboarded-rejected|run#7|AssertionError|assert message == (
Issue-Commit: 70ed2d2e7473e9995477fa4e995826b18e438b3d

# ConnectCampaigns (v1) control plane: reserved aws: tag keys accepted, bandwidthAllocation over-enforced, ListCampaigns never checks onboarding, DeleteConnectInstanceConfig is a 501 stub, wrong already-onboarded message and wrong Content-Type

> _Note: this issue was identified against the code base at commit `70ed2d2e7473e9995477fa4e995826b18e438b3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCampaign

## Description
Six net-new root causes in the ConnectCampaigns v1 crate (services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice). CONTEXT: the fixed issue 20261029T172833Z-connectcampaigns-v1-validation-and-not-found-refusals-diverge.md covers v1 constraint-message/not-found wording and CreateCampaign's instance-registry gate (that gate now works — the CreateCampaign half of create-campaign-invalid-connect-instance-id-rejected passes); none of the causes below are described by it. The open 20261029T183744Z-connect-campaigns-connectcampaignsv2-responses-carry-the-wrong-content-type-on.md covers the V2 crate only. Checks the suite marked ALREADY FILED and the one OWNED by 20261022T145747Z are deliberately not re-filed.

=== Cause 1 of 6: CreateCampaign accepts and stores tag keys in the reserved aws: namespace ===
AWS: Smithy: CreateCampaign.tags -> TagMap, key TagKey @length 1..128 with pattern `^(?!aws:)[a-zA-Z+-=._:/]+$` — the reserved prefix is excluded by the model itself. Recorded behavior for a key that starts with "aws:": HTTP 400, code `BadRequestException`, message "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)" — a FIXED sentence with no variable part — and no campaign is created.
Emulator: the request SUCCEEDS with HTTP 200, mints id/arn and echoes `tags: {"aws:reserved": "nope"}`; the handler stores input.tags unvalidated (ops/create_campaign.rs:259) and no tag-key rule runs anywhere in front of it.
Why: a caller can write into the AWS-reserved tag namespace, which AWS forbids outright; tag-governance code and every negative test of the reserved prefix pass against the emulator and fail against AWS. The sibling V2 crate was already corrected for exactly this class (fixed 20261024T082644Z-connectcampaignsv2-tagresource-validation.md), so v1 is now the outlier.
Fix: CreateCampaign's input validation (the tag-key rule belongs in the crate's one tag-key check, shared with TagResource once that stub lands), refusing with BadRequestException and AWS's sentence.
Traces: connectcampaigns.create-campaign-reserved-tag-key-rejected.

=== Cause 2 of 6: CreateCampaign enforces the modeled [0,1] range on progressiveDialerConfig.bandwidthAllocation, which the real service does not enforce ===
AWS: Smithy: ProgressiveDialerConfig.bandwidthAllocation -> BandwidthAllocation, double, @required, @range min 0 max 1. Recorded behavior contradicts the model for this member: a CreateCampaign carrying bandwidthAllocation = 1.5 is ACCEPTED (HTTP 200, id/arn returned) and DescribeCampaign echoes dialerConfig.progressiveDialerConfig.bandwidthAllocation back as 1.5, unclamped; an in-range 0.5 is accepted and echoed identically. The model's @range is a documentation defect for this member, the same situation already recorded for GetCampaignStateBatch.campaignIds (modeled 25, real 500) in 20261029T172833Z.
Emulator: the centralized validate step refuses the request with HTTP 400, code `ValidationException`, message "1 validation error detected: Value at 'dialerConfig.progressiveDialerConfig.bandwidthAllocation' failed to satisfy constraint: Member must have value less than or equal to 1", so the campaign is never created.
Why: a legitimate request AWS accepts is impossible against the emulator — any caller using a progressive dialer with an allocation above 1 cannot create a campaign at all, and a client cannot discover the real (unenforced) bound.
Fix: the input-constraint override in front of CreateCampaign for that member (drop the range check and keep the stored value verbatim so DescribeCampaign echoes it unclamped), not the handler body.
Traces: connectcampaigns.create-campaign-dialer-bandwidth-range-not-enforced.

=== Cause 3 of 6: ListCampaigns with an instanceIdFilter naming an instance that is not onboarded returns an empty page instead of refusing ===
AWS: Smithy: ListCampaigns (POST /campaigns-summary) models no per-operation errors, so the service-common ValidationException (HTTP 400) is the refusal channel. Recorded behavior: filtering by `filters={'instanceIdFilter': {'value': <a UUID-shaped instance that was never onboarded>, 'operator': 'Eq'}}` is refused with HTTP 400, code `ValidationException`, message "Instance is not onboarded" — a FIXED sentence with no variable part. This is the same registry precondition CreateCampaign enforces (where AWS words it "Connect Instance does not exist or is not enabled to use Campaigns."); each operation keeps its own recorded sentence.
Emulator: answers HTTP 200 with `{'campaignSummaryList': []}`. The filter is applied as a plain predicate over the campaign store (ops/list_campaigns.rs:179-190) and the Connect/onboarding registry — which CreateCampaign already consults via crate::state::connect_instance_refusal (ops/create_campaign.rs:191-209) — is never asked.
Why: a client cannot distinguish "that instance has no campaigns" from "that instance is not onboarded at all": a typo'd or un-onboarded instance id reads as a healthy empty result, so callers silently poll an instance that will never report anything.
Fix: ListCampaigns' validation — consult the shared Connect-instance/onboarding registry for the instanceIdFilter value before filtering, refusing with ValidationException "Instance is not onboarded".
Traces: connectcampaigns.create-campaign-invalid-connect-instance-id-rejected (the CreateCampaign half of this check now passes; it fails on this ListCampaigns assertion).

=== Cause 4 of 6: DeleteConnectInstanceConfig is still an unimplemented 501 stub, so the instance config can never be deleted and its in-use refusal is unreachable ===
AWS: Smithy: DeleteConnectInstanceConfig (DELETE /connect-instance/{connectInstanceId}/config), output Unit, models InvalidStateException (HTTP 409), ResourceNotFoundException (HTTP 404), ThrottlingException. Recorded behavior: while the instance still owns at least one campaign (in ANY state, including Running) the call is refused with HTTP 409, code `InvalidStateException`, message "This instance still has campaigns" — a FIXED sentence with no variable part — and the config stays readable; once every campaign is deleted the call succeeds (HTTP 200, empty body) and GetConnectInstanceConfig then answers HTTP 404 ResourceNotFoundException "ConnectInstanceConfig not found <connectInstanceId>." (fixed text around the instance id taken from the request).
Emulator: answers HTTP 501, code `NotImplemented`, "operation DeleteConnectInstanceConfig is scaffolded but not implemented in this emulator" (ops/delete_connect_instance_config.rs:184 returns Scaffolded::not_implemented). The suite did not flag this one as an already-filed stub and no open issue names it.
Why: the onboarding lifecycle has no exit — an onboarded instance's config can never be removed, so teardown of any fixture leaks and the in-use guard that protects live campaigns is never exercised.
Fix: implement the DeleteConnectInstanceConfig handler against the shared campaign/onboarding store: refuse with InvalidStateException while campaigns remain for the instance, otherwise drop the onboarding record (and audit the change like its sibling instance mutators).
Traces: connectcampaigns.delete-connect-instance-config-active-campaign-rejected, connectcampaigns.delete-connect-instance-config-active-campaign-conflict-rejected.

=== Cause 5 of 6: StartInstanceOnboardingJob's already-onboarded ConflictException carries the model's doc sentence instead of AWS's message ===
AWS: Smithy: StartInstanceOnboardingJob models ConflictException (HTTP 409). Recorded behavior when the named instance is already successfully onboarded: HTTP 409, code `ConflictException` (the emulator's code is correct), message "Instance <connectInstanceId> under account <accountId> is already onboarded." — fixed text "Instance ", " under account ", " is already onboarded."; variable parts are the instance id taken from the request and the account id of the caller. The existing onboarding job/config is left untouched (still SUCCEEDED).
Emulator: returns the generic Smithy documentation string for the error shape — "The request could not be processed because of conflict in the current state of the resource." — hard-coded as state.rs:340-341 INSTANCE_ALREADY_ONBOARDED_MESSAGE and documented there as "a FIXED service string carrying nothing from the request", which is precisely what AWS does not return here.
Why: the message is part of the contract; a caller or support tool that reads this refusal cannot tell WHICH instance or account the conflict is about, and text that matches AWS's sentence never matches.
Fix: the already-onboarded refusal wording shared by the onboarding handlers (state.rs:332-341 and its use in start_instance_onboarding_job.rs) — compose AWS's sentence from the request's instance id and the emulator account id.
Traces: connectcampaigns.start-instance-onboarding-job-conflict-when-already-onboarded-rejected.

=== Cause 6 of 6: every v1 reply carries Content-Type application/x-amz-json-1.1, but this service is restJson1 ===
AWS: Smithy: the service's only protocol trait is aws.protocols#restJson1 (digest: protocols [restJson1]), whose responses carry Content-Type `application/json`; a Unit-output operation answers with no body and no JSON content type at all.
Emulator: the response headers recorded on these checks' replies are `content-type: application/x-amz-json-1.1` — the AWS JSON 1.1 RPC media type — on both a 200 with a body (CreateCampaign) and a 200 with content-length 0 (StartCampaign).
Why: the media type announces the wrong protocol for the service. A client or proxy that dispatches on Content-Type (or any strict parser) is told this REST-JSON service speaks AWS JSON 1.1. The open V2 issue 20261029T183744Z names the same divergence for the connectcampaignsv2 crate and carries only V2 checks, so the v1 replies are not covered by it; one shared serializer fix may close both.
Fix: the restJson1 response serializer's Content-Type for this service (the shared serialize step of the deserialize -> validate -> handle -> serialize walk), not any handler.
Traces: observed in the recorded replies of connectcampaigns.create-campaign-reserved-tag-key-rejected (200, content-type application/x-amz-json-1.1, body with id/arn/tags) and connectcampaigns.start-campaign-after-stop-rejected (200, content-length 0, same content-type).

## Evidence
Cause 1 — CreateCampaign(name=<unique>, connectInstanceId=<onboarded>, dialerConfig={'agentlessDialerConfig':{'dialingCapacity':1.0}}, outboundCallConfig={...}, tags={'aws:reserved':'nope'}):
  Expected (AWS): HTTP 400, code BadRequestException, message "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)", no campaign created.
  Actual (emulator): HTTP 200 — {'id': '15efd0bc88261ac6df6fa5da7317de0d', 'arn': 'arn:aws:connect-campaigns:us-west-2:000000000000:campaign/15efd0bc88261ac6df6fa5da7317de0d', 'tags': {'aws:reserved': 'nope'}} (the id/arn/account are per-run; the acceptance is the defect).
  Replay: connectcampaigns.create-campaign-reserved-tag-key-rejected [mismatch] — "expected a rejection, but the call succeeded".

Cause 2 — CreateCampaign with dialerConfig={'progressiveDialerConfig':{'bandwidthAllocation':1.5}}:
  Expected (AWS): HTTP 200 with an id/arn, and DescribeCampaign echoing bandwidthAllocation == 1.5; the same request with 0.5 behaves identically.
  Actual (emulator): ValidationException "1 validation error detected: Value at 'dialerConfig.progressiveDialerConfig.bandwidthAllocation' failed to satisfy constraint: Member must have value less than or equal to 1".
  Replay: connectcampaigns.create-campaign-dialer-bandwidth-range-not-enforced [error].

Cause 3 — ListCampaigns(filters={'instanceIdFilter': {'value': <never-onboarded UUID>, 'operator': 'Eq'}}):
  Expected (AWS): HTTP 400, code ValidationException, message "Instance is not onboarded".
  Actual (emulator): HTTP 200 — {'campaignSummaryList': []} (ResponseMetadata HTTPStatusCode 200).
  Replay: connectcampaigns.create-campaign-invalid-connect-instance-id-rejected [mismatch] — AssertionError "expected a rejection, but the call succeeded and returned {... 'campaignSummaryList': []}".

Cause 4 — DeleteConnectInstanceConfig(connectInstanceId=<onboarded instance that still owns a campaign>):
  Expected (AWS): HTTP 409, code InvalidStateException, message "This instance still has campaigns"; config still readable; after DeleteCampaign the same call succeeds and GetConnectInstanceConfig then 404s with "ConnectInstanceConfig not found <instanceId>.".
  Actual (emulator): HTTP 501, code NotImplemented — "operation DeleteConnectInstanceConfig is scaffolded but not implemented in this emulator"; source delete_connect_instance_config.rs:184.
  Replay: connectcampaigns.delete-connect-instance-config-active-campaign-rejected [mismatch] ("expected InvalidStateException, got NotImplemented"), connectcampaigns.delete-connect-instance-config-active-campaign-conflict-rejected [mismatch] ("AssertionError: NotImplemented").

Cause 5 — StartInstanceOnboardingJob(connectInstanceId=<already onboarded>, encryptionConfig={'enabled': False}):
  Expected (AWS): HTTP 409, code ConflictException, message "Instance <instanceId> under account <accountId> is already onboarded."
  Actual (emulator): code ConflictException (correct) with message "The request could not be processed because of conflict in the current state of the resource." (state.rs:340).
  Replay: connectcampaigns.start-instance-onboarding-job-conflict-when-already-onboarded-rejected [mismatch] — AssertionError at `assert message == (...)`.

Cause 6 — any v1 reply:
  Expected (AWS, restJson1): Content-Type `application/json` on a JSON body; no JSON content type on an empty Unit response.
  Actual (emulator): 'content-type': 'application/x-amz-json-1.1' on both {'content-length': '171'} (CreateCampaign 200) and {'content-length': '0'} (StartCampaign 200), as captured in the two checks' recorded responses.

## Steps to Reproduce
1. Boot the emulator from this branch and point a botocore/boto3 `connectcampaigns` client at it; create a Connect instance and onboard it (campaigns.start_instance_onboarding_job, poll get_instance_onboarding_job_status until SUCCEEDED).
2. Cause 1: `client.create_campaign(name='x-1', connectInstanceId=<onboarded>, dialerConfig={'agentlessDialerConfig':{'dialingCapacity':1.0}}, outboundCallConfig={'connectContactFlowId':<flow>,'connectSourcePhoneNumber':<number>}, tags={'aws:reserved':'nope'})`. Observe HTTP 200 with the aws:-prefixed tag echoed back, instead of BadRequestException "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)".
3. Cause 2: same call with `dialerConfig={'progressiveDialerConfig':{'bandwidthAllocation':1.5}}`. Observe ValidationException naming the <=1 bound, instead of a 200 whose DescribeCampaign echoes 1.5.
4. Cause 3: `client.list_campaigns(filters={'instanceIdFilter': {'value': str(uuid.uuid4()), 'operator': 'Eq'}})`. Observe HTTP 200 with an empty campaignSummaryList instead of ValidationException "Instance is not onboarded".
5. Cause 4: with one campaign existing for the onboarded instance, `client.delete_connect_instance_config(connectInstanceId=<onboarded>)`. Observe HTTP 501 NotImplemented instead of InvalidStateException "This instance still has campaigns"; delete the campaign and retry — the call must then succeed.
6. Cause 5: `client.start_instance_onboarding_job(connectInstanceId=<already onboarded>, encryptionConfig={'enabled': False})`. Observe ConflictException with the generic doc sentence instead of "Instance <id> under account <account> is already onboarded."
7. Cause 6: inspect the response headers of any of the calls above (e.g. `resp['ResponseMetadata']['HTTPHeaders']['content-type']`). Observe application/x-amz-json-1.1 instead of application/json.
8. Re-replay the 6 checks in trace_ids to confirm the fix.
