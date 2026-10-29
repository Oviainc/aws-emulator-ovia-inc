Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearchserverless.create-collection-without-encryption-config-or-policy-rejected opensearchserverless.server-side-default-encryption-enforced-without-policy opensearchserverless.delete-collection-deletion-protection-rejected opensearchserverless.get-nonexistent-security-config-resourcenotfound opensearchserverless.security-config-lifecycle-transitions opensearchserverless.tag-resource-on-missing-resource-not-found-rejected opensearchserverless.untag-resource-after-delete-blocked opensearchserverless.tag-unicode-value-round-trip opensearchserverless.delete-collection-group-nonempty-rejected opensearchserverless.delete-collection-group-with-collections-blocked opensearchserverless.update-access-policy-optimistic-lock-stale-version-rejected opensearchserverless.update-access-policy-concurrent-version-conflict opensearchserverless.update-lifecycle-policy-optimistic-lock-stale-version-rejected opensearchserverless.update-security-config-optimistic-lock-stale-version-rejected opensearchserverless.update-security-policy-optimistic-lock-stale-version-rejected opensearchserverless.access-policy-empty-principal-malformed-rejected opensearchserverless.update-account-settings-out-of-range-rejected opensearchserverless.update-collection-group-capacity-limits-validated
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: opensearchserverless.create-collection-without-encryption-config-or-policy-rejected|run#13|AssertionError|assert message == f"No matching security policy of encryption type found for collection name: {bare_name}. Please create security policy of encryption type for
Issue-Key: opensearchserverless.server-side-default-encryption-enforced-without-policy|run#7|AssertionError|assert error_message == expected_message, f"expected message {expected_message!r}, got {error_message!r}"
Issue-Key: opensearchserverless.delete-collection-deletion-protection-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: opensearchserverless.get-nonexistent-security-config-resourcenotfound|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: opensearchserverless.security-config-lifecycle-transitions|run#24|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: opensearchserverless.tag-resource-on-missing-resource-not-found-rejected|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: opensearchserverless.untag-resource-after-delete-blocked|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: opensearchserverless|TagResource|ResourceNotFoundException
Issue-Key: opensearchserverless.delete-collection-group-nonempty-rejected|run#11|AssertionError|assert code == "ValidationException", f"unexpected error code: {code!r}"
Issue-Key: opensearchserverless.delete-collection-group-with-collections-blocked|run#16|AssertionError|assert code == "ValidationException", f"unexpected error code: {code!r}"
Issue-Key: opensearchserverless.update-access-policy-optimistic-lock-stale-version-rejected|run#18|AssertionError|assert code == "ValidationException", (
Issue-Key: opensearchserverless.update-access-policy-concurrent-version-conflict|run#16|AssertionError|assert code == "ValidationException", (
Issue-Key: opensearchserverless.update-lifecycle-policy-optimistic-lock-stale-version-rejected|run#14|AssertionError|assert code == "ValidationException", (
Issue-Key: opensearchserverless.update-security-config-optimistic-lock-stale-version-rejected|run#14|AssertionError|assert code == "ValidationException", (
Issue-Key: opensearchserverless.update-security-policy-optimistic-lock-stale-version-rejected|run#14|AssertionError|assert code == "ValidationException", (
Issue-Key: opensearchserverless.access-policy-empty-principal-malformed-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: opensearchserverless.update-account-settings-out-of-range-rejected|_error_code#5|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: opensearchserverless.update-collection-group-capacity-limits-validated|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# opensearchserverless: 10 net-new causes in the refusal plane — stale-version and non-empty-group refusals use ConflictException where AWS uses ValidationException, two validations are missing entirely, and six refusal messages are reworded

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateSecurityPolicy

## Description
TEN net-new root causes in this crate's refusal plane, all grounded in failing behavioral checks of the opensearchserverless service token. Each cause is independent and must be closed on its own.

=== Cause 1 of 10: CreateCollection's missing-encryption refusal is reworded ===
AWS: Smithy: CreateCollection has an optional encryptionConfig and models ValidationException (400). Recorded behavior: with neither encryptionConfig nor an encryption security policy covering the name, the refusal message is `No matching security policy of encryption type found for collection name: <name>. Please create security policy of encryption type for this collection.` — fixed text except `<name>`, taken from the request's `name`.
Emulator: `No encryption policy or encryption configuration exists for the collection <name>: specify encryptionConfig in the request, or create an encryption security policy whose rules cover collection/<name>`.
Why: the code is the generic ValidationException, so the message is the only discriminator a client has; a client matching AWS's text does not recognise the refusal.
Fix: the CreateCollection handler's encryption validation message.
Traces: opensearchserverless.create-collection-without-encryption-config-or-policy-rejected, opensearchserverless.server-side-default-encryption-enforced-without-policy.

=== Cause 2 of 10: DeleteCollection's deletion-protection refusal is reworded and names the id instead of the name ===
AWS: Smithy: DeleteCollection models ValidationException (400); input is the collection `id`. Recorded behavior: message `Collection <name> cannot be deleted because deletion protection is enabled. Disable deletion protection before deleting the collection.` — fixed text with `<name>` the collection's NAME from state.
Emulator: `Collection with ID <id> has deletion protection enabled and cannot be deleted`.
Why: wrong fixed text and the wrong variable (id rather than name), so neither a human nor a matcher sees AWS's refusal.
Fix: the DeleteCollection handler's deletion-protection refusal message.
Traces: opensearchserverless.delete-collection-deletion-protection-rejected.

=== Cause 3 of 10: the security-config not-found message is reworded ===
AWS: Smithy: GetSecurityConfig / UpdateSecurityConfig model ResourceNotFoundException (404) and take `id`. Recorded behavior: message `Security config with id: <id> does not exist` — note the colon after 'id' and the 'does not exist' tail; `<id>` is the request's id (e.g. saml/<account>/<name>).
Emulator: `Security config with id <id> is not found`.
Why: a client keying on AWS's message text mis-classifies a genuine 404.
Fix: the security-config read/update handlers' not-found message in this crate.
Traces: opensearchserverless.get-nonexistent-security-config-resourcenotfound, opensearchserverless.security-config-lifecycle-transitions.

=== Cause 4 of 10: the tagging plane's not-found message names the ARN instead of the resource kind and id ===
AWS: Smithy: TagResource/UntagResource take resourceArn and model ResourceNotFoundException (404). Recorded behavior: the message names the resource KIND and its ID, not the ARN — `Collection Group ID <id> is not found` for a collection-group ARN and `Collection ID <id> is not found` for a collection ARN; `<id>` is the id parsed out of the request's resourceArn.
Emulator: `Resource arn:aws:aoss:<region>:<account>:collection-group/<id> is not found` for the collection-group case and `Collection with ID <id> is not found` for the collection case.
Why: both the fixed text and (for the group case) the variable rendered differ, so the refusal is unrecognisable to a client matching AWS.
Fix: the not-found refusal message of the tagging handlers (TagResource/UntagResource) in this crate.
Traces: opensearchserverless.tag-resource-on-missing-resource-not-found-rejected, opensearchserverless.untag-resource-after-delete-blocked.

=== Cause 5 of 10: TagResource cannot resolve a LIVE collection-group ARN, so tagging a collection group is impossible ===
AWS: Smithy: TagResource takes resourceArn (any aoss resource ARN) + tags and models ResourceNotFoundException only for a resource that does not exist. Recorded behavior: TagResource on the ARN of a collection group that CreateCollectionGroup just returned succeeds (200) and ListTagsForResource then reports the tag, including a value with multi-byte UTF-8 preserved byte-for-byte.
Emulator: `ResourceNotFoundException: Resource arn:aws:aoss:<region>:<account>:collection-group/<id> is not found` for a collection group that exists.
Why: no tag can ever be written to a collection group, so tag round-trip, tag-based cost allocation and any Unicode-preservation guarantee are simply unavailable on that resource type.
Fix: the tagging plane's ARN resolution in this crate — resolve collection-group (and every other taggable) ARN to its stored record, not only collection ARNs. NOTE: open issue 20261028T185156Z-opensearchserverless-tagresource-wrong-behavior.md describes this same resolution gap from a static read; this check was not marked owned by it and is recorded here so it is re-replayed, and the cause should close with that issue if its fix lands first.
Traces: opensearchserverless.tag-unicode-value-round-trip.

=== Cause 6 of 10: deleting a collection group that still has member collections is refused with the wrong error class ===
AWS: Smithy: DeleteCollectionGroup models both ConflictException (409) and ValidationException (400). Recorded behavior: when the group still has collections, the refusal is ValidationException (400) with `CollectionGroup with ID <id>, has collections associated and cannot be deleted` — fixed text (including the comma after the id) with `<id>` the request's group id.
Emulator: ConflictException (409).
Why: the HTTP status and the error code both differ, so a client's error handling takes the wrong branch — a 409 is widely treated as retryable, while this condition never clears without deleting the member collections.
Fix: the DeleteCollectionGroup handler's non-empty-group precondition — classify it as ValidationException with that message.
Traces: opensearchserverless.delete-collection-group-nonempty-rejected, opensearchserverless.delete-collection-group-with-collections-blocked.

=== Cause 7 of 10: every optimistic-lock (stale version) refusal is ConflictException where AWS uses ValidationException ===
AWS: Smithy: UpdateAccessPolicy / UpdateSecurityPolicy / UpdateLifecyclePolicy / UpdateSecurityConfig all require a policyVersion/configVersion and model both ConflictException (409) and ValidationException (400). Recorded behavior: an already-consumed (stale) version token is refused ValidationException (400) with `Policy version specified in the request refers to an older version and policy has since changed` (entirely fixed text), and the rejected update leaves the resource untouched; a MALFORMED version token is refused ValidationException with the standard `1 validation error detected: Value '<value>' at 'policyVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: ([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))? ` form.
Emulator: ConflictException for the stale-version case on all four update operations, and the same for the concurrent-update loser.
Why: a client implementing read-modify-write against AWS's contract treats ValidationException as 'refetch the version and rebuild the request' and a 409 Conflict as a retry-the-same-request condition; with a 409 it retries the identical stale-version request forever.
Fix: the stale-version comparison in the four policy/config update handlers — refuse with ValidationException (400) and the message above.
Traces: opensearchserverless.update-access-policy-optimistic-lock-stale-version-rejected, opensearchserverless.update-access-policy-concurrent-version-conflict, opensearchserverless.update-lifecycle-policy-optimistic-lock-stale-version-rejected, opensearchserverless.update-security-config-optimistic-lock-stale-version-rejected, opensearchserverless.update-security-policy-optimistic-lock-stale-version-rejected.

=== Cause 8 of 10: CreateAccessPolicy accepts a data policy whose only statement has an EMPTY Principal list ===
AWS: Smithy: CreateAccessPolicy requires `policy` (PolicyDocument, length 1..20480) and models ValidationException (400). Recorded behavior: a statement with `"Principal": []` is refused ValidationException with `Policy json is invalid, error: [$[0].Principal: there must be a minimum of 1 items in the array]` — fixed text whose variable part is the JSON path of the offending member (`$[<statement index>].Principal`) — and GetAccessPolicy for that name then answers ResourceNotFoundException `Policy with name <name> and type data is not found`.
Emulator: HTTP 200 with an accessPolicyDetail echoing `'policy': [{'Principal': [], 'Rules': [...]}]` — the malformed policy is stored.
Why: a policy that grants nothing to nobody is persisted and counts against the account's policy quota, and a caller that mistakenly ships an empty Principal gets no signal at all; worse, the stored policy looks authoritative when the data plane is later consulted.
Fix: the CreateAccessPolicy handler's policy-document validation (and the same validation on UpdateAccessPolicy).
Traces: opensearchserverless.access-policy-empty-principal-malformed-rejected.

=== Cause 9 of 10: UpdateAccountSettings accepts an indexing capacity above the account-wide OCU quota ===
AWS: Smithy: UpdateAccountSettings takes capacityLimits (CapacityLimits.maxIndexingCapacityInOCU @range min 2) and models ServiceQuotaExceededException (402) alongside ValidationException (400). Recorded behavior: a maxIndexingCapacityInOCU above the non-adjustable account ceiling of 1700 OCU is refused ServiceQuotaExceededException with a message of the fixed form `The requested total indexing capacity of <requested>.0 OCUs exceeds your maximum indexing capacity service quota limit of 1700 OCUs. Current settings: <n> OCUs for collections that are not part of any collection group, <m> OCUs for collection groups. New request of <requested> OCUs would bring total to <total>.0 OCUs.` (the limit 1700 is fixed; the requested value comes from the request and the current/total OCU figures from account state), and the stored settings are left unchanged.
Emulator: HTTP 200 and the out-of-quota value is stored — GetAccountSettings reports maxIndexingCapacityInOCU as the requested over-quota number.
Why: the account-wide quota is unenforced, so a caller can drive the account into a state real AWS refuses and never sees the 402 its retry/alerting logic is written against.
Fix: the UpdateAccountSettings handler's quota validation and the account-settings state it writes.
Traces: opensearchserverless.update-account-settings-out-of-range-rejected.

=== Cause 10 of 10: UpdateCollectionGroup's invalid-capacity-range message is reworded ===
AWS: Smithy: UpdateCollectionGroup takes CollectionGroupCapacityLimits (float members min/maxIndexingCapacityInOCU, min/maxSearchCapacityInOCU) and models ValidationException (400). Recorded behavior: when the minimum exceeds the maximum, the message is `Invalid capacity range: minIndexingCapacityInOCU (<min>) exceeds maxIndexingCapacityInOCU (<max>)` — fixed text with the two values from the request rendered as floats (the members are modeled float).
Emulator: `capacityLimits.minIndexingCapacityInOCU (<min>) must not exceed capacityLimits.maxIndexingCapacityInOCU (<max>)`, with the values rendered as integers.
Why: different fixed text and a different numeric rendering of the modeled float members, so the refusal does not match what a client parses or logs.
Fix: the UpdateCollectionGroup handler's capacity-range validation message.
Traces: opensearchserverless.update-collection-group-capacity-limits-validated.

## Evidence
Cause 1 — expected 'No matching security policy of encryption type found for collection name: <name>. Please create security policy of encryption type for this collection.'; got 'No encryption policy or encryption configuration exists for the collection sim2real-aoss-enc-bare-<suffix>: specify encryptionConfig in the request, or create an encryption security policy whose rules cover collection/sim2real-aoss-enc-bare-<suffix>' (the name is the per-run value; the surrounding text is the defect).
Cause 2 — expected 'Collection <name> cannot be deleted because deletion protection is enabled. Disable deletion protection before deleting the collection.'; got 'Collection with ID <id> has deletion protection enabled and cannot be deleted'.
Cause 3 — expected 'Security config with id: <id> does not exist'; got 'Security config with id <id> is not found'.
Cause 4 — expected 'Collection Group ID <id> is not found' / 'Collection ID <id> is not found'; got 'Resource arn:aws:aoss:<region>:<account>:collection-group/<id> is not found' / 'Collection with ID <id> is not found'.
Cause 5 — expected TagResource 200 on a live collection group's ARN; got botocore.errorfactory.ResourceNotFoundException 'Resource arn:aws:aoss:<region>:<account>:collection-group/<id> is not found'.
Cause 6 — expected error code ValidationException (400) with 'CollectionGroup with ID <id>, has collections associated and cannot be deleted'; got 'ConflictException' (409).
Cause 7 — expected 'ValidationException' with 'Policy version specified in the request refers to an older version and policy has since changed' for a stale (already-consumed) policyVersion/configVersion; got 'ConflictException' on all four update operations and on the concurrent-update loser.
Cause 8 — expected ValidationException "Policy json is invalid, error: [$[0].Principal: there must be a minimum of 1 items in the array]"; got HTTP 200 and {'accessPolicyDetail': {'type': 'data', 'name': <name>, 'policyVersion': <v>, 'policy': [{'Principal': [], 'Rules': [...]}], ...}}.
Cause 9 — expected ServiceQuotaExceededException (402) with 'The requested total indexing capacity of <requested>.0 OCUs exceeds your maximum indexing capacity service quota limit of 1700 OCUs. ...' and unchanged settings; got HTTP 200 and {'accountSettingsDetail': {'capacityLimits': {'maxIndexingCapacityInOCU': 1701, 'maxSearchCapacityInOCU': 4}}}.
Cause 10 — expected 'Invalid capacity range: minIndexingCapacityInOCU (32.0) exceeds maxIndexingCapacityInOCU (16.0)'; got 'capacityLimits.minIndexingCapacityInOCU (32) must not exceed capacityLimits.maxIndexingCapacityInOCU (16)'.

## Steps to Reproduce
Cause 1: create_collection(name=<n>, type='SEARCH') with no encryptionConfig and no encryption policy covering <n>.
Cause 2: create_collection(deletionProtection='ENABLED'), wait ACTIVE, delete_collection(id=<id>).
Cause 3: get_security_config(id='saml/<account>/<never-created>'); also reached by get_security_config after delete_security_config.
Cause 4: tag_resource(resourceArn='arn:aws:aoss:<region>:<account>:collection-group/<never-created-id>', tags=[...]); untag_resource on a collection ARN whose collection was deleted.
Cause 5: create_collection_group(name=<g>), then tag_resource(resourceArn=<the group's ARN>, tags=[{'key':'k','value':'<multi-byte UTF-8>'}]).
Cause 6: create_collection_group(name=<g>), create_collection(collectionGroupName=<g>), wait until the group reports numberOfCollections>=1, then delete_collection_group(id=<g id>).
Cause 7: create_security_policy / create_access_policy / create_lifecycle_policy / create_security_config, update once to consume version v1, then re-issue the update with v1 again.
Cause 8: create_access_policy(type='data', name=<n>, policy=json.dumps([{'Rules':[{'ResourceType':'index','Resource':['index/<n>*/*'],'Permission':['aoss:ReadDocument']}],'Principal':[]}])).
Cause 9: update_account_settings(capacityLimits={'maxIndexingCapacityInOCU': 1701, 'maxSearchCapacityInOCU': 4}) then get_account_settings.
Cause 10: create_collection_group(name=<g>), update_collection_group(id=<g id>, capacityLimits={'minIndexingCapacityInOCU': 32, 'maxIndexingCapacityInOCU': 16, ...}).
