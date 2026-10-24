Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-catalog-resource-policy-error-lifecycle-v2
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-catalog-resource-policy-error-lifecycle-v2
Issue-Shape-Id: com.amazonaws.marketplacecatalog#AWSMPSeymour
Issue-Key: marketplace-catalog|GetResourcePolicy|2xx->4xx|$.Policy
Issue-Key: marketplace-catalog|GetResourcePolicy|2xx->4xx|status
Issue-Key: marketplace-catalog|DeleteResourcePolicy|2xx->4xx|$
Issue-Key: marketplace-catalog|DeleteResourcePolicy|2xx->4xx|status
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# Resource-policy lifecycle: an ARN whose policy the catalog holds is answered 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
The emulator's resource-policy operations apply a resource-existence precondition that rejects the very ARN the catalog is holding a policy for, so the whole read/delete half of the resource-policy lifecycle answers 404 ResourceNotFoundException on its happy path. Two operations are affected; both are examples of the same precondition.

Cause 1 of 2: GetResourcePolicy returns 404 ResourceNotFoundException instead of the stored policy.
AWS: Smithy: GetResourcePolicy http GET /GetResourcePolicy, success code 200; input ResourceArn (ResourceARN, @required, httpQuery resourceArn, @length 1..255, pattern ^arn:[\w+=/,.@-]+:aws-marketplace:...); output member Policy (ResourcePolicyJson, @length 1..10240). ResourceNotFoundException (httpError 404) is modeled, but it is the error for an ARN the account/catalog does not hold — an ARN that carries a resource-based policy is held, so the modeled 200 + Policy applies. The recorded response for this step is 200 with $.Policy present.
Emulator: answers 404 and the body carries no Policy member at all ($.Policy missing, expected a policy document, got None). Reproduced at fixture step task#1 and again at task#4 of the same lifecycle, i.e. both before and after the delete, so it is not a post-delete-state effect.
Why: a client that attaches a resource policy and then reads it back cannot. Every SDK maps 404 ResourceNotFoundException to a terminal non-retryable error, so policy read-back, drift checks and any "get, modify, put" update loop fail outright rather than returning the document the client just stored; the client concludes the resource does not exist.
Fix: the GetResourcePolicy handler and the resource-existence/ARN-resolution predicate it shares with the other resource-policy operations — the lookup must treat an ARN with a stored policy entry as held and serve that entry, rather than gating the read on a separate entity-exists check.
Traces: marketplace-catalog-resource-policy-error-lifecycle-v2#task#1 (representative), also marketplace-catalog-resource-policy-error-lifecycle-v2#task#4.

Cause 2 of 2: DeleteResourcePolicy returns 404 ResourceNotFoundException instead of removing the policy and answering an empty 200.
AWS: Smithy: DeleteResourcePolicy http DELETE /DeleteResourcePolicy, success code 200; input ResourceArn (@required, httpQuery resourceArn); output shape DeleteResourcePolicyResponse has NO members, so success is 200 with an empty JSON body. ResourceNotFoundException (httpError 404) is modeled for an ARN the catalog does not hold. The recorded response for this step is 200 with body {}.
Emulator: answers 404 with body {"__type": "ResourceNotFoundException", "Message": "Requested entity '<entity-id>' does not exist."} (the recorded message text is an example; the status and the error code are the requirement). The same precondition as Cause 1 is rejecting the ARN.
Why: the delete half of the lifecycle is unreachable, so a client can never detach a policy it attached; teardown and policy-replacement flows abort on a non-retryable 404 and the policy is silently left in place.
Fix: the DeleteResourcePolicy handler and the same shared resource-existence predicate, so a held ARN deletes and returns an empty 200, with 404 reserved for ARNs the catalog genuinely does not hold.
Traces: marketplace-catalog-resource-policy-error-lifecycle-v2#task#3.

Related open issue, which does NOT cover this: "DeleteResourcePolicy answers 200 for an ARN naming a resource the catalog does not hold; ResourceNotFoundException is unreachable" (20261024T010454Z-marketplace-catalog-deleteresourcepolicy-error-response.md) reports the OPPOSITE polarity — a 200 where a 404 was due. The divergence here is a 404 where a 200 is due, on an ARN the catalog does hold, so a precondition added to satisfy that issue must not be allowed to over-reject held ARNs; both directions have to hold at once.

## Evidence
Replay of trace set simparity-wire-ga-prod-v1, fixture marketplace-catalog-resource-policy-error-lifecycle-v2 (13 fixtures, 28 steps, 25 passed, 3 mismatched, score 89.3%; no steps blocked, not_implemented=0, so both operations are implemented and answering).

Cause 1 - GetResourcePolicy, step task#1 (cluster also covers task#4), divergence_key service=marketplace-catalog|GetResourcePolicy|status=200->404|keys=$.Policy,status:
  expected: HTTP 200, $.Policy = a policy document (recorded: '{}')
  actual:   HTTP 404, $.Policy missing (None)
  detail:   "status: expected 200, got 404 / $.Policy: missing (expected '{}', got None)"

Cause 2 - DeleteResourcePolicy, step task#3, divergence_key service=marketplace-catalog|DeleteResourcePolicy|status=200->404|keys=$,status:
  expected: HTTP 200, empty body {} (DeleteResourcePolicyResponse has no members)
  actual:   HTTP 404, {"__type": "ResourceNotFoundException", "Message": "Requested entity 'prod-b2c3d4e5f6g7h' does not exist."}
  detail:   "status: expected 200, got 404 / $: value_mismatch (expected {}, got {'__type': 'ResourceNotFoundException', ...})"

Model consulted: services/service-com-amazonaws-marketplacecatalog-awsmpseymour/smithy-model/com-amazonaws-marketplacecatalog-awsmpseymour.json - GetResourcePolicy (200, output Policy: ResourcePolicyJson @length 1..10240), DeleteResourcePolicy (200, empty output shape), PutResourcePolicy (200, empty output shape); all three model ResourceNotFoundException at httpError 404.

Both clusters report stub_path="" and owned_by=[], so neither is blocked by an unimplemented operation nor already owned by an open issue's replay key.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture marketplace-catalog-resource-policy-error-lifecycle-v2 from trace set simparity-wire-ga-prod-v1.
2. Cause 1 (step task#1): attach a resource policy to a marketplace-catalog entity ARN with POST /PutResourcePolicy {"ResourceArn": "<arn:...:aws-marketplace:...:AWSMarketplace/SaaSProduct/<entity-id>>", "Policy": "{}"}, then read it back with GET /GetResourcePolicy?resourceArn=<same ARN>. Observe HTTP 404 ResourceNotFoundException with no Policy member; AWS answers 200 with Policy set to the stored document. Step task#4 repeats the same read later in the lifecycle with the same result.
3. Cause 2 (step task#3): against that same held ARN, call DELETE /DeleteResourcePolicy?resourceArn=<same ARN>. Observe HTTP 404 with {"__type": "ResourceNotFoundException"}; AWS answers 200 with an empty body and removes the policy.
4. Confirm the fix by re-replaying marketplace-catalog-resource-policy-error-lifecycle-v2 steps task#1, task#3 and task#4, and check that the existing opposite-polarity case still holds: DeleteResourcePolicy on an ARN the catalog does NOT hold must still answer 404 ResourceNotFoundException.
