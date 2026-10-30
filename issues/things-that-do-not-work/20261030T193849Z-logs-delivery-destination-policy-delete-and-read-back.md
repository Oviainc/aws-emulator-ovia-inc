Issue-Source: trace-sweep-divergence
Issue-Trace: logs-b2-delivery-source-destination-pipeline
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-b2-delivery-source-destination-pipeline logs-get-delivery-destination-policy-returns-empty-when-unset
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DeleteDeliveryDestinationPolicy|4xx->2xx|$
Issue-Key: logs|DeleteDeliveryDestinationPolicy|4xx->2xx|status
Issue-Key: logs|GetDeliveryDestinationPolicy|2xx->2xx|$.policy.deliveryDestinationPolicy
Issue-Commit: 9c3f119a129741821a20d0bbcab650876a52fb45

# logs delivery-destination policy delete and read-back diverge from AWS

> _Note: this issue was identified against the code base at commit `9c3f119a129741821a20d0bbcab650876a52fb45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDeliveryDestinationPolicy

## Description
Two root causes on the CloudWatch Logs delivery-destination policy surface: the delete path does not refuse when the destination carries no policy, and the read path does not return the stored policy document as AWS returns it. Cause count: 2 — both must be closed before submitting.

Related open issue 20261029T222444Z-logs-vended-delivery-pipeline-delivery-records-and-destination.md covers other delivery-destination policy gaps (the unvalidated Put, the missing `policy` member when unset) and carries different replay keys; neither cause below is covered by it, and both steps below were reported as unowned by the replay.

=== Cause 1 of 2: DeleteDeliveryDestinationPolicy succeeds when the destination has no policy ===
AWS: deleting the policy of a delivery destination that has no delivery-destination policy is refused with HTTP 400 and error code ResourceNotFoundException, message (fixed text, no variable parts) 'Delivery Destination Policy does not exist for this Delivery Destination.'. Smithy: DeleteDeliveryDestinationPolicy binds ResourceNotFoundException (httpError 400, defaulted) alongside ConflictException/ValidationException; its output is Unit, so the only observable on the success path is the 200 itself. Recorded response for this step: 400 ResourceNotFoundException with that message.
Emulator: answers HTTP 200 with an empty body — no error at all. Replay reports 'status: expected 400, got 200' and '$: empty_body (expected {"__type": "ResourceNotFoundException", "message": "Delivery Destination Policy does not exist for this Delivery Destination."}, got {})'.
Why: a client that uses the delete call to assert whether a cross-account grant was ever installed (or that idempotently deletes and branches on ResourceNotFoundException) concludes a policy existed and was removed. On AWS the call fails, so teardown/clean-up logic and permission probes take the opposite branch against the emulator and the two environments diverge permanently from that point.
Fix: the not-found precondition of the DeleteDeliveryDestinationPolicy handler — look up the destination's policy in state and raise ResourceNotFoundException with the fixed message above when there is none, instead of returning success unconditionally (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/).
Traces: logs-b2-delivery-source-destination-pipeline#task#10.

=== Cause 2 of 2: GetDeliveryDestinationPolicy re-canonicalizes the stored policy document instead of returning it as written ===
AWS: GetDeliveryDestinationPolicy returns policy.deliveryDestinationPolicy as the policy document that was supplied to PutDeliveryDestinationPolicy — the JSON is round-tripped, so a `Principal` given in IAM-ARN form stays in IAM-ARN form ('arn:aws:iam::<account-id>:root') and a scalar `Action` stays a scalar string. Smithy: Policy.deliveryDestinationPolicy is a plain string member (DeliveryDestinationPolicy, @length 1..51200), so the model imposes no normalization; recorded behavior only for the round-trip itself.
Emulator: returns a rewritten document — the principal is collapsed to the bare account id ('"Principal":{"AWS":"000000000000"}') and the single action is wrapped in a list ('"Action":["logs:CreateDelivery"]') — while Version, Sid, Effect and Resource survive. The account id, Sid and destination ARN in the recording are per-run values; the requirement is that the document comes back with the same principal form and the same scalar/list shape for Action as it was stored with.
Why: a client that reads the policy back and compares it to the document it submitted (drift detection, idempotent re-put, policy diffing in IaC tooling) sees a difference on every read and re-applies the policy forever; code that parses `Action` as a string throws on the list form.
Fix: the GetDeliveryDestinationPolicy read path and the state record behind it — store the delivery-destination policy document verbatim and serialize it unchanged rather than parsing it into an internal policy model and re-emitting it (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/ plus the delivery-destination policy state).
Traces: logs-get-delivery-destination-policy-returns-empty-when-unset#task#7.

## Evidence
Cause 1 (logs-b2-delivery-source-destination-pipeline#task#10, DeleteDeliveryDestinationPolicy, expected 400 -> actual 200): "status: expected 400, got 200\n$: empty_body (expected {'__type': 'ResourceNotFoundException', 'message': 'Delivery Destination Policy does not exist for this Delivery Destination.'}, got {})". Requirement: HTTP 400, __type 'ResourceNotFoundException', message fixed text 'Delivery Destination Policy does not exist for this Delivery Destination.'

Cause 2 (logs-get-delivery-destination-policy-returns-empty-when-unset#task#7, GetDeliveryDestinationPolicy, 200 -> 200): "$.policy.deliveryDestinationPolicy: value_mismatch (expected '{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"sim2realAllowCreateDelivery\",\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"},\"Action\":\"logs:CreateDelivery\",\"Resource\":\"arn:aws:logs:us-west-2:000000000000:delivery-destination:sim2real-probe-ddest-23025854\"}]}', got '{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"sim2realAllowCreateDelivery\",\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"000000000000\"},\"Action\":[\"logs:CreateDelivery\"],\"Resource\":\"arn:aws:logs:us-west-2:000000000000:delivery-destination:sim2real-probe-ddest-23025854\"}]}')". The differing parts are the Principal form (IAM root ARN vs bare account id) and the Action shape (scalar string vs single-element list); the account id, Sid and ARN are per-run values from the request, not requirements.

## Steps to Reproduce
Cause 1: PutDeliveryDestination(name=<dest>) for a destination with no delivery-destination policy (in the fixture the preceding PutDeliveryDestinationPolicy is itself refused by AWS), then call DeleteDeliveryDestinationPolicy(deliveryDestinationName=<dest>) — expect HTTP 400 ResourceNotFoundException with message 'Delivery Destination Policy does not exist for this Delivery Destination.'; the emulator answers 200 with an empty body. Replay logs-b2-delivery-source-destination-pipeline#task#10.

Cause 2: PutDeliveryDestination(name=<dest>), then PutDeliveryDestinationPolicy(deliveryDestinationName=<dest>, deliveryDestinationPolicy='{"Version":"2012-10-17","Statement":[{"Sid":"<sid>","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account-id>:root"},"Action":"logs:CreateDelivery","Resource":"<destination-arn>"}]}'), then GetDeliveryDestinationPolicy(deliveryDestinationName=<dest>) — expect policy.deliveryDestinationPolicy to parse back to the same document (principal still the root ARN, Action still a scalar string). Replay logs-get-delivery-destination-policy-returns-empty-when-unset#task#7.
