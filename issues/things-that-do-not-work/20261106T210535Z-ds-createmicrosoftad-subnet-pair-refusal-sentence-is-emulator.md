Issue-Source: trace-sweep-divergence
Issue-Trace: ds-madl-traj-create-microsoft-ad-subnet-pick-mistakes
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ds-madl-traj-create-microsoft-ad-subnet-pick-mistakes
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds|CreateMicrosoftAD|4xx->4xx|$.Message
Issue-Commit: 0a6b2caf8aaebe9b3d91e5e0596565c0c4125f85

# ds CreateMicrosoftAD subnet-pair refusal sentence is emulator-authored, not AWS's InvalidParameterException wording

> _Note: this issue was identified against the code base at commit `0a6b2caf8aaebe9b3d91e5e0596565c0c4125f85`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrosoftAD

## Description
One cause.

Cause 1 of 1: CreateMicrosoftAD refuses a bad VpcSettings.SubnetIds pair with the right status and the right error code, but with a re-worded, emulator-authored message sentence, so the recorded message shape is not met.

AWS: CreateMicrosoftAD places a Managed Microsoft AD's two domain controllers in two subnets in two different Availability Zones, and refuses a VpcSettings.SubnetIds value that cannot satisfy that rule with 400 InvalidParameterException. Smithy (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json): CreateMicrosoftAD errors = ClientException, DirectoryLimitExceededException, InvalidParameterException (client, HTTP 400), ServiceException, UnsupportedOperationException; VpcSettings (DirectoryVpcSettings) is required; InvalidParameterException carries Message and RequestId; its doc text is only "One or more parameters are not valid.", so the model does not state the sentence. The sentence itself is recorded behavior: AWS words this refusal class with the FIXED text "Invalid subnet ID(s). They must correspond to two subnets in different Availability Zones. " followed by ": RequestId: <request id>" — the same fixed text the companion behavioral finding 20261106T203453Z-ds-ec2-ds-2-net-new-causes-describedirectories-vpcsettings-omits.md records for the single-subnet variant of this same guard. The only variable part of the message is the request id of that call (per-run, not a requirement). The wire fixture asserts the message by shape rather than literally ('<shaped-not-asserted>'), and the emulator's sentence fails that shape.

Emulator: for both subnet-pick mistakes in this fixture (task#7 and task#8, one cluster) it answers 400 with __type InvalidParameterException (correct) and Message "Invalid subnet ID(s). The two subnets must be in different Availability Zones. : RequestId: <request id>". The fixed text "They must correspond to two subnets in different Availability Zones." is replaced by the emulator's own "The two subnets must be in different Availability Zones.", and the same single sentence is emitted for every defect of the pair rather than AWS's wording for the class. (The request id in the recorded run was 00000000-0000-4000-8000-000000000001; that value is per-run and is NOT part of the finding.)

Why: a client that matches, logs, localizes or surfaces AWS's refusal sentence — SDK consumers keying on message text, runbooks, and tests that assert the boundary explanation — does not recognise the refusal although the status and error code are right. The emulator's sentence also mis-states the boundary: it asserts something about "the two subnets" even when the request's defect is that it does not name two usable subnets at all, so the message does not tell the caller what to change.

Fix: the CreateMicrosoftAD request validation's refusal message template — services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_microsoft_ad.rs line 213 calls crate::state::subnet_pair_defect(&input.vpc_settings.subnet_ids) and wraps its string in invalid_parameter, so the wording lives in that state-level helper. Emit AWS's fixed sentence for this refusal class there, keeping the request id as the only substituted value.

Traces: ds-madl-traj-create-microsoft-ad-subnet-pick-mistakes#task#7 (representative) and #task#8 — the two members of the one cluster. Note: this is NOT covered by the two open ds wire issues the replay names as owners of the other clusters (20261029T180805Z-ds-directory-service-response-fidelity-input-validation-skipped-or.md, 20261103T100504Z-ds-directory-service-certificate-and-directory-lookups-unvalidated-shortname.md), and the adjacent behavioral issue 20261106T203453Z-... owns only the SINGLE-subnet check and carries no wire replay key for these steps.

## Evidence
Replay cluster (service=ds | CreateMicrosoftAD | status=400->400 | keys=$.Message), verdict mismatch, affected_count 2:
  fixture ds-madl-traj-create-microsoft-ad-subnet-pick-mistakes, steps task#7 and task#8
  expected_status 400, actual_status 400 (status and $.__type InvalidParameterException agree)
  $.Message: value_mismatch
    expected: '<shaped-not-asserted>' (the fixture asserts the message's shape, not a literal string)
    actual:   'Invalid subnet ID(s). The two subnets must be in different Availability Zones. : RequestId: 00000000-0000-4000-8000-000000000001'

AWS wording for this refusal class (fixed text): 'Invalid subnet ID(s). They must correspond to two subnets in different Availability Zones. : RequestId: <request id>'.
  Fixed text: 'Invalid subnet ID(s). They must correspond to two subnets in different Availability Zones. ' and the ' : RequestId: ' separator.
  Variable text: the request id only, taken from the call being answered.

Smithy: CreateMicrosoftAD errors include InvalidParameterException (client, httpError 400); VpcSettings (DirectoryVpcSettings) required; InvalidParameterException members Message, RequestId. The model states no message text, so the sentence is recorded behavior.

Emulator source of the diverging sentence: services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_microsoft_ad.rs:209-215 — 'if let Some(defect) = crate::state::subnet_pair_defect(&input.vpc_settings.subnet_ids) { return Err(invalid_parameter(defect)); }'.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it.
2. Replay the wire fixture ds-madl-traj-create-microsoft-ad-subnet-pick-mistakes (replay_wire_corpus) and look at step task#7 (and task#8).
3. Equivalently, by hand: call ds CreateMicrosoftAD with a valid Name/Password/Edition and VpcSettings naming a subnet pair that cannot satisfy the two-subnets-in-two-different-Availability-Zones rule (e.g. the same subnet twice, or two subnets in one zone).
4. Observe HTTP 400 with __type InvalidParameterException (correct) and Message 'Invalid subnet ID(s). The two subnets must be in different Availability Zones. : RequestId: <id>'.
5. AWS answers the same status and code with the fixed sentence 'Invalid subnet ID(s). They must correspond to two subnets in different Availability Zones. ' before ': RequestId: <id>' — the fixture's message-shape assertion fails on the emulator's sentence.
