Issue-Source: trace-sweep-divergence
Issue-Trace: freetier-get-freetier-usage-multiple-match-options-rejected
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: freetier-get-freetier-usage-multiple-match-options-rejected freetier-account-plan-state-account-id-matches-caller-identity
Issue-Shape-Id: com.amazonaws.freetier#AWSFreeTierService
Issue-Key: freetier|GetFreeTierUsage|2xx->2xx|$headers.content-type
Issue-Key: freetier|GetFreeTierUsage|4xx->4xx|$.message
Issue-Key: freetier|GetFreeTierUsage|4xx->4xx|$headers.content-type
Issue-Key: freetier|GetAccountPlanState|4xx->2xx|$.__type
Issue-Key: freetier|GetAccountPlanState|4xx->2xx|$.message
Issue-Key: freetier|GetAccountPlanState|4xx->2xx|$headers.content-type
Issue-Key: freetier|GetAccountPlanState|4xx->2xx|status
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# freetier responses use awsJson1_0 content-type and skip ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFreeTierUsage

## Description
Two independent defects in the AWS Free Tier (AWSFreeTierService, apiVersion 2023-09-07) emulator, both found by replaying the wire corpus. This issue bundles 2 causes; BOTH must be closed before submitting.

Cause 1 of 2 — every freetier response is serialized with the wrong awsJson content-type (affects GetFreeTierUsage success, GetFreeTierUsage ValidationException, and GetAccountPlanState alike).
AWS: the recorded AWS responses for this service carry the response header `content-type: application/x-amz-json-1.1`, on success (HTTP 200) and on error (HTTP 400) alike — recorded behavior only, observed identically in every step of both fixtures. Note the repository's on-disk Smithy model for this service declares `aws.protocols#awsJson1_0` (smithy_query service_info -> protocols: ["awsJson1_0"], sdkId FreeTier, endpointPrefix freetier, sigv4Name freetier), which is what the emulator is evidently following; the recording is the wire authority here and disagrees with that trait.
Emulator: emits `content-type: application/x-amz-json-1.0` on every response. Replay detail for all three clusters: "$headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/x-amz-json-1.0')".
Why: the awsJson content-type is how an AWS SDK client confirms the protocol of the body it is about to parse. A client built against the real service's 1.1 framing sees a 1.0 media type on every operation and can reject or mis-dispatch the response, so even the steps whose status and body are otherwise correct (GetFreeTierUsage task#1/task#3) are unusable — this is a blanket, every-operation protocol defect, not a per-operation one.
Fix: the service's protocol/response-serializer layer, where the awsJson media type is chosen for both success and error responses (and, if the protocol trait itself is the source, the declared protocol for this service). One change at the serializer fixes all three clusters.
Traces: freetier-get-freetier-usage-multiple-match-options-rejected#task#1 (also affects #task#3), freetier-get-freetier-usage-multiple-match-options-rejected#task#2, freetier-account-plan-state-account-id-matches-caller-identity#task#3.

Cause 2 of 2 — GetAccountPlanState returns a 200 success body for an account that has no account-plan data, instead of the modeled not-found error.
AWS: when no account-plan record exists for the calling account, the recorded response is HTTP 400 with an awsJson error body whose `__type` is `ResourceNotFoundException` and whose `message` is built from the request's resolved account id, of the form "Missing data for account: <accountId>" (recorded example: 'Missing data for account: 000000000000'). Smithy: GetAccountPlanState models ResourceNotFoundException as a client error (the model annotates @httpError 404, but the recording shows the service answering 400 for this condition — match the recording). GetAccountPlanState's output members accountId/accountPlanType/accountPlanStatus are all @required, so there is no well-formed success representation of "no plan data": the operation must fail rather than synthesize one.
Emulator: answers HTTP 200 and returns no error at all — replay detail: "status: expected 400, got 200; $.__type: missing (expected 'ResourceNotFoundException', got None); $.message: missing (expected 'Missing data for account: 000000000000', got None)".
Why: a client that handles ResourceNotFoundException to detect an account with no Free Tier plan data never sees it; it instead receives a 200 whose required fields are absent or defaulted, so it reads a fabricated plan state as real and takes the wrong branch. Error handling for this path cannot be exercised against the emulator at all.
Fix: the GetAccountPlanState handler and its state lookup — when the account-plan record for the resolved caller account is absent, raise ResourceNotFoundException (HTTP 400, `__type` ResourceNotFoundException, message naming the account id) rather than returning a success shape.
Traces: freetier-account-plan-state-account-id-matches-caller-identity#task#3.

Not filed as a cause: in freetier-get-freetier-usage-multiple-match-options-rejected#task#2 the emulator already returns the correct 400 ValidationException for a filter with two MatchOptions; only the message text differs (recorded '[Message = Exactly one match option must be provided]' vs emitted 'filter.Dimensions.MatchOptions: exactly one match option is allowed, got 2'). Neither the model nor the recording states the rule that produces that wording, so the recorded text is an example, not a requirement. That step is listed above only for its content-type divergence (Cause 1).

No prior open issue covers either cause; the only existing freetier issue is the bootstrap issue, which is already resolved.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, steps_total 6, steps_passed 2, steps_mismatched 4, score_pct 33.3, state_reset_failures 0, divergence_count 3. All 3 divergence clusters are service=freetier with empty stub_path and empty owned_by.

Cause 1 (content-type, 3 clusters / 4 steps):
- fixture freetier-get-freetier-usage-multiple-match-options-rejected step task#1 (GetFreeTierUsage), expected 200 / actual 200, affected also task#3 -> "$headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/x-amz-json-1.0')"
- fixture freetier-get-freetier-usage-multiple-match-options-rejected step task#2 (GetFreeTierUsage), expected 400 / actual 400 -> same content-type value_mismatch
- fixture freetier-account-plan-state-account-id-matches-caller-identity step task#3 (GetAccountPlanState) -> same content-type value_mismatch
Expected header: content-type: application/x-amz-json-1.1. Actual header: content-type: application/x-amz-json-1.0.

Cause 2 (GetAccountPlanState not-found, 1 cluster):
- fixture freetier-account-plan-state-account-id-matches-caller-identity step task#3, divergence_key service=freetier|GetAccountPlanState|status=400->200|keys=$.__type,$.message,$headers.content-type,status
  Expected: HTTP 400, body {"__type": "ResourceNotFoundException", "message": "Missing data for account: <accountId>"} (recorded account id 000000000000)
  Actual:   HTTP 200, no __type, no message

Model evidence (smithy_query on services/service-com-amazonaws-freetier-awsfreetierservice/smithy-model/com-amazonaws-freetier-awsfreetierservice.json): service AWSFreeTierService v2023-09-07, protocols [awsJson1_0], sigv4Name freetier; GetAccountPlanState output accountId/accountPlanType/accountPlanStatus @required, errors include ResourceNotFoundException (client, @httpError 404), ValidationException (client, 400); GetFreeTierUsage errors InternalServerException/ThrottlingException/ValidationException.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: replay fixture freetier-get-freetier-usage-multiple-match-options-rejected, step task#1 — call GetFreeTierUsage with a valid filter and inspect the response headers. Observe content-type: application/x-amz-json-1.0 where the recording has application/x-amz-json-1.1. Repeat for step task#2 (filter whose Dimensions has two MatchOptions -> 400 ValidationException) and for fixture freetier-account-plan-state-account-id-matches-caller-identity step task#3: the same wrong media type appears on every response, success and error.
4. Cause 2: replay fixture freetier-account-plan-state-account-id-matches-caller-identity, step task#3 — call GetAccountPlanState as a caller whose account has no account-plan record seeded. Observe HTTP 200 with no error body, where the recording has HTTP 400 with __type ResourceNotFoundException and a message naming the account id.
5. After fixing, re-replay both fixtures (freetier-get-freetier-usage-multiple-match-options-rejected#task#1, #task#2 and freetier-account-plan-state-account-id-matches-caller-identity#task#3) and confirm all 6 steps pass.
