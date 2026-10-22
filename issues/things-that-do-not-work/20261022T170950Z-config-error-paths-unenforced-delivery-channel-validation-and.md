Issue-Source: trace-sweep-divergence
Issue-Trace: config-configuration-recorder-and-delivery-channel-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: config-configuration-recorder-and-delivery-channel-lifecycle config-resource-config-and-evaluation-validation
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config|PutDeliveryChannel|4xx->2xx|$
Issue-Key: config|PutDeliveryChannel|4xx->2xx|status
Issue-Key: config|DescribeDeliveryChannels|4xx->2xx|$.__type
Issue-Key: config|DescribeDeliveryChannels|4xx->2xx|$.message
Issue-Key: config|DescribeDeliveryChannels|4xx->2xx|status
Issue-Key: config|DeleteDeliveryChannel|4xx->2xx|$
Issue-Key: config|DeleteDeliveryChannel|4xx->2xx|status
Issue-Key: config|DeletePendingAggregationRequest|4xx->4xx|$.message
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Config error paths unenforced: delivery-channel validation and absence answer 200 instead of 400

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeliveryChannel

## Description
Four distinct causes on the AWS Config error path, all in this service. The emulator accepts or silently succeeds where AWS rejects, and renders one validation message with a different constraint rendering. No existing open issue covers the config service wire surface. FOUR CAUSES must be closed.

Cause 1 of 4 — PutDeliveryChannel does not validate the delivery target (fixture config-configuration-recorder-and-delivery-channel-lifecycle, step task#5).
AWS: PutDeliveryChannel models InsufficientDeliveryPolicyException (client error, httpError 400, defaulted) alongside NoSuchBucketException, InvalidS3KeyPrefixException, InvalidSNSTopicARNException, InvalidS3KmsKeyArnException (Smithy: PutDeliveryChannel.errors). The recording shows that a PutDeliveryChannel whose s3BucketName does not exist and whose role cannot be assumed is rejected with HTTP 400 and __type InsufficientDeliveryPolicyException (recorded message, an example: "Insufficient delivery policy to s3 bucket: <bucket>, unable to assume role: <roleArn>."). The requirement is the 400 status and the InsufficientDeliveryPolicyException code, not the message text.
Emulator: answers HTTP 200 with an empty body and persists the delivery channel.
Why: a client that relies on AWS validating the delivery target believes Config delivery is configured when it is not; the misconfiguration is only discovered later (or never), and every downstream assertion about delivery/snapshot behaviour is made against a channel real AWS would have refused to create.
Fix: PutDeliveryChannel's handler validation — verify the S3 bucket and the recorder role before persisting, and raise the modeled 400 InsufficientDeliveryPolicyException (or NoSuchBucketException where the bucket itself is the fault) instead of persisting.

Cause 2 of 4 — DescribeDeliveryChannels does not raise NoSuchDeliveryChannelException for a named, absent channel (fixture config-configuration-recorder-and-delivery-channel-lifecycle, step task#6).
AWS: DescribeDeliveryChannels models exactly one error, NoSuchDeliveryChannelException (client, 400) — "You have specified a delivery channel that does not exist" (Smithy: DescribeDeliveryChannels.errors, NoSuchDeliveryChannelException). The recording shows HTTP 400 with __type NoSuchDeliveryChannelException when DeliveryChannelNames names a channel that does not exist (recorded message, an example: "Cannot find delivery channel with specified name '<name>'.").
Emulator: answers HTTP 200 and omits both __type and message (empty body), i.e. it treats a named-but-absent channel as an empty result set.
Why: existence checks never fail, so a client cannot distinguish "channel absent" from "channel present with nothing to report" — the standard AWS idiom of probing with a name and catching NoSuchDeliveryChannelException silently succeeds.
Fix: DescribeDeliveryChannels' handler/state lookup — when the request explicitly names channels, raise 400 NoSuchDeliveryChannelException for any name not in state rather than filtering it out.

Cause 3 of 4 — DeleteDeliveryChannel is wrongly idempotent (fixture config-configuration-recorder-and-delivery-channel-lifecycle, step task#9).
AWS: DeleteDeliveryChannel models NoSuchDeliveryChannelException (client, 400) and LastDeliveryChannelDeleteFailedException (Smithy: DeleteDeliveryChannel.errors). The recording shows that deleting a channel that is already gone answers HTTP 400 with __type NoSuchDeliveryChannelException (recorded message, an example: "Cannot find delivery channel with specified name '<name>'.").
Emulator: answers HTTP 200 with an empty body — the delete of a nonexistent channel succeeds.
Why: teardown and reconciliation code that depends on the second delete failing (to detect double-deletion or a lost channel) gets a success from the emulator and a 400 from AWS, so the two environments disagree on the resource's existence.
Fix: DeleteDeliveryChannel's handler — look the channel up in state first and raise 400 NoSuchDeliveryChannelException when it is absent.

Cause 4 of 4 — DeletePendingAggregationRequest renders the constraint pattern with anchors in the validation message (fixture config-resource-config-and-evaluation-validation, step task#8).
AWS: DeletePendingAggregationRequest.RequesterAccountId is required and targets AccountId with pattern ^\d{12}$ (Smithy: DeletePendingAggregationRequest input). A violating value is rejected with HTTP 400 and the standard AWS validation message, which is built from the request — it names the member and the constraint it failed — and renders the regular expression WITHOUT the anchors: "1 validation error detected: Value 'not-an-acct1' at 'requesterAccountId' failed to satisfy constraint: Member must satisfy regular expression pattern: \d{12}". Because the message's construction rule is known (member + constraint from the request), that rendering is the requirement, not merely a recorded example.
Emulator: same 400 and same error class, but renders the raw model pattern instead: "... Member must satisfy regular expression pattern: ^\d{12}$".
Why: clients and test suites that assert on the documented AWS validation-message rendering (or extract the pattern from it) see a different constraint string than AWS produces.
Fix: the shared request-validation message formatter used by this operation's validation — strip the leading ^ / trailing $ anchors when rendering a @pattern constraint into the validation message.

Traces: config-configuration-recorder-and-delivery-channel-lifecycle#task#5 (PutDeliveryChannel), config-configuration-recorder-and-delivery-channel-lifecycle#task#6 (DescribeDeliveryChannels), config-configuration-recorder-and-delivery-channel-lifecycle#task#9 (DeleteDeliveryChannel), config-resource-config-and-evaluation-validation#task#8 (DeletePendingAggregationRequest).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, state_reset_failures=0).

Cause 1 — PutDeliveryChannel, config-configuration-recorder-and-delivery-channel-lifecycle#task#5:
  expected status 400, actual 200
  expected body {'__type': 'InsufficientDeliveryPolicyException', 'message': 'Insufficient delivery policy to s3 bucket: sim2real-probe-config-no-such-bucket-e8ffebde, unable to assume role: arn:aws:iam::000000000000:role/sim2real-probe-config-unused-role-e8ffebde.'}
  actual body {} (empty_body)

Cause 2 — DescribeDeliveryChannels, config-configuration-recorder-and-delivery-channel-lifecycle#task#6:
  expected status 400, actual 200
  $.__type: missing (expected 'NoSuchDeliveryChannelException', got None)
  $.message: missing (expected "Cannot find delivery channel with specified name 'sim2real-probe-config-channel-e8ffebde'.", got None)

Cause 3 — DeleteDeliveryChannel, config-configuration-recorder-and-delivery-channel-lifecycle#task#9:
  expected status 400, actual 200
  expected body {'__type': 'NoSuchDeliveryChannelException', 'message': "Cannot find delivery channel with specified name 'sim2real-probe-config-channel-e8ffebde'."}
  actual body {} (empty_body)

Cause 4 — DeletePendingAggregationRequest, config-resource-config-and-evaluation-validation#task#8 (status 400 on both sides):
  $.message: value_mismatch
    expected "1 validation error detected: Value 'not-an-acct1' at 'requesterAccountId' failed to satisfy constraint: Member must satisfy regular expression pattern: \d{12}"
    got      "1 validation error detected: Value 'not-an-acct1' at 'requesterAccountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\d{12}$"

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): PutDeliveryChannel errors include InsufficientDeliveryPolicyException/NoSuchBucketException (400); DescribeDeliveryChannels and DeleteDeliveryChannel both model NoSuchDeliveryChannelException (400); DeletePendingAggregationRequest.RequesterAccountId pattern ^\d{12}$ with InvalidParameterValueException (400).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: fixture config-configuration-recorder-and-delivery-channel-lifecycle, step task#5 — PutDeliveryChannel with an s3BucketName that does not exist and a role that cannot be assumed; emulator returns 200 instead of 400 InsufficientDeliveryPolicyException.
4. Cause 2: same fixture, step task#6 — DescribeDeliveryChannels naming a channel that does not exist; emulator returns 200 empty instead of 400 NoSuchDeliveryChannelException.
5. Cause 3: same fixture, step task#9 — DeleteDeliveryChannel for an already-absent channel; emulator returns 200 instead of 400 NoSuchDeliveryChannelException.
6. Cause 4: fixture config-resource-config-and-evaluation-validation, step task#8 — DeletePendingAggregationRequest with RequesterAccountId='not-an-acct1'; status/class match but the validation message renders the pattern as ^\d{12}$ instead of \d{12}.
7. Re-replay all four fixture#step bindings above to confirm the fix.
