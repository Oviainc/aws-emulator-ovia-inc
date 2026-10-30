Issue-Source: trace-sweep-divergence
Issue-Trace: config-configuration-recorder-and-delivery-channel-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: config-configuration-recorder-and-delivery-channel-lifecycle
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config|DescribeDeliveryChannelStatus|4xx->2xx|$.__type
Issue-Key: config|DescribeDeliveryChannelStatus|4xx->2xx|$.message
Issue-Key: config|DescribeDeliveryChannelStatus|4xx->2xx|status
Issue-Commit: fdbc3a0ffd43a28553464e8afd6021169d0df514

# Config DescribeDeliveryChannelStatus answers 200 for an absent named channel instead of 400 NoSuchDeliveryChannelException

> _Note: this issue was identified against the code base at commit `fdbc3a0ffd43a28553464e8afd6021169d0df514`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDeliveryChannelStatus

## Description
ONE CAUSE must be closed.

Cause 1 of 1: DescribeDeliveryChannelStatus does not raise NoSuchDeliveryChannelException when the request explicitly names a delivery channel that does not exist (fixture config-configuration-recorder-and-delivery-channel-lifecycle, step task#7).

AWS: DescribeDeliveryChannelStatus takes an optional DeliveryChannelNames list (target DeliveryChannelNameList) and models exactly one error, NoSuchDeliveryChannelException, client kind, httpError 400 (defaulted) — documented as "You have specified a delivery channel that does not exist." (Smithy: DescribeDeliveryChannelStatus.input DeliveryChannelNames, DescribeDeliveryChannelStatus.errors, NoSuchDeliveryChannelException). The recorded response confirms it: when DeliveryChannelNames names a channel that is not present, AWS answers HTTP 400 with __type NoSuchDeliveryChannelException and message "Cannot find delivery channel with specified name '<name>'." The fixed text of that message is "Cannot find delivery channel with specified name '" ... "'." — the only variable part is the channel name, taken verbatim from the request's DeliveryChannelNames entry (the recording's 'sim2real-probe-config-channel-e8ffebde' is one run's value filling that slot, not part of the requirement).

Emulator: answers HTTP 200 and emits neither __type nor message (the replay reports both missing and the status as 200 where 400 was expected), i.e. it treats a named-but-absent channel as an empty DeliveryChannelsStatus result set instead of an error.

Why: the standard AWS idiom for probing delivery-channel existence is to call DescribeDeliveryChannelStatus with the name and catch NoSuchDeliveryChannelException. Against the emulator that probe always succeeds, so a client cannot distinguish "channel absent" from "channel present with no status to report"; teardown/reconciliation logic that depends on the 400 after a delete silently proceeds, and the emulator and AWS disagree about whether the resource exists.

Fix: DescribeDeliveryChannelStatus's handler/state lookup — when the request names channels, look each name up in the delivery-channel state and raise the modeled 400 NoSuchDeliveryChannelException (with the message template above) for any name not present, instead of filtering unknown names out of the response. This is the same missing existence check the open issue 20261022T170950Z-config-error-paths-unenforced-delivery-channel-validation-and.md reports for PutDeliveryChannel / DescribeDeliveryChannels / DeleteDeliveryChannel, but that issue does not cover DescribeDeliveryChannelStatus or carry this step's replay key, so this operation remains unfixed and unexercised under it.

Traces: config-configuration-recorder-and-delivery-channel-lifecycle#task#7 (DescribeDeliveryChannelStatus).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false).

Cause 1 — DescribeDeliveryChannelStatus, config-configuration-recorder-and-delivery-channel-lifecycle#task#7:
  status: expected 400, got 200
  $.__type: missing (expected 'NoSuchDeliveryChannelException', got None)
  $.message: missing (expected "Cannot find delivery channel with specified name 'sim2real-probe-config-channel-e8ffebde'.", got None)
  divergence_key: service=config|DescribeDeliveryChannelStatus|status=400->200|keys=$.__type,$.message,status
  verdict: mismatch; affected_count 1; no stub_path; owned_by empty (no open issue carries this replay key).

Message anatomy: fixed text "Cannot find delivery channel with specified name '" + <name> + "'."; <name> is the requested DeliveryChannelNames entry, not a recorded constant.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): DescribeDeliveryChannelStatus errors = [NoSuchDeliveryChannelException] (client, httpError 400, defaulted); NoSuchDeliveryChannelException doc "You have specified a delivery channel that does not exist."

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: fixture config-configuration-recorder-and-delivery-channel-lifecycle, step task#7 — after the delivery channel has been deleted/was never created, call DescribeDeliveryChannelStatus with DeliveryChannelNames=['<channel-name>']; the emulator returns HTTP 200 with no __type/message instead of HTTP 400 NoSuchDeliveryChannelException with message "Cannot find delivery channel with specified name '<channel-name>'.".
4. Re-replay config-configuration-recorder-and-delivery-channel-lifecycle#task#7 to confirm the fix.
