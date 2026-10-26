Issue-Source: trace-sweep-divergence
Issue-Trace: ddbctl-resource-policy-deny-vs-not-found
Issue-Step: task#13
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ddbctl-resource-policy-deny-vs-not-found dynamodb-resource-policy-lifecycle dynamodb-resource-policy-management dynamodb-tagging-and-resource-policy
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|GetResourcePolicy|4xx->4xx|$.message
Issue-Key: dynamodb|DeleteResourcePolicy|4xx->4xx|$.message
Issue-Key: dynamodb|PutResourcePolicy|4xx->4xx|$.message
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Resource-policy PolicyNotFoundException messages drop AWS's wording and the requested/actual revision ids

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
DynamoDB's resource-based-policy operations do raise PolicyNotFoundException with the correct HTTP status (400) and error code, but the message body is composed from the emulator's own wording instead of AWS's, and on the ExpectedRevisionId paths it drops the request-derived and state-derived revision ids that AWS echoes. 3 causes, all dynamodb.

Cause 1 of 3: GetResourcePolicy against a table that has no resource-based policy returns the wrong PolicyNotFoundException text.
AWS: Smithy: GetResourcePolicy input ResourceArn is required (ResourceArnString, @length 1..1283); modeled error PolicyNotFoundException (client, httpError 400), documented as "The operation tried to access a nonexistent resource-based policy. If you specified an ExpectedRevisionId, it's possible that a policy is present for the resource but its revision ID didn't match the expected value." The recorded responses show the message is built from the request: the fixed prefix "Resource-based policy not found for the provided ResourceArn: " followed by the ResourceArn that was sent (recorded example: 'Resource-based policy not found for the provided ResourceArn: arn:aws:dynamodb:us-west-2:000000000000:table/sim2real-probe-ddbctl-authz-0e2781f7'). The same prefix appears independently in divergence 39 of this same replay (dynamodb-tagging-and-resource-policy#task#8), so the prefix is the rule, the ARN after it is the request-derived part.
Emulator: HTTP 400 with the right code but message 'No resource-based policy is attached to <arn>' - AWS's prefix is replaced wholesale.
Why: clients and operators distinguish the two PolicyNotFoundException situations (no policy at all vs. revision mismatch) by this text, and SDK-level contract tests and log scrapers match on the "Resource-based policy not found for the provided ResourceArn" prefix; against the emulator that match never fires.
Fix: GetResourcePolicy's not-found path in the resource-policy handler (or the shared resource-policy error renderer) should emit AWS's prefix plus the request's ResourceArn.
Traces: ddbctl-resource-policy-deny-vs-not-found#task#13 (cluster of 5; also dynamodb-resource-policy-lifecycle#task#4, dynamodb-resource-policy-management#task#8, dynamodb-tagging-and-resource-policy#task#13 and #task#14).

Cause 2 of 3: DeleteResourcePolicy with an ExpectedRevisionId that does not match the stored policy omits both revision ids from the error.
AWS: Smithy: DeleteResourcePolicy input ExpectedRevisionId targets PolicyRevisionId (@length 1..255); modeled error PolicyNotFoundException (client, 400), whose documentation explicitly covers the revision-mismatch case. The recorded message is built from the request and from state: AWS's PolicyNotFound prefix followed by 'Requested update for policy with revision id <the ExpectedRevisionId that was sent>, but the policy associated to target table <table> has revision id <the policy's current revision id>.' (recorded example: "Resource-based policy not found for the provided ResourceArn: Requested update for policy with revision id 00000000000000000000000000000000, but the policy associated to target table sim2real-probe-dynamodb-table-2efda94c has revision id 4e81a1d5240ce222c41bf836b3a27f23c445c8a4eb3fc2a922e81952c811fc51.").
Emulator: HTTP 400 with 'No resource-based policy with the expected revision id is attached to <arn>' - neither the supplied revision id nor the policy's actual revision id appears.
Why: this is the error a client doing optimistic concurrency on a resource policy relies on; without the actual revision id in the message it cannot re-read-and-retry from the error alone, and it cannot tell a stale-revision conflict apart from "this table has no policy" (cause 3 produces the identical emulator string).
Fix: DeleteResourcePolicy's precondition check in the resource-policy handler/state - include the requested and the current revision id in the PolicyNotFoundException message.
Traces: dynamodb-resource-policy-lifecycle#task#7 (cluster of 2; also dynamodb-resource-policy-management#task#10).

Cause 3 of 3: PutResourcePolicy with an ExpectedRevisionId against a table that has no policy returns the same generic text instead of AWS's "no associated policy" message.
AWS: Smithy: PutResourcePolicy input ExpectedRevisionId targets PolicyRevisionId (@length 1..255); modeled error PolicyNotFoundException (client, 400). Recorded message, built from the request: AWS's PolicyNotFound prefix plus 'Requested update for policy with revision id <ExpectedRevisionId sent>, but table <table> has no associated policy.' (recorded example: "Resource-based policy not found for the provided ResourceArn: Requested update for policy with revision id stale-revision-id-does-not-match, but table sim2real-probe-ddb-policy-e530a509 has no associated policy.").
Emulator: HTTP 400 with 'No resource-based policy with the expected revision id is attached to <arn>' - byte-identical to the cause-2 message, so the two distinct conditions are indistinguishable.
Why: a client that sends ExpectedRevisionId to guard an update cannot learn from the error whether its revision is stale or the policy was deleted out from under it, which is exactly the decision AWS's two messages exist to support.
Fix: PutResourcePolicy's ExpectedRevisionId precondition - branch on "no policy stored" vs. "revision differs" and render the matching AWS message.
Traces: dynamodb-resource-policy-management#task#9.

Related but not covering: open issue 20261023T105249Z-dynamodb-resource-based-policy-lifecycle-diverges-policy-unreadable-conditioned.md owns other resource-policy divergences (policy unreadable after put, conditioned deny refusing all reads, RevisionId shape). The three clusters above are reported unowned by the replay and are not covered by it.

## Evidence
Divergence 9 (GetResourcePolicy, 400->400, 5 affected steps): $.message value_mismatch - expected 'Resource-based policy not found for the provided ResourceArn: arn:aws:dynamodb:us-west-2:000000000000:table/sim2real-probe-ddbctl-authz-0e2781f7', got 'No resource-based policy is attached to arn:aws:dynamodb:us-west-2:000000000000:table/sim2real-probe-ddbctl-authz-0e2781f7'. Affected: ddbctl-resource-policy-deny-vs-not-found#task#13, dynamodb-resource-policy-lifecycle#task#4, dynamodb-resource-policy-management#task#8, dynamodb-tagging-and-resource-policy#task#13, #task#14.
Divergence 28 (DeleteResourcePolicy, 400->400, 2 affected steps): $.message value_mismatch - expected 'Resource-based policy not found for the provided ResourceArn: Requested update for policy with revision id 00000000000000000000000000000000, but the policy associated to target table sim2real-probe-dynamodb-table-2efda94c has revision id 4e81a1d5240ce222c41bf836b3a27f23c445c8a4eb3fc2a922e81952c811fc51.', got 'No resource-based policy with the expected revision id is attached to arn:aws:dynamodb:us-west-2:000000000000:table/sim2real-probe-dynamodb-table-2efda94c'. Affected: dynamodb-resource-policy-lifecycle#task#7, dynamodb-resource-policy-management#task#10.
Divergence 29 (PutResourcePolicy, 400->400): $.message value_mismatch - expected 'Resource-based policy not found for the provided ResourceArn: Requested update for policy with revision id stale-revision-id-does-not-match, but table sim2real-probe-ddb-policy-e530a509 has no associated policy.', got 'No resource-based policy with the expected revision id is attached to arn:aws:dynamodb:us-west-2:000000000000:table/sim2real-probe-ddb-policy-e530a509'.
Status and error code match in all three clusters; only the message content diverges, and in causes 2 and 3 it drops request-derived values.
Model: smithy_query(operation=GetResourcePolicy,PutResourcePolicy,DeleteResourcePolicy) -> ResourceArn required (ResourceArnString 1..1283), ExpectedRevisionId: PolicyRevisionId 1..255, error PolicyNotFoundException client/400; smithy_query(doc, shape=PolicyNotFoundException) -> documents both the absent-policy and the revision-mismatch case.

## Steps to Reproduce
1. Replay ddbctl-resource-policy-deny-vs-not-found#task#13: GetResourcePolicy for a table ARN with no resource-based policy -> 400 PolicyNotFoundException whose message is 'No resource-based policy is attached to <arn>' instead of 'Resource-based policy not found for the provided ResourceArn: <arn>'.
2. Replay dynamodb-resource-policy-lifecycle#task#7: DeleteResourcePolicy with ExpectedRevisionId set to a value that does not match the stored policy's revision -> 400 whose message names neither the requested nor the current revision id.
3. Replay dynamodb-resource-policy-management#task#9: PutResourcePolicy with ExpectedRevisionId against a table that has no policy -> 400 with the same generic message as step 2, so the two conditions are indistinguishable.
4. Re-replay all three fixture steps (plus the other members of divergence 9's and 28's clusters) to confirm the fix.
