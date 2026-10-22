Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: verifiedpermissions.concurrent-create-policy-store-alias-race verifiedpermissions.create-policy-store-alias-conflict-while-pending-deletion-rejected verifiedpermissions.tag-resource-too-many-tags-rejected
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions.concurrent-create-policy-store-alias-race|run#14|AssertionError|assert all(error == expected_error for _, _, error in failures), (
Issue-Key: verifiedpermissions.create-policy-store-alias-conflict-while-pending-deletion-rejected|run#11|AssertionError|assert error == expected_error, f"expected {expected_error!r}, got {error!r}"
Issue-Key: verifiedpermissions.tag-resource-too-many-tags-rejected|run#10|AssertionError|assert error == expected_error, (
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# ConflictException and TooManyTagsException carry emulator-authored descriptive messages instead of the service's fixed generic messages

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicyStoreAlias

## Description
Two root causes, both about an exception body the emulator composes itself where the recordings show the service emitting a fixed, request-independent string. Status codes and error codes are correct in every case here; what diverges is the message the service is recorded as producing.

Cause 1 of 2: ConflictException on CreatePolicyStoreAlias.
AWS: Smithy — ConflictException is {message (required), resources (required, list of ResourceConflict{resourceId, resourceType})}, httpError 409, doc "The request failed because another request to modify a resource occurred at the same time." Recorded behavior: two independent and quite different conflict causes — (a) eight racers claiming one alias name for eight different policy stores, and (b) recreating an alias name that is currently in PendingDeletion — both produce the IDENTICAL message "Conflict occurred during the processing of this request", with no store id, alias name or state in it. The message is therefore fixed for this exception and not derived from the request; the conflicting resource is conveyed through the required `resources` member instead. Status 409 / code ConflictException are the hard requirements; the fixed message is the rule the two recordings establish.
Emulator: composes a per-cause sentence that embeds the alias name and the conflicting store id or state, e.g. 'the policy store alias policy-store-alias/<name> cannot be created because it is already associated with policy store PS...' and 'the policy store alias policy-store-alias/<name> cannot be created because it is in the PendingDeletion state'. Everything else in both checks passed: exactly one racer won, the other seven were rejected with ConflictException, and the alias resolved to the winner in state Active.
Why: a client that branches on the conflict message (or asserts on it in a fixture) diverges, and because the specifics are packed into prose the required `resources` list is the natural place they should appear — a client reading ConflictException.resources to learn which store it collided with gets nothing structured.
Fix: the shared ConflictException constructor for the policy-store-alias paths — emit the service's fixed message and put the colliding alias/store into the required `resources` list as ResourceConflict entries.
Traces: verifiedpermissions.concurrent-create-policy-store-alias-race, verifiedpermissions.create-policy-store-alias-conflict-while-pending-deletion-rejected.

Cause 2 of 2: TooManyTagsException on TagResource.
AWS: Smithy — TooManyTagsException is {message, resourceName (AmazonResourceName, 1..2048)}, httpError 400, doc "No more tags be added because the limit (50) has been reached. To add new tags, use UntagResource to remove existing tags." Recorded behavior: tagging a 51st distinct key onto a policy store already holding 50 tags is refused 400 TooManyTagsException with the fixed message "The request exceeds the maximum number of user tags allowed" — it names neither the count nor the key, and the offending resource is carried in `resourceName`.
Emulator: returns the right status and code but a computed sentence: 'the request would leave this resource with 51 tags, over the limit of 50; use UntagResource to remove existing tags'. The rest of the behavior was correct — the 51st key did not land, re-valuing an existing key still succeeded, and freeing a slot let the same key land.
Why: same as above — the message is the part a client matches on, and the structured `resourceName` member is where the resource belongs.
Fix: the TagResource tag-count validation's error construction — fixed message plus `resourceName` set to the request's resourceArn.
Traces: verifiedpermissions.tag-resource-too-many-tags-rejected.

## Evidence
Cause 1a (concurrent-create-policy-store-alias-race): expected every loser to fail with ('ConflictException', 'Conflict occurred during the processing of this request'); got 7x ('ConflictException', 'the policy store alias policy-store-alias/sim2real-behav-aliasrace-<id> cannot be created because it is already associated with policy store PS<id>'). Winner count (1) and alias resolution/state (Active) were correct.
Cause 1b (create-policy-store-alias-conflict-while-pending-deletion-rejected): expected ('ConflictException', 'Conflict occurred during the processing of this request'); got ('ConflictException', 'the policy store alias policy-store-alias/sim2real-behav-alias-<id> cannot be created because it is in the PendingDeletion state').
Cause 2 (tag-resource-too-many-tags-rejected): expected ('TooManyTagsException', 'The request exceeds the maximum number of user tags allowed'); got ('TooManyTagsException', 'the request would leave this resource with 51 tags, over the limit of 50; use UntagResource to remove existing tags'). HTTP 400 and the code matched; the 51st tag correctly did not land.
Note the cross-check grounding: two structurally different conflict causes produced byte-identical messages in the recordings, which is what establishes the message as fixed rather than request-derived.

## Steps to Reproduce
Cause 1a: create 8 policy stores; from 8 threads call CreatePolicyStoreAlias(aliasName='policy-store-alias/<unique>', policyStoreId=<a different store per thread>) concurrently. Inspect the (Code, Message) of the 7 losers.
Cause 1b: create two policy stores; CreatePolicyStoreAlias(alias, storeA); DeletePolicyStoreAlias(alias) with a soft delete so it enters PendingDeletion; CreatePolicyStoreAlias(alias, storeB). Inspect the (Code, Message).
Cause 2: CreatePolicyStore -> GetPolicyStore to read arn -> TagResource(arn, 50 distinct keys) -> TagResource(arn, {'k50': 'v50'}). Inspect the (Code, Message) and the exception's resourceName member.
