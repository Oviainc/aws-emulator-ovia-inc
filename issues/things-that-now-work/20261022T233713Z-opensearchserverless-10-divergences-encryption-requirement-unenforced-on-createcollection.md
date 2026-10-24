Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearchserverless.create-collection-without-encryption-config-or-policy-rejected opensearchserverless.server-side-default-encryption-enforced-without-policy opensearchserverless.collection-deletion-grace-period-before-purge opensearchserverless.delete-collection-deletion-protection-rejected opensearchserverless.create-collection-duplicate-name-conflict-rejected opensearchserverless.create-collection-malformed-name-validation-rejected opensearchserverless.update-security-policy-optimistic-lock-stale-version-rejected opensearchserverless.create-index-nonexistent-collection-rejected opensearchserverless.untag-resource-after-delete-blocked opensearchserverless.update-collection-concurrent-modification-resolves-consistently opensearchserverless.deploy-collection-cfn-becomes-active opensearchserverless.deploy-security-policy-cfn-enforced
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: opensearchserverless.create-collection-without-encryption-config-or-policy-rejected|_error#11|AssertionError|raise AssertionError(f"expected this call to be refused, but it succeeded with {result!r}")
Issue-Key: opensearchserverless.server-side-default-encryption-enforced-without-policy|_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: opensearchserverless.collection-deletion-grace-period-before-purge|run#12|AssertionError|assert len(immediate_details) == 1, (
Issue-Key: opensearchserverless.delete-collection-deletion-protection-rejected|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: opensearchserverless.create-collection-duplicate-name-conflict-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: opensearchserverless.create-collection-malformed-name-validation-rejected|run#5|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: opensearchserverless.update-security-policy-optimistic-lock-stale-version-rejected|run#9|AssertionError|assert message == _MALFORMED_VERSION_MESSAGE, (
Issue-Key: opensearchserverless.create-index-nonexistent-collection-rejected|run#4|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: opensearchserverless.untag-resource-after-delete-blocked|run#13|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: opensearchserverless.update-collection-concurrent-modification-resolves-consistently|run#10|AssertionError|assert payload == "ConflictException", (
Issue-Key: opensearchserverless|BatchGetCollection|ValidationException
Issue-Key: opensearchserverless|GetSecurityPolicy|ResourceNotFoundException
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# opensearchserverless: 10 divergences — encryption requirement unenforced on CreateCollection, deletion purges with no DELETING window, protected-delete miscoded, CreateIndex/UntagResource/UpdateCollection answer 501, and CFN Collection/SecurityPolicy resources are wrong

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCollection

## Description
This issue bundles TEN distinct root causes in the opensearchserverless emulator, grounded in 12 failing behavioral checks. Each cause must be closed before submitting.

Cause 1 of 10: CreateCollection accepts a collection that has neither an encryptionConfig in the request nor a matching encryption security policy.
AWS: Smithy CreateCollection models encryptionConfig as OPTIONAL but declares ValidationException (HTTP 400) among its errors; AWS requires an encryption setting to come from somewhere — the request's encryptionConfig or a pre-existing encryption security policy whose Rules match collection/<name>. Recorded behavior: the call is refused with ValidationException (400) and a message naming encryption.
Emulator: returns HTTP 200 with createCollectionDetail (status CREATING, kmsKeyArn "auto") for a CreateCollection carrying only name/type/tags, with no encryption policy present in the account.
Why: a client gets a collection that no encryption policy ever covered, and the server-side encryption precondition is silently optional — a template or script that forgets the policy looks valid against the emulator and then fails on AWS.
Fix: CreateCollection's validation/handler (services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/create_collection.rs), cross-checking the stored encryption security policies in this service's state before admitting the collection.
Traces: opensearchserverless.create-collection-without-encryption-config-or-policy-rejected, opensearchserverless.server-side-default-encryption-enforced-without-policy.

Cause 2 of 10: DeleteCollection purges the collection instantly instead of leaving it visible in DELETING.
AWS: DeleteCollection returns deleteCollectionDetail with status DELETING, and the collection remains readable through BatchGetCollection in DELETING until it is purged; only after the purge does BatchGetCollection report it as a NOT_FOUND collectionErrorDetails entry and ListCollections omit it (recorded behavior; Smithy models BatchGetCollection's collectionDetails/collectionErrorDetails split for exactly this).
Emulator: immediately after DeleteCollection, BatchGetCollection(ids=[id]) returns an empty collectionDetails list — the resource is already gone.
Why: any client that polls deletion progress (or asserts the DELETING state exists) sees the resource vanish in one step, so waiters and status-driven orchestration cannot observe deletion at all.
Fix: DeleteCollection's handler and the collection state machine (mark DELETING, purge on a later transition), plus BatchGetCollection's read path so a DELETING collection is still returned.
Traces: opensearchserverless.collection-deletion-grace-period-before-purge.

Cause 3 of 10: DeleteCollection against a deletionProtection=ENABLED collection is refused with the wrong error code.
AWS: Smithy DeleteCollection errors are ConflictException (409), ResourceNotFoundException (404), InternalServerException (500) and ValidationException (400); recorded behavior for the deletion-protection refusal is ValidationException (HTTP 400).
Emulator: answers ConflictException instead.
Why: the two codes mean opposite things to a client — ConflictException reads as a transient state conflict worth retrying, so a client loops on a permanent, configuration-driven refusal instead of disabling deletion protection.
Fix: the deletion-protection branch of DeleteCollection's handler, choosing the modeled error variant.
Traces: opensearchserverless.delete-collection-deletion-protection-rejected.

Cause 4 of 10: the duplicate-collection-name conflict message is not AWS's form and leaks a generated id.
AWS: ConflictException (409) with a message built from the request's name member: "A collection with name '<name>' already exists." (example from the recording).
Emulator: right code, message "Collection with name <name> already exists with id 8sh3tqaaaaaaaaaaaaaaaaaa" — a different template that also exposes an internally generated collection id.
Why: the message is derived from the request, so its rule is known; clients and tests that match on it break, and the leaked id is environment-specific noise AWS never emits here.
Fix: the conflict-message construction in CreateCollection's handler.
Traces: opensearchserverless.create-collection-duplicate-name-conflict-rejected.

Cause 5 of 10: constraint-violation messages report the modeled regex WITH its ^/$ anchors.
AWS: the validation message is built from the request — "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — and the pattern is rendered UNANCHORED. Recorded examples: CollectionName -> [a-z][a-z0-9_-]+ ; policyVersion -> ([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))? . Smithy declares CollectionName pattern ^[a-z][a-z0-9-]+$ and PolicyVersion pattern ^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$.
Emulator: emits the model pattern verbatim including anchors ("...pattern: ^[a-z][a-z0-9-]+$", "...pattern: ^([0-9a-zA-Z+/]{4})*...$"). The code and status (ValidationException, 400) are correct.
Why: clients and conformance tests that parse the reported constraint out of the message receive a pattern AWS never produces; the anchors (and, for CollectionName, the differing character class) make the reported rule not match the service's own.
Fix: the shared constraint-violation message formatter in the validation layer (strip the anchors when rendering a @pattern trait into the message), not in either operation handler.
Traces: opensearchserverless.create-collection-malformed-name-validation-rejected, opensearchserverless.update-security-policy-optimistic-lock-stale-version-rejected.

Cause 6 of 10: CreateIndex is an unimplemented scaffold stub and answers 501 NotImplemented.
AWS: Smithy CreateIndex (input id/CollectionId pattern ^[a-z0-9]{3,40}$, indexName, indexSchema) returns 200 on success and models ConflictException (409), InternalServerException (500), ResourceNotFoundException (404) and ValidationException (400); against a collection id that does not exist it is refused with ResourceNotFoundException (recorded message example: "The Collection with Id '00000000000000000000' does not exist."), and against a real ACTIVE collection it succeeds and the schema round-trips through GetIndex.
Emulator: HTTP 501 with code NotImplemented, "operation CreateIndex is scaffolded but not implemented in this emulator".
Why: no index can be created in a serverless collection, and the not-found rejection class is unobservable — a client cannot distinguish "bad collection id" from "server has no such feature".
Fix: implement the handler in services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/create_index.rs (the Err(Scaffolded::not_implemented("CreateIndex")) at line 164) and drop the SCAFFOLDED marker.
Traces: opensearchserverless.create-index-nonexistent-collection-rejected.

Cause 7 of 10: UntagResource is an unimplemented scaffold stub and answers 501 NotImplemented.
AWS: Smithy UntagResource (resourceArn, tagKeys) returns 200 with an empty body on success and models ConflictException (409), InternalServerException (500), ResourceNotFoundException (404), ValidationException (400); against an already-deleted collection ARN it is refused with ResourceNotFoundException (recorded message example: "Collection ID <id> is not found").
Emulator: HTTP 501 NotImplemented, "operation UntagResource is scaffolded but not implemented in this emulator".
Why: tags can never be removed, and an over-permissive-vs-not-found distinction on a deleted resource cannot be exercised at all.
Fix: implement the handler in services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/untag_resource.rs (the not_implemented return at line 161) and drop the SCAFFOLDED marker.
Traces: opensearchserverless.untag-resource-after-delete-blocked.

Cause 8 of 10: UpdateCollection is an unimplemented scaffold stub and answers 501 NotImplemented.
AWS: Smithy UpdateCollection (id, description, vectorOptions, deletionProtection, clientToken) returns 200 with updateCollectionDetail and models ConflictException (409), InternalServerException (500), ValidationException (400); concurrent writers either both succeed (last write wins, ordered by the service's own lastModifiedDate) or the loser is refused with ConflictException — never another code.
Emulator: HTTP 501 NotImplemented, "operation UpdateCollection is scaffolded but not implemented in this emulator".
Why: a collection's description, vector options and deletionProtection can never be changed, so no update or concurrency behavior exists; it also makes the deletion-protection restore path (Cause 3) unusable.
Fix: implement the handler in services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/update_collection.rs and drop the SCAFFOLDED marker.
Traces: opensearchserverless.update-collection-concurrent-modification-resolves-consistently.

Cause 9 of 10: a CloudFormation AWS::OpenSearchServerless::Collection resolves {"Ref"} to the collection NAME instead of the collection Id.
AWS: Ref on that resource yields the collection's Id, which satisfies CollectionId's pattern ^[a-z0-9]{3,40}$, so BatchGetCollection(ids=[Ref]) returns exactly one collectionDetails entry with status ACTIVE and the declared name.
Emulator: the stack output carries the collection name (example: 'sim2real-cfncoll-10e103bf4234'), and BatchGetCollection then refuses it with ValidationException (400): "1 validation error detected: Value 'sim2real-cfncoll-10e103bf4234' at 'ids.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9]{3,40}$".
Why: every template that passes a collection Ref into another resource or output feeds an id-shaped parameter a name, so cross-resource wiring and post-deploy verification fail even though the stack reports CREATE_COMPLETE.
Fix: the CloudFormation resource provider / physical-resource-id mapping for AWS::OpenSearchServerless::Collection (Ref must return the created collection's id).
Traces: opensearchserverless.deploy-collection-cfn-becomes-active.

Cause 10 of 10: a CloudFormation AWS::OpenSearchServerless::SecurityPolicy does not actually create the policy in the service.
AWS: once the stack reaches CREATE_COMPLETE, GetSecurityPolicy(type='encryption', name=<Name>) returns securityPolicyDetail whose policy document equals the template's Policy, and a same-stack collection declaring no EncryptionConfig of its own reaches ACTIVE because the policy is enforced.
Emulator: after CREATE_COMPLETE, GetSecurityPolicy answers ResourceNotFoundException — example message "Policy with name sim2real-secpol-678dfebfb960 and type encryption is not found" — so the stack reports success for a resource that does not exist in the service state.
Why: CloudFormation-driven setup of encryption/network policies silently produces nothing; any flow that relies on a stack-created policy (including a stack-created collection depending on it) is built on a resource the data plane cannot see.
Fix: the CloudFormation resource provider for AWS::OpenSearchServerless::SecurityPolicy must create the policy through the opensearchserverless service state (and delete it on stack delete).
Traces: opensearchserverless.deploy-security-policy-cfn-enforced.

## Evidence
Cause 1 — expected ValidationException (400) mentioning encryption; actual HTTP 200: {'createCollectionDetail': {'id': '7qygd7aaaaaaaaaaaaaaaaaa', 'name': 'sim2real-aoss-enc-bare-...', 'status': 'CREATING', 'type': 'SEARCH', 'kmsKeyArn': 'auto', ...}} (same for the default-encryption check: id '8sh3tqaaaaaaaaaaaaaaaaaa', 200).
Cause 2 — expected one collectionDetails entry in DELETING immediately after DeleteCollection; actual: AssertionError: collection should still be visible immediately after DeleteCollection, got [].
Cause 3 — expected ValidationException; actual 'ConflictException'.
Cause 4 — expected message "A collection with name 'sim2real-dupcol-<id>' already exists."; actual 'Collection with name sim2real-dupcol-<id> already exists with id 8sh3tqaaaaaaaaaaaaaaaaaa'.
Cause 5 — expected "...pattern: [a-z][a-z0-9_-]+"; actual "...pattern: ^[a-z][a-z0-9-]+$". Expected "...pattern: ([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?"; actual the same pattern wrapped in ^...$.
Cause 6 — expected ResourceNotFoundException for CreateIndex against a nonexistent collection id; actual 'NotImplemented' (HTTP 501, "operation CreateIndex is scaffolded but not implemented in this emulator").
Cause 7 — expected ResourceNotFoundException for UntagResource on a deleted collection ARN; actual 'NotImplemented'.
Cause 8 — expected a losing concurrent UpdateCollection to fail with ConflictException (or both calls to succeed); actual 'NotImplemented'.
Cause 9 — expected BatchGetCollection(ids=[Ref Collection]) -> 1 ACTIVE detail; actual botocore.errorfactory.ValidationException: 1 validation error detected: Value 'sim2real-cfncoll-10e103bf4234' at 'ids.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9]{3,40}$.
Cause 10 — expected GetSecurityPolicy to return the stack-created encryption policy; actual botocore.errorfactory.ResourceNotFoundException: Policy with name sim2real-secpol-678dfebfb960 and type encryption is not found.

## Steps to Reproduce
Boot the emulator and point botocore at it; aoss = client('opensearchserverless'), cfn = client('cloudformation').
1 (Cause 1): aoss.create_collection(name='sim2real-enc-bare', type='SEARCH') with NO encryption security policy in the account -> expect ValidationException (400) naming encryption; observe 200 CREATING.
2 (Cause 2): create a collection with encryptionConfig={'aWSOwnedKey': True}, wait ACTIVE, aoss.delete_collection(id=...), then immediately aoss.batch_get_collection(ids=[id]) -> expect 1 detail in DELETING; observe [].
3 (Cause 3): create a collection with deletionProtection='ENABLED', wait ACTIVE, aoss.delete_collection(id=...) -> expect ValidationException; observe ConflictException.
4 (Cause 4): create two collections with the same name -> expect ConflictException with "A collection with name '<name>' already exists."; observe the id-leaking message.
5 (Cause 5): aoss.create_collection(name='1-invalid-collection-name', ...) and aoss.update_security_policy(policyVersion='not-a-valid-version-token!!', ...) -> expect the reported regex unanchored; observe ^...$.
6 (Cause 6): aoss.create_index(id='0'*20, indexName='sim2real-idx', indexSchema={...}) -> expect ResourceNotFoundException; observe 501 NotImplemented.
7 (Cause 7): tag a collection, delete it, then aoss.untag_resource(resourceArn=<deleted arn>, tagKeys=['team']) -> expect ResourceNotFoundException; observe 501 NotImplemented.
8 (Cause 8): aoss.update_collection(id=<active id>, description='x') -> expect 200 updateCollectionDetail; observe 501 NotImplemented.
9 (Cause 9): deploy a stack with AWS::OpenSearchServerless::Collection and Outputs {CollectionId: {Ref: Collection}}, then aoss.batch_get_collection(ids=[output]) -> expect 1 ACTIVE detail; observe the output is the name and BatchGetCollection rejects it.
10 (Cause 10): deploy a stack with AWS::OpenSearchServerless::SecurityPolicy (Type=encryption) and, after CREATE_COMPLETE, aoss.get_security_policy(type='encryption', name=<Name>) -> expect the policy; observe ResourceNotFoundException.
Re-replay the 12 check ids in trace_ids.
