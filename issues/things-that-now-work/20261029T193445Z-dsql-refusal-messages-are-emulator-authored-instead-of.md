Issue-Source: trace-sweep-divergence
Issue-Trace: dsql-request-validation-error-surface
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsql-request-validation-error-surface dsql-cluster-error-paths dsql-cluster-policy-version-guards dsql-missing-cluster-error-surface
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql|ListClusters|4xx->4xx|$.fieldList[*]
Issue-Key: dsql|ListClusters|4xx->4xx|$.message
Issue-Key: dsql|ListClusters|4xx->4xx|$.fieldList[name="next-token"]
Issue-Key: dsql|PutClusterPolicy|4xx->4xx|$.message
Issue-Key: dsql|CreateCluster|4xx->4xx|$.message
Issue-Key: dsql|ListTagsForResource|4xx->4xx|$.message
Issue-Key: dsql|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|TagResource|4xx->4xx|$.message
Issue-Key: dsql|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|UntagResource|4xx->4xx|$.message
Issue-Key: dsql|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 16092bb1fe4b1c4e1f6f331d6e3c413bf3cfad22

# dsql refusal messages are emulator-authored instead of Aurora DSQL's fixed sentences

> _Note: this issue was identified against the code base at commit `16092bb1fe4b1c4e1f6f331d6e3c413bf3cfad22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListClusters

## Description
Seven refusals return the right status but a message (and, for the pagination refusals, a fieldList entry) the emulator wrote itself rather than the sentence Aurora DSQL emits for that error class. The status and error code are correct; the human- and machine-readable text is not.

Cause 1 of 7: ListClusters with a nextToken the service never issued.
AWS: 400 ValidationException whose message is the fixed sentence "Invalid pagination token. Request a new token and try again." and whose fieldList carries exactly one entry {name: "next-token", message: "Invalid pagination token. Request a new token and try again."} — the entry's `name` is the WIRE member name next-token, and the whole message is fixed text with no value interpolated from the request. Smithy: ValidationException requires message and reason, fieldList is a ValidationExceptionFieldList of {name, message} (both required).
Emulator: 400 with the message "nextToken must be a nextToken a previous page of this operation returned, got <token>" and a fieldList entry {name: "nextToken", message: "must be a nextToken a previous page of this operation returned"} — generic framework-constraint prose that also echoes the rejected token back.
Why: clients and operators keyed on the documented sentence (and on the next-token field name) do not match, and the echoed token leaks the caller's input into a message AWS keeps constant.
Fix: ListClusters' pagination validation, which must emit DSQL's fixed pagination sentence and name the field next-token instead of deriving both from the Smithy member name.
Traces: dsql-request-validation-error-surface#task#1.

Cause 2 of 7: the same ListClusters pagination refusal reached from the cluster-error-paths sequence.
AWS: same 400 ValidationException, same fixed sentence, same single fieldList entry named next-token.
Emulator: same emulator-authored message; the fieldList entry for next-token is absent altogether.
Why: as Cause 1; additionally a client that reads fieldList to locate the offending field finds nothing.
Fix: same component as Cause 1.
Traces: dsql-cluster-error-paths#task#4.

Cause 3 of 7: PutClusterPolicy with a policy document that is not a valid policy.
AWS: 400 ValidationException with the fixed sentence pair "Aurora DSQL can't process the request because the policy that you specified isn't valid. Provide the policy in a valid format and try again." Smithy: PutClusterPolicy.policy targets PolicyDocument (@length 1..20480), ValidationException httpError 400.
Emulator: 400 with "The policy is not a valid JSON policy document."
Why: the status matches but the text does not, so message-asserting clients and goldens diverge; the open issue 20261028T131655Z-dsql-putclusterpolicy-error-response covers only WHEN this check runs relative to the idempotency ledger, and 20261029T185208Z-dsql-putclusterpolicy-s-lockout-safety-refusal-carries-the covers the lockout-safety message, neither the invalid-document wording.
Fix: the shared policy-document validation used by the policy write path.
Traces: dsql-cluster-policy-version-guards#task#4.

Cause 4 of 7: CreateCluster with an invalid inline policy document.
AWS: 400 ValidationException with the same fixed sentence pair as Cause 3 — the wording belongs to the error class, not to the operation.
Emulator: 400 with "The policy is not a valid JSON policy document."
Why: as Cause 3, on the create path.
Fix: same shared policy-document validation, reached from CreateCluster.
Traces: dsql-request-validation-error-surface#task#6.

Cause 5 of 7: ListTagsForResource for a resourceArn naming a cluster that does not exist.
AWS: 404 ResourceNotFoundException whose message is the fixed text "Resource was not found" — no ARN, identifier or remediation sentence is interpolated. Smithy: ResourceNotFoundException httpError 404, required message/resourceId/resourceType (the ARN belongs in resourceId, not in the message).
Emulator: 404 with "The resource with ARN <arn> doesn't exist. Verify the ARN and try again."
Why: message-asserting clients diverge, and the emulator puts resource identity in prose where AWS puts it in the modeled resourceId member.
Fix: the tagging path's not-found error construction (shared ARN resolver for the /tags/{resourceArn} operations).
Traces: dsql-missing-cluster-error-surface#task#8.

Cause 6 of 7: TagResource against a resourceArn naming a nonexistent cluster.
AWS: 404 ResourceNotFoundException, message "Resource was not found".
Emulator: 404 with the descriptive ARN sentence.
Why: as Cause 5.
Fix: same shared tagging not-found construction.
Traces: dsql-missing-cluster-error-surface#task#9.

Cause 7 of 7: UntagResource against a resourceArn naming a nonexistent cluster.
AWS: 404 ResourceNotFoundException, message "Resource was not found".
Emulator: 404 with the descriptive ARN sentence.
Why: as Cause 5.
Fix: same shared tagging not-found construction.
Traces: dsql-missing-cluster-error-surface#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1:
- dsql-request-validation-error-surface#task#1 ListClusters (400 both sides): $.message expected 'Invalid pagination token. Request a new token and try again.', got 'nextToken must be a nextToken a previous page of this operation returned, got sim2real-not-a-pagination-token'; $.fieldList[0] expected {'name': 'next-token', 'message': 'Invalid pagination token. Request a new token and try again.'}, got {'name': 'nextToken', 'message': 'must be a nextToken a previous page of this operation returned'}.
- dsql-cluster-error-paths#task#4 ListClusters (400 both sides): $.fieldList[name="next-token"] missing entirely; $.message as above.
- dsql-cluster-policy-version-guards#task#4 PutClusterPolicy (400 both sides): expected "Aurora DSQL can't process the request because the policy that you specified isn't valid. Provide the policy in a valid format and try again.", got 'The policy is not a valid JSON policy document.'
- dsql-request-validation-error-surface#task#6 CreateCluster (400 both sides): identical expected/actual message pair.
- dsql-missing-cluster-error-surface#task#8 ListTagsForResource, #task#9 TagResource, #task#10 UntagResource (404 both sides): expected 'Resource was not found', got "The resource with ARN arn:aws:dsql:us-west-2:000000000000:cluster/aaaaaaaaaaaaaaaaaaaaaaaaaa doesn't exist. Verify the ARN and try again."

## Steps to Reproduce
1. Replay dsql-request-validation-error-surface step task#1 and dsql-cluster-error-paths step task#4: GET /cluster?nextToken=<token never issued> -> compare $.message and $.fieldList against the recorded ValidationException.
2. Replay dsql-cluster-policy-version-guards step task#4: PutClusterPolicy with a malformed policy document -> compare $.message.
3. Replay dsql-request-validation-error-surface step task#6: CreateCluster with a malformed inline policy -> compare $.message.
4. Replay dsql-missing-cluster-error-surface steps task#8/#9/#10: ListTagsForResource / TagResource / UntagResource on a cluster ARN that does not exist -> compare $.message against 'Resource was not found'.
