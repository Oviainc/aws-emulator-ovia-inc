Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaigns.create-campaign-name-too-long-rejected connectcampaigns.list-campaigns-max-results-out-of-range-rejected connectcampaigns.describe-nonexistent-campaign-not-found connectcampaigns.get-campaign-state-batch-too-many-ids-rejected
Issue-Shape-Id: com.amazonaws.connectcampaigns#AmazonConnectCampaignService
Issue-Key: connectcampaigns.create-campaign-name-too-long-rejected|run#9|AssertionError|assert message == (
Issue-Key: connectcampaigns.list-campaigns-max-results-out-of-range-rejected|run#8|AssertionError|assert message == (
Issue-Key: connectcampaigns.describe-nonexistent-campaign-not-found|run#5|AssertionError|assert message == f"CampaignId not found {bogus_id}.", message
Issue-Key: connectcampaigns|GetCampaignStateBatch|ValidationException
Issue-Commit: 0711e0a570adac04732f4f058a2849d70bc38321

# ConnectCampaigns (v1) validation and not-found refusals diverge: constraint messages echo the rejected value, DescribeCampaign's 404 text is hand-written, campaignIds is capped at the modeled 25 instead of the real 500, and CreateCampaign never checks the Connect instance

> _Note: this issue was identified against the code base at commit `0711e0a570adac04732f4f058a2849d70bc38321`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCampaign

## Description
Four net-new root causes in the ConnectCampaigns v1 crate (services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice). CONTEXT: the open issue 20261022T145747Z-connectcampaigns-is-entirely-unreachable-router-resolves-services-by.md lists these same check ids, but its replay keys are the earlier, coarser assertions (`assert code == ...`) that the router refusal tripped; requests now reach the v1 handlers and the ERROR CODES are correct, so what follows are the deeper divergences that issue never described and whose keys it does not carry. The check connectcampaigns.list-campaigns-max-results-exceeds-limit-is-accepted is OWNED by that issue and is deliberately not re-filed here.

=== Cause 1 of 4: the shared ValidationException constraint message echoes the rejected value, where AWS omits it for these scalar members ===
AWS: Smithy: CreateCampaign.name -> CampaignName, @required, @length min 1 max 127; ListCampaigns.maxResults -> MaxResults, @range min 1 max 50; both operations model ValidationException (HTTP 400). Recorded behavior for the message: AWS answers HTTP 400 / code `ValidationException` with the sentence "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must have length less than or equal to <bound>" for a length violation and "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must have value greater than or equal to <bound>" for a range violation. Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must have length/value <relation> ". Variable parts: the member path taken from the request (`name`, `maxResults`) and the bound taken from the constraint. The rejected value itself is NOT part of the message for these members.
Emulator: the centralized validate step renders the value inline — "1 validation error detected: Value 'xxxx…(the whole 128-character name)' at 'name' failed to satisfy constraint: Member must have length less than or equal to 127" and "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1". Status and error code are correct; only the message diverges.
Why: the error message is part of the contract. A client (or a test, log scraper, or support tool) that matches AWS's sentence for a constraint violation does not match the emulator's, so a rejection that is correct in code reads as an unknown failure; and echoing an arbitrarily long caller-supplied value into an error message is a shape AWS does not produce for these members.
Fix: the shared constraint-violation message renderer used by the centralized deserialize -> validate -> handle walk (smithy_server_runtime request validation), not any ConnectCampaigns handler — the same renderer produces both sentences.
Traces: connectcampaigns.create-campaign-name-too-long-rejected, connectcampaigns.list-campaigns-max-results-out-of-range-rejected.

=== Cause 2 of 4: DescribeCampaign's ResourceNotFoundException message is hand-written and does not match AWS's wording ===
AWS: Smithy: DescribeCampaign (GET /campaigns/{id}) models ResourceNotFoundException (HTTP 404); CampaignId's pattern is `^[\S]*$`, so a UUID-shaped unknown id is a pure not-found lookup, not a shape rejection. Recorded behavior: HTTP 404, code `ResourceNotFoundException`, message "CampaignId not found <id>." — fixed text "CampaignId not found " followed by the id taken verbatim from the request path and a terminating ".".
Emulator: answers code `ResourceNotFoundException` (correct) with the message "No campaign with id <id> exists" — different fixed text, no trailing period.
Why: a client that surfaces or matches the service's not-found sentence gets text AWS never emits for this operation, and the emulator's own crates are inconsistent with the wording the sibling v2 crate was already corrected to.
Fix: the DescribeCampaign handler's not-found branch (its error construction), which should go through the crate's one not-found wording rather than a per-handler `format!`. Trace: services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/src/ops/describe_campaign.rs:155.
Traces: connectcampaigns.describe-nonexistent-campaign-not-found.

=== Cause 3 of 4: GetCampaignStateBatch enforces the modeled campaignIds max of 25, but the service's real cap is 500, and the oversize message renders a count instead of the list ===
AWS: Smithy: GetCampaignStateBatch.campaignIds -> CampaignIdList, @required, @length min 1 max 25; ValidationException (HTTP 400) is modeled. The model is wrong about the upper bound: recorded behavior is that a batch of up to 500 well-formed ids is PROCESSED structurally (each unknown id comes back in failedRequests, successfulRequests empty), and only 501 ids is refused with HTTP 400 / `ValidationException` and the message "1 validation error detected: Value '[<the campaignIds, comma-and-space separated>]' at 'campaignIds' failed to satisfy constraint: Member must have length less than or equal to 500". Fixed text: "1 validation error detected: Value '[", "]' at 'campaignIds' failed to satisfy constraint: Member must have length less than or equal to 500"; variable part: the submitted ids, rendered as a bracketed list.
Emulator: a 500-id batch is REFUSED before the handler with HTTP 400 / `ValidationException` "1 validation error detected: Value '500 items' at 'campaignIds' failed to satisfy constraint: Member must have length less than or equal to 25" — wrong bound (25 instead of 500) and the value rendered as "<n> items" instead of the bracketed id list.
Why: every caller batching more than 25 campaign ids — which real AWS accepts up to 500 — is refused, so a legitimate batch read is impossible against the emulator; and the refusal that IS correct at 501 ids carries the wrong limit and the wrong value rendering, so a client cannot learn the real cap from the message.
Fix: the campaignIds length rule for this operation — the input constraint/validation override in front of GetCampaignStateBatch must use the service's real 500 cap (the model's 25 is a documentation defect) and the shared list-constraint message must render the submitted list, not its count. Note the GetCampaignStateBatch handler itself is still an unimplemented scaffold stub (already filed separately for connectcampaigns.get-campaign-state-batch-empty-list-rejected); this cause is the validation layer in front of that stub, so confirming it needs the stub implemented too.
Traces: connectcampaigns.get-campaign-state-batch-too-many-ids-rejected.

=== Cause 4 of 4: CreateCampaign never consults the Amazon Connect instance registry, so a campaign is minted for an instance that does not exist or is not onboarded ===
AWS: CreateCampaign (PUT /campaigns) requires a Connect instance that exists and is onboarded for campaigns; recorded behavior for a UUID-shaped connectInstanceId that was never onboarded is HTTP 400 / code `ValidationException` with the message "Connect Instance does not exist or is not enabled to use Campaigns." — a fixed sentence with no variable part. Smithy: connectInstanceId -> InstanceId @required, @length 0..256, pattern `^[a-zA-Z0-9_\-.]*$`; ValidationException and ResourceNotFoundException are both modeled for the operation.
Emulator: the handler consults only its own campaign store (per-instance quota and store capacity) and then inserts the campaign; there is no onboarding/instance lookup at all, so any pattern-valid connectInstanceId is accepted and a campaign AWS would never create is minted and subsequently listed by ListCampaigns.
Why: the emulator is over-permissive on the operation's central precondition — a client gets an id and ARN for a campaign attached to a nonexistent instance, and every negative test that relies on the not-onboarded rejection (including the positive control that proves a name-length rejection was the name and not the instance) passes for the wrong reason.
Fix: the CreateCampaign handler's validation, which must consult the shared onboarding/instance registry before minting. Trace: services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/src/ops/create_campaign.rs:185-235 (store/quota checks only, then insert_campaign). Grounded in the second half of connectcampaigns.create-campaign-name-too-long-rejected, which asserts exactly this control rejection; the check currently stops at the Cause 1 message mismatch before reaching it.
Traces: connectcampaigns.create-campaign-name-too-long-rejected.

## Evidence
Cause 1 — CreateCampaign, name of 128 'x' characters:
  Expected (AWS): HTTP 400, code ValidationException, message "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must have length less than or equal to 127".
  Actual (emulator): code ValidationException, message "1 validation error detected: Value 'xxxxxxxx…<the full 128-character name>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 127".
  Replay: connectcampaigns.create-campaign-name-too-long-rejected [mismatch] — AssertionError at `assert message == (...)`.

Cause 1 — ListCampaigns with maxResults=0 (client-side validation disabled so the request reaches the wire):
  Expected (AWS): HTTP 400, code ValidationException, message "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1".
  Actual (emulator): code ValidationException, message "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1".
  Replay: connectcampaigns.list-campaigns-max-results-out-of-range-rejected [mismatch] — AssertionError at `assert message == (...)`.

Cause 2 — DescribeCampaign(id=<never-created UUID>):
  Expected (AWS): HTTP 404, code ResourceNotFoundException, message "CampaignId not found <id>."
  Actual (emulator): code ResourceNotFoundException, message "No campaign with id <id> exists" (observed id aebd292a-3edd-47be-87f3-bb50e98ae440 in that run; the id is the request's, only the surrounding text is the requirement).
  Source: describe_campaign.rs:155 `message: format!("No campaign with id {} exists", input.id)`.
  Replay: connectcampaigns.describe-nonexistent-campaign-not-found [mismatch].

Cause 3 — GetCampaignStateBatch(campaignIds=[500 nonexistent ids]):
  Expected (AWS): HTTP 200, successfulRequests == [], 500 entries in failedRequests. Only at 501 ids: HTTP 400, ValidationException, "1 validation error detected: Value '[<ids>]' at 'campaignIds' failed to satisfy constraint: Member must have length less than or equal to 500".
  Actual (emulator): HTTP 400 already at 500 ids — ClientError (ValidationException): "1 validation error detected: Value '500 items' at 'campaignIds' failed to satisfy constraint: Member must have length less than or equal to 25".
  Replay: connectcampaigns.get-campaign-state-batch-too-many-ids-rejected [error].

Cause 4 — CreateCampaign with a valid-length name and a UUID-shaped, never-onboarded connectInstanceId:
  Expected (AWS): HTTP 400, code ValidationException, message "Connect Instance does not exist or is not enabled to use Campaigns.", and no campaign visible from ListCampaigns.
  Actual (emulator): the create succeeds — create_campaign.rs performs only the per-instance quota and store-capacity checks and then inserts the campaign, with no instance/onboarding lookup, so the campaign is minted and returned with an id and ARN.
  Replay: connectcampaigns.create-campaign-name-too-long-rejected (control half; the check currently fails earlier on the Cause 1 message).

## Steps to Reproduce
1. Boot the emulator from mainline (both connectcampaigns and connectcampaignsv2 crates loaded, the default build) and point a botocore/boto3 `connectcampaigns` client at it.
2. Cause 1a: `client.create_campaign(name='x'*128, connectInstanceId=str(uuid.uuid4()), dialerConfig={'agentlessDialerConfig':{'dialingCapacity':1.0}}, outboundCallConfig={'connectContactFlowId':'some-contact-flow-id'})`. Observe ValidationException whose message contains "Value 'xxxx…' at 'name'" instead of AWS's "Value at 'name'".
3. Cause 1b: with `Config(parameter_validation=False)`, `client.list_campaigns(maxResults=0)`. Observe "Value '0' at 'maxResults' …" instead of "Value at 'maxResults' …".
4. Cause 2: `client.describe_campaign(id=str(uuid.uuid4()))`. Observe ResourceNotFoundException with "No campaign with id <id> exists" instead of "CampaignId not found <id>.".
5. Cause 3: `client.get_campaign_state_batch(campaignIds=[f'nonexistent-{i}' for i in range(500)])`. Observe ValidationException naming the 25-item limit and the value as "500 items", instead of a 200 with 500 failedRequests (and, at 501 ids, a message naming the 500-item limit with the bracketed id list).
6. Cause 4: repeat step 2 with a valid-length name and the same UUID-shaped connectInstanceId. Observe the call SUCCEEDS and `client.list_campaigns(maxResults=50)` lists the new campaign, instead of ValidationException "Connect Instance does not exist or is not enabled to use Campaigns." and no campaign.
7. Re-replay the four checks in trace_ids to confirm a fix.
