Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaignsv2.concurrent-create-campaign-same-name-one-winner
Issue-Shape-Id: com.amazonaws.connectcampaignsv2#AmazonConnectCampaignServiceV2
Issue-Key: connectcampaignsv2.concurrent-create-campaign-same-name-one-winner|run#17|AssertionError|assert message == expected, f"loser ConflictException message {message!r}, expected {expected!r}"
Issue-Commit: c549793690d362456582989e0f126b756da14a88

# CreateCampaign's duplicate-name ConflictException message is the emulator's own wording instead of AWS's fixed conflict literal

> _Note: this issue was identified against the code base at commit `c549793690d362456582989e0f126b756da14a88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCampaign

## Description
ONE net-new root cause in the ConnectCampaignsV2 crate (service token `connectcampaignsv2`), found by replaying the behavioral suite simparity-behavioral-aws-v1 (9/25 passed). A fixer must close this ONE cause before submitting.

Not covered by any open ConnectCampaignsV2 issue. In particular, 20261029T232338Z-connectcampaignsv2-campaign-plane-wrong-state-conflict-error-4.md owns the campaign *state-gate* refusals (Pause/Resume/Stop/Start/PutOutboundRequestBatch answering InvalidCampaignStateException where AWS answers ConflictException, and the `Campaign: <id> is in invalid state: <STATE>` wording) plus nine other campaign-plane causes; it says nothing about CreateCampaign's per-instance NAME-uniqueness conflict, which is a different code path (the store's NameTaken rejection in the CreateCampaign handler) and whose error CODE is already correct here. 20261029T183744Z-connect-campaigns-connectcampaignsv2-responses-carry-the-wrong-content-type-on.md owns only the Content-Type header; 20261022T145747Z-connectcampaigns-is-entirely-unreachable-... owns the v1 router collision; 20261024T082644Z-...-deleteinstanceonboardingjob-... owns the onboarding-job delete edge.

=== Cause 1 of 1: CreateCampaign's name-already-exists refusal interpolates the campaign name and the Connect instance id into a message AWS emits as a fixed, variable-free literal ===

AWS: CreateCampaign enforces campaign-name uniqueness within a Connect instance. A create for a name the named `connectInstanceId` already holds is refused with **ConflictException**, HTTP **409**, and the message is the service's FIXED conflict literal with NO variable part and no interpolation of request values:

    Failed due to conflict in current state of the resource.

Every character of that message is fixed text (note the trailing period); there is no name, no instance id, no campaign id and no other value taken from the request or from state. Smithy (services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/smithy-model/com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2.json): CreateCampaign models `ConflictException` (errorKind client, httpError 409); `ConflictException` is a structure whose only required member is `message` (target String) and whose doc is "The request could not be processed because of conflict in the current state of the resource." — i.e. the model carries no per-resource template, and the recorded refusal is the generic literal above. CreateCampaign input: `name` @required (CampaignName, @length 1..127), `connectInstanceId` @required (InstanceId, @length 1..256, @pattern ^[-_.a-zA-Z0-9]+$). Under N concurrent identically-named creates against one instance, AWS admits exactly one winner and refuses the other N-1 this way.

Emulator: the HTTP status (409), the error code (`ConflictException`) and the single-winner/N-1-loser outcome are all CORRECT — the only divergence is the message. Instead of the fixed literal the emulator composes its own sentence out of request values:

    A campaign named <name> already exists in Connect instance <connectInstanceId>

(in the replay: `A campaign named sim2real-race-da27152d7597 already exists in Connect instance 4cd1aa0d-5ae8-497f-901e-e5a6892bcee8` — the generated campaign name and the generated instance uuid are this run's values and are not part of any requirement; the requirement is that NO such values appear at all). Fixed text differs (`A campaign named ` / ` already exists in Connect instance ` vs AWS's single literal sentence) and the emulator additionally omits AWS's trailing period.

Why: a client relying on AWS's behavior reads the wrong thing off the refusal. Message text is a requirement of the error class, so anything surfacing, logging, localizing or asserting on `Error.Message` for a duplicate-campaign-name conflict sees prose AWS never produces — SDK-level message assertions and conflict-handling tests pass against AWS and fail against the emulator (and conversely). The emulator's wording also leaks the caller's campaign name and the Connect instance id into an error body AWS keeps value-free, so a caller that treats the message as stable/non-sensitive is given information AWS deliberately does not return here. Because the code is right, the divergence is invisible to `except ConflictException` branches and only bites the message path, which is exactly where it is hardest to notice.

Fix: the duplicate-name refusal in the CreateCampaign handler's `InsertRejected::NameTaken` arm, and the message helper it calls in the service state module — the ConflictException it constructs must carry AWS's fixed conflict literal rather than a composed, value-interpolating sentence. If that helper is shared with other conflict paths, each caller must be checked against the message its own recordings show rather than unified blindly.

Traces: connectcampaignsv2.concurrent-create-campaign-same-name-one-winner.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator: 9/25 passed (36.0%), 15 mismatched + 1 errored, all under service heading `connectcampaignsv2`. Of those 16 failures, 14 are marked OWNED by 20261029T232338Z-connectcampaignsv2-campaign-plane-wrong-state-conflict-error-4.md and 1 (connectcampaignsv2.update-campaign-communication-limits-override-round-trip) is marked ALREADY FILED (blocked by unimplemented customerprofiles CreateSegmentDefinition); none of those is part of this issue. This issue is grounded in the single remaining, unclaimed failure.

Cause 1 — connectcampaignsv2.concurrent-create-campaign-same-name-one-winner [mismatch]:

  File "check_connectcampaignsv2_concurrent_create_campaign_same_name_one_winner.py", line 69, in run
    assert message == expected, f"loser ConflictException message {message!r}, expected {expected!r}"
  AssertionError: loser ConflictException message 'A campaign named sim2real-race-da27152d7597 already exists in Connect instance 4cd1aa0d-5ae8-497f-901e-e5a6892bcee8', expected 'Failed due to conflict in current state of the resource.'

What the check had already established BEFORE this assertion (these all passed, so they are the emulator behaving correctly and must not regress):
  - exactly 1 of the 5 concurrent CreateCampaign racers succeeded (len(successes) == 1);
  - the other 4 were refused (len(failures) == 4);
  - every one of the 4 refusals carried Error.Code == 'ConflictException' (so the error SHAPE and its HTTP 409 class are already right);
  - the sole failing assertion is on Error.Message.

Expected (AWS) message: the fixed literal `Failed due to conflict in current state of the resource.` — 100% fixed text, zero variable parts.
Actual (emulator) message: `A campaign named <name> already exists in Connect instance <connectInstanceId>` — fixed text `A campaign named ` + the request's `name` member + ` already exists in Connect instance ` + the request's `connectInstanceId` member, with no trailing period. The two interpolated values in the recorded line (`sim2real-race-da27152d7597`, `4cd1aa0d-5ae8-497f-901e-e5a6892bcee8`) are this run's generated name and instance uuid and are NOT part of the requirement.

Source of the divergence in the checkout: services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/ops/create_campaign.rs lines 376-385 — the `Err(crate::state::InsertRejected::NameTaken)` arm builds `ConflictException { message: crate::state::campaign_name_taken_message(&requested_name, &requested_instance), .. }`, i.e. the message is composed from the two request members rather than being the service's fixed conflict literal. The handler's doc comment (lines 12-14) confirms the intent that a name-collision answers the modelled ConflictException/409 — only the message text is wrong.

Model evidence: smithy_query(query="shape", shape="ConflictException") on this service's model returns {errorKind: client, httpError: 409, members: {message: {target: String, required: true}, xAmzErrorType: httpHeader x-amzn-ErrorType}, doc: "The request could not be processed because of conflict in the current state of the resource."}; smithy_query(query="operation", operation="CreateCampaign") lists ConflictException (client, 409) among its errors, with `name` and `connectInstanceId` both @required. The model prescribes no per-resource message template, and the recorded refusal is the fixed literal.

## Steps to Reproduce
1. Boot the emulator from mainline and point a botocore `connectcampaignsv2` client at its data listener.
2. Create an Amazon Connect instance and drive it to ACTIVE (connect.create_instance + describe_instance), then onboard it to Campaigns via connectcampaignsv2.start_instance_onboarding_job so CreateCampaign's instance gate passes. Create a disconnect contact flow and claim a phone number for the telephony defaultOutboundConfig.
3. Pick a single campaign name, e.g. name = 'sim2real-race-<random>'.
4. Fire 5 CreateCampaign calls CONCURRENTLY (ThreadPoolExecutor, max_workers=5) with the IDENTICAL `name`, the same `connectInstanceId`, and the same telephony `channelSubtypeConfig` ({'telephony': {'capacity': 1.0, 'outboundMode': {'agentless': {}}, 'defaultOutboundConfig': {'connectContactFlowId': <flow>, 'connectSourcePhoneNumber': <number>}}}).
5. Confirm the parts that already hold: exactly 1 call succeeds, 4 are refused, and each refusal has response['Error']['Code'] == 'ConflictException' with HTTP 409.
6. Inspect response['Error']['Message'] on each of the 4 losers. It MUST be exactly the fixed literal 'Failed due to conflict in current state of the resource.' (including the trailing period) with no campaign name, instance id or other request/state value interpolated. The emulator instead returns 'A campaign named <name> already exists in Connect instance <connectInstanceId>'.
7. The same refusal is reachable serially, without the race: CreateCampaign once with the name, then CreateCampaign again with the same name and same connectInstanceId. The second call must be refused 409 ConflictException with the same fixed literal.
8. Confirm the state effect is unaffected (it already passes): ListCampaigns(filters={'instanceIdFilter': {'operator': 'Eq', 'value': <instanceId>}}) shows exactly ONE campaign with that name.
9. Re-replay connectcampaignsv2.concurrent-create-campaign-same-name-one-winner; it must pass on error code, HTTP status, message text and the single-winner listing effect.
