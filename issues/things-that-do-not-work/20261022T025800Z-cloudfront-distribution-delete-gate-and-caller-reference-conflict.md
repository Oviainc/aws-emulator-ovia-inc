Issue-Source: trace-sweep-divergence
Issue-Trace: cfetag-distribution-etag-chain
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfetag-distribution-etag-chain cfval-caller-reference-reuse-conflict
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|DeleteDistribution|4xx->2xx|$
Issue-Key: cloudfront|DeleteDistribution|4xx->2xx|$headers.content-length
Issue-Key: cloudfront|DeleteDistribution|4xx->2xx|status
Issue-Key: cloudfront|GetDistribution|2xx->4xx|$
Issue-Key: cloudfront|GetDistribution|2xx->4xx|$.Distribution
Issue-Key: cloudfront|GetDistribution|2xx->4xx|$.Distribution.Status
Issue-Key: cloudfront|GetDistribution|2xx->4xx|$headers.etag
Issue-Key: cloudfront|GetDistribution|2xx->4xx|status
Issue-Key: cloudfront|DeleteDistribution|2xx->4xx|status
Issue-Key: cloudfront|CreateDistribution|2xx->4xx|$.Distribution
Issue-Key: cloudfront|CreateDistribution|2xx->4xx|$headers.etag
Issue-Key: cloudfront|CreateDistribution|2xx->4xx|$headers.location
Issue-Key: cloudfront|CreateDistribution|2xx->4xx|status
Issue-Commit: b0c0a3cf60eeb3c3c5e56cfd149cad2378448d10

# CloudFront distribution delete gate and caller-reference conflict are wrong

> _Note: this issue was identified against the code base at commit `b0c0a3cf60eeb3c3c5e56cfd149cad2378448d10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDistribution

## Description
Two state-machine rules around distribution identity and deletion are inverted, and they corrupt the rest of the fixture they appear in. (state_reset_failures was 0 for this run, so these are not cross-fixture state leaks.)

Cause 1 of 2: DeleteDistribution deletes a still-enabled distribution instead of refusing it, which then breaks the remaining lifecycle steps.
AWS: Smithy DeleteDistribution (DELETE /2020-05-31/distribution/{Id}, success 204) models DistributionNotDisabled with httpError 409 — 'The specified CloudFront distribution is not disabled. You must disable the distribution before you can delete it.' The recording is 409 with Code 'DistributionNotDisabled', Type 'Sender', Message 'The distribution you are trying to delete has not been disabled.', with a content-length-bearing body.
Emulator: 204 No Content with an empty body — it deletes the enabled distribution. The two following recorded steps then diverge as a direct consequence: GetDistribution, which should still return 200 and poll to Status 'Deployed', returns 404 (the replay poll capped at 60s), and the subsequent legitimate DeleteDistribution, which should return 204, returns 404 because the distribution is already gone.
Why: the Enabled=false-before-delete rule that every CloudFront teardown script depends on does not exist, so client code that is incorrect against AWS passes here, and any state a client expects to still be readable after a refused delete is destroyed.
Fix: the DeleteDistribution handler in the cloudfront service crate must check the stored DistributionConfig.Enabled flag (and deployment status) before removing state and raise DistributionNotDisabled (409) when it is still enabled.

Cause 2 of 2: a fresh CallerReference is reported as an existing one (expected 201, got 409).
AWS: Smithy models DistributionConfig.CallerReference as required and documents the rule precisely: 'If the value of CallerReference is new (regardless of the content of the DistributionConfig object), CloudFront creates a new distribution. If CallerReference is a value that you already sent in a previous request to create a distribution, CloudFront returns a DistributionAlreadyExists error.' The recording for a create with a new caller reference is 201 with the full Distribution payload.
Emulator: 409 DistributionAlreadyExists for a create whose CallerReference has not been used before — the conflict check is keyed on something broader than the caller reference (the fixture's earlier distribution differs only in caller reference), so a legitimate second distribution can never be created.
Why: the documented idempotency contract is inverted: distinct caller references must yield distinct distributions. Any client that creates more than one similar distribution, or retries with a new reference after a conflict, is permanently blocked with a 409.
Fix: the CreateDistribution handler's conflict detection must compare only CallerReference against previously accepted creates, not the rest of the config.

Traces: cfetag-distribution-etag-chain#task#5 (delete gate), with the follow-on clusters cfetag-distribution-etag-chain#task#6 (GetDistribution 200 -> 404) and #task#7 (DeleteDistribution 204 -> 404); cfval-caller-reference-reuse-conflict#task#3 (caller-reference conflict).

## Evidence
cfetag-distribution-etag-chain#task#5 (DeleteDistribution): 'status: expected 409, got 204'; '$: empty_body (expected {ErrorResponse: {Error: {Type: Sender, Code: DistributionNotDisabled, Message: "The distribution you are trying to delete has not been disabled."}, RequestId: <shaped-not-asserted>}}, got {})'; '$headers.content-length: missing'. #task#6 (GetDistribution): 'status: expected 200, got 404' — expected body is the Deployed distribution E7IYCGPKTANSF3. #task#7 (DeleteDistribution): 'status: expected 204, got 404'. cfval-caller-reference-reuse-conflict#task#3 (CreateDistribution): 'status: expected 201, got 409'; '$.Distribution: missing (expected the full Distribution for CallerReference sim2real-probe-cfval-c05c7a74-dbdf-41ea-8b39-6b87b5ae93c9, got None)'.

## Steps to Reproduce
1. Replay fixture cfetag-distribution-etag-chain: create a distribution (task#1), then at task#5 issue DELETE /2020-05-31/distribution/{Id} while DistributionConfig.Enabled is still true — expect 409 DistributionNotDisabled, observe 204. 2. Continue to task#6/task#7: GetDistribution now 404 instead of 200 and the real delete 404 instead of 204. 3. Replay cfval-caller-reference-reuse-conflict: task#1 creates with caller reference A, task#2 reuses A (409, correct), task#3 creates with a brand-new caller reference — expect 201, observe 409.
