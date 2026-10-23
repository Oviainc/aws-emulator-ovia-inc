Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-pricing-calculator-get-preferences-readonly
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-pricing-calculator-get-preferences-readonly
Issue-Shape-Id: com.amazonaws.bcmpricingcalculator#AWSBCMPricingCalculator
Issue-Key: bcm-pricing-calculator|GetPreferences|2xx->2xx|$headers.content-type
Issue-Commit: 8da75f4f8e37c6bec2b6e28a02ca64766de7a47e

# Responses carry content-type application/x-amz-json-1.0 instead of the recorded 1.1

> _Note: this issue was identified against the code base at commit `8da75f4f8e37c6bec2b6e28a02ca64766de7a47e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPreferences

## Description
Cause 1 of 1: every bcm-pricing-calculator response is framed with the wrong awsJson content-type.
  AWS: Smithy: the service AWSBCMPricingCalculator (version 2024-06-19, sdkId "BCM Pricing Calculator") declares protocol awsJson1_0, but the recorded responses from the real service are framed as "content-type: application/x-amz-json-1.1" — on every status, success and error alike. The wire recording is the authority for the response framing here, and it is unambiguous: this header is 1.1 in every single recorded step of all seven fixtures.
  Emulator: answers with "content-type: application/x-amz-json-1.0" on every response. GetPreferences (bcm-pricing-calculator-get-preferences-readonly#task#1) is the clean witness: the status is 200 and the body matches exactly, and the content-type header is the ONLY diverging key.
  Why: the content-type is how a client picks its protocol deserializer and how proxies, recorders and strict SDK response handlers decide the payload is one they understand. A client pinned to the service's real framing rejects or mis-dispatches every response — including the error bodies — so the failure is not localized to one operation but to the whole service surface.
  Fix: the response serializer / content-type constant in the bcm-pricing-calculator crate's awsJson protocol layer — one service-wide header, not a per-operation change.
  Traces: bcm-pricing-calculator-get-preferences-readonly#task#1 (GetPreferences) is the isolated case. The same key also diverges, alongside unrelated causes, in bcm-pricing-calculator-bill-scenario-estimate-member-account-restricted, bcm-pricing-calculator-list-workload-estimates-pagination-includes-all-created, bcm-pricing-calculator-not-found-reads, bcm-pricing-calculator-workload-estimate-tagging and bcm-pricing-calculator-workload-estimate-usage-lifecycle; those steps are the replay keys of the other bcm-pricing-calculator issues in this sweep and will keep failing on their own causes until those are fixed too, so this issue is pointed at the one step where the header is the sole divergence.

Note: the sibling service bcm-dashboards has the same defect filed as an open issue ("bcm-dashboards responses carry content-type application/x-amz-json-1.0 instead of the recorded 1.1", 20261023T081740Z...). That issue is for a different service and does not cover bcm-pricing-calculator; the two may share a serializer helper and are worth fixing together.

## Evidence
Divergence cluster (affected_count 1), expected_status 200, actual_status 200, verdict mismatch, fixture bcm-pricing-calculator-get-preferences-readonly, step task#1, operation GetPreferences:
  detail: "$headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/x-amz-json-1.0')"
No other key diverges in this cluster — the status and the whole JSON body match — so the header is the isolated, attributable defect.

The same expected/actual header pair appears in 17 further clusters across the other five bcm-pricing-calculator fixtures (e.g. workload-estimate-usage-lifecycle#setup#1, not-found-reads#task#1, list-workload-estimates-pagination-includes-all-created#task#4), confirming it is service-wide and not operation-specific.

Model grounding: smithy_query service_info on services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/smithy-model/com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator.json → {"service": "AWSBCMPricingCalculator", "version": "2024-06-19", "protocols": ["awsJson1_0"], "sigv4Name": "bcm-pricing-calculator"} — the model's protocol trait says 1_0 while the recorded wire says 1.1, which is exactly why this must follow the recording rather than the protocol default.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Inspect fixture bcm-pricing-calculator-get-preferences-readonly, step task#1: call GetPreferences and read the response headers.
4. Expected: content-type: application/x-amz-json-1.1. Actual: content-type: application/x-amz-json-1.0.
5. After the fix that step must report passed, and the $headers.content-type key must disappear from the detail of every other bcm-pricing-calculator cluster.
