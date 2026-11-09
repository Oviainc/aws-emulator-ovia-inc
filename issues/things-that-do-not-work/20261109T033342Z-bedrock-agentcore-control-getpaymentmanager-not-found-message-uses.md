Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-not-found-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-not-found-error-paths bedrock-agentcore-control-payment-error-paths
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetPaymentManager|4xx->4xx|$.message
Issue-Commit: dd62406f8379c1f6ec445456da967cb3a28bbbbc

# bedrock-agentcore-control GetPaymentManager not-found message uses emulator wording instead of AWS's "Payment manager not found: <id>"

> _Note: this issue was identified against the code base at commit `dd62406f8379c1f6ec445456da967cb3a28bbbbc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPaymentManager

## Description
Cause 1 of 1: GetPaymentManager's ResourceNotFoundException message text diverges from AWS.

AWS: Smithy (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol): GetPaymentManager GET /payments/managers/{paymentManagerId}, paymentManagerId @httpLabel @required, errors include ResourceNotFoundException (client, httpError 404). The model does not state the message text, so recorded behavior only: when the named payment manager does not exist, AWS answers HTTP 404 with x-amzn-errortype ResourceNotFoundException and the message template "Payment manager not found: <paymentManagerId>" — the fixed text is "Payment manager not found: " and the single variable part is the paymentManagerId taken verbatim from the request URI label (the recordings' "probenofakepm-abcdefghij" is only one run's value filling that slot, not part of the requirement).

Emulator: answers the correct status (404) and error class, but renders a different message, "Payment manager with ID <paymentManagerId> does not exist" — a different fixed phrasing ("... with ID ... does not exist") around the same request-supplied id. Replay reports $.message: value_mismatch (expected 'Payment manager not found: probenofakepm-abcdefghij', got 'Payment manager with ID probenofakepm-abcdefghij does not exist').

Why: a client or test that asserts on, logs, or pattern-matches the ResourceNotFoundException message for a missing payment manager (a common way SDK consumers distinguish "no such manager" from other 404s, and the way error strings are surfaced to operators) sees text AWS never emits. The condition is reported, but the human- and machine-readable payload is wrong, so message-based branching and golden-output comparisons that pass against AWS fail against the emulator.

Fix: the message template in GetPaymentManager's handler / its not-found error construction for the payment-manager lookup path in the bedrock-agentcore-control service crate — emit the fixed text "Payment manager not found: " followed by the requested paymentManagerId, keeping status 404 and errortype ResourceNotFoundException as they already are. The same template should be used wherever the payment-manager registry reports a missing manager so both recorded fixtures agree.

No open issue carries this replay key: the replay reported this cluster with an empty owned_by, and the neighbouring open bedrock-agentcore-control issues (20261108T224708Z-bedrock-agentcore-control-2-net-new-causes-deletepolicyengine.md, 20261108T224708Z-bedrock-agentcore-control-updatepaymentcredentialprovider-stub.md, 20261107T225524Z-bedrock-agentcore-payments-instrument-surface-is-unserved-createpaymentinstrument-and-listpaymentinstruments.md) concern DeletePolicyEngine, UpdatePaymentCredentialProvider and the payment-instrument surface, not GetPaymentManager's not-found wording.

Traces: bedrock-agentcore-control-not-found-error-paths#task#2 (representative) and bedrock-agentcore-control-payment-error-paths#task#2, both in the same cluster service=bedrock-agentcore-control|GetPaymentManager|status=404->404|keys=$.message.

## Evidence
Replay divergence (verdict mismatch, cluster key service=bedrock-agentcore-control|GetPaymentManager|status=404->404|keys=$.message), affected_count 2.

Expected (AWS recording): HTTP 404, x-amzn-errortype: ResourceNotFoundException, body {"message": "Payment manager not found: probenofakepm-abcdefghij"}.
Actual (emulator): HTTP 404, ResourceNotFoundException, body {"message": "Payment manager with ID probenofakepm-abcdefghij does not exist"}.

Detail verbatim: "$.message: value_mismatch (expected 'Payment manager not found: probenofakepm-abcdefghij', got 'Payment manager with ID probenofakepm-abcdefghij does not exist')".

Status and error class match; only the message's fixed text differs. Fixed text required: "Payment manager not found: ". Variable part: the paymentManagerId from the request path label.

Model citation: Smithy GetPaymentManager — GET /payments/managers/{paymentManagerId}; input paymentManagerId @required @httpLabel (length 12..211, pattern ^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$); errors ResourceNotFoundException httpError 404. Message wording itself: recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture bedrock-agentcore-control-not-found-error-paths, step task#2: GET /payments/managers/<paymentManagerId> for a payment-manager id that was never created (shaped to satisfy the paymentManagerId pattern, e.g. the fixture's probenofakepm-abcdefghij).
3. Observe the 404 ResourceNotFoundException body: the emulator returns message "Payment manager with ID <id> does not exist" where AWS returns "Payment manager not found: <id>".
4. The same divergence reproduces at bedrock-agentcore-control-payment-error-paths#task#2.
