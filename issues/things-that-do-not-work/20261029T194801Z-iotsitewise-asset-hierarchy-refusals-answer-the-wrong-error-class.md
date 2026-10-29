Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.associate-assets-duplicate-rejected iotsitewise.associate-assets-concurrent-duplicate-race iotsitewise.disassociate-assets-never-associated-rejected iotsitewise.delete-asset-conflict-when-hierarchy-associated-rejected
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.associate-assets-duplicate-rejected|run#10|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}"
Issue-Key: iotsitewise.associate-assets-concurrent-duplicate-race|run#14|AssertionError|assert all(error in (in_progress, duplicate) for error in failures), (
Issue-Key: iotsitewise.disassociate-assets-never-associated-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iotsitewise.delete-asset-conflict-when-hierarchy-associated-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# Asset-hierarchy refusals answer the wrong error class and emulator-authored messages (AssociateAssets, DisassociateAssets, DeleteAsset)

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateAssets

## Description
Four distinct root causes in the asset-hierarchy association/deletion refusal paths. One of them returns the WRONG modeled error class (so a client's exception handling branches differently), the other three return the right class with a message the emulator authored itself. This issue bundles 4 causes; all four must be closed.

Cause 1 of 4: a duplicate AssociateAssets is refused as an in-flight operation conflict instead of the duplicate-parent rejection.
AWS: Smithy: AssociateAssets declares InvalidRequestException (httpError 400) and ConflictingOperationException (httpError 409), each with a `message` member. Re-associating a (assetId, hierarchyId, childAssetId) triple that is ALREADY associated — the association having already settled, so no operation is in flight — is refused with InvalidRequestException / HTTP 400 and the message "Child asset already has a parent". That message is entirely fixed text: it carries no variable parts, and in particular no ids.
Emulator: answers ConflictingOperationException (409) with "An operation associating the asset <childAssetId> with the asset <assetId> through the hierarchy <hierarchyId> is already in progress." — i.e. it treats a settled duplicate as an in-flight operation conflict. The emulator has no duplicate-association check at all: there is no code path that can produce AWS's 400 for this request.
Why: a client that catches InvalidRequestException (a terminal, caller-fault condition) sees ConflictingOperationException instead, which callers are documented to RETRY; the caller therefore retries forever against a condition that will never clear, and never learns the child already has a parent.
Fix: the AssociateAssets handler's validation — resolve the (assetId, hierarchyId, childAssetId) triple against already-stored associations and the child's existing parent BEFORE the in-flight/conflict gate, and refuse a settled duplicate with InvalidRequestException and AWS's fixed message.
Traces: iotsitewise.associate-assets-duplicate-rejected (code assertion: expected 'InvalidRequestException', got 'ConflictingOperationException'); also the dominant failure mode in iotsitewise.associate-assets-concurrent-duplicate-race.

Cause 2 of 4: AssociateAssets' ConflictingOperationException message is the emulator's own sentence.
AWS: Smithy: AssociateAssets error ConflictingOperationException, httpError 409, member message. Where AWS does answer a concurrent racer with a 409 on this operation, the message is "A hierarchy with ID <hierarchyId> already exists" — fixed text "A hierarchy with ID " and " already exists", with the hierarchyId taken from the request body. (The surviving alternative AWS may answer a losing racer is Cause 1's InvalidRequestException/"Child asset already has a parent"; the recording shows those two and nothing else.)
Emulator: every losing racer of five concurrent identical AssociateAssets calls gets "An operation associating the asset <childAssetId> with the asset <assetId> through the hierarchy <hierarchyId> is already in progress." — a sentence this service does not produce, with three interpolated ids AWS's message does not carry.
Why: the 409 is the one refusal a correct client is expected to interpret (retry vs. give up) under contention; matching on AWS's wording fails, and log-based diagnosis attributes the failure to a phantom in-flight operation.
Fix: the AssociateAssets handler's conflict refusal must emit the service's hierarchy-scoped 409 message, not the crate's generic "operation ... is already in progress" template.
Traces: iotsitewise.associate-assets-concurrent-duplicate-race.

Cause 3 of 4: DisassociateAssets on a never-associated triple reports the association state instead of the hierarchy not-found message.
AWS: Smithy: DisassociateAssets error ResourceNotFoundException, httpError 404, member message; input members assetId (httpLabel), hierarchyId and childAssetId (body, all @required). For a (parent, hierarchy, child) triple that was never associated, AWS refuses 404 with "A hierarchy with ID <hierarchyId> doesn't exist" — fixed text "A hierarchy with ID " and " doesn't exist" (no trailing period), variable part: the hierarchyId from the request body.
Emulator: answers the right code (ResourceNotFoundException) but the message "Asset with ID <childAssetId> is not associated with the hierarchy with ID <hierarchyId> of asset with ID <parentAssetId>" — the emulator's own three-id sentence.
Why: a client or test asserting on the documented refusal text fails, and the message describes a different condition (an association check) than the resource-lookup failure AWS reports, so callers build the wrong recovery (they look for the association rather than for the hierarchy id).
Fix: the DisassociateAssets handler's not-found refusal must render the hierarchy-scoped message with only the hierarchyId interpolated.
Traces: iotsitewise.disassociate-assets-never-associated-rejected.

Cause 4 of 4: DeleteAsset's child-association refusal is the emulator's own sentence and interpolates the asset id.
AWS: Smithy: DeleteAsset error InvalidRequestException, httpError 400, member message. Deleting an asset that still has children associated under one of its hierarchies is refused 400 with "You must disassociate the asset from all of its children assets before you can delete it." — the message is entirely FIXED text including the trailing period, and carries NO variable parts (notably not the asset id).
Emulator: answers the right code with "The asset <assetId> has child assets associated with its hierarchies. Disassociate them before deleting it." — different wording and an interpolated asset id AWS's message does not contain.
Why: message-equality assertions and client-side error classification on the documented text fail; the emulator's sentence is not the contract.
Fix: the DeleteAsset handler's child-association precondition refusal must emit AWS's fixed sentence verbatim.
Traces: iotsitewise.delete-asset-conflict-when-hierarchy-associated-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from mainline: 46/60 passed, all 14 failures in iotsitewise. The four checks below are the ones grounding this issue (the ids in each quoted message are that run's values filling the templates; which parts are fixed text and which come from the request is stated per cause above).

1) iotsitewise.associate-assets-duplicate-rejected — AssociateAssets repeated on an already-associated triple. Expected code 'InvalidRequestException' with message 'Child asset already has a parent'; got code 'ConflictingOperationException'. AssertionError: expected InvalidRequestException, got 'ConflictingOperationException'.

2) iotsitewise.associate-assets-concurrent-duplicate-race — five concurrent identical AssociateAssets calls; a losing racer must get ('ConflictingOperationException', 'A hierarchy with ID <hierarchyId> already exists') or ('InvalidRequestException', 'Child asset already has a parent'). All five losers got ('ConflictingOperationException', 'An operation associating the asset <childAssetId> with the asset <parentAssetId> through the hierarchy <hierarchyId> is already in progress.').

3) iotsitewise.disassociate-assets-never-associated-rejected — DisassociateAssets on a never-associated triple. Expected ResourceNotFoundException message "A hierarchy with ID <hierarchyId> doesn't exist"; got 'Asset with ID <childAssetId> is not associated with the hierarchy with ID <hierarchyId> of asset with ID <parentAssetId>'.

4) iotsitewise.delete-asset-conflict-when-hierarchy-associated-rejected — DeleteAsset on a parent with an associated child. Expected InvalidRequestException message 'You must disassociate the asset from all of its children assets before you can delete it.'; got 'The asset <assetId> has child assets associated with its hierarchies. Disassociate them before deleting it.'

Model grounding: AssociateAssets declares ConflictingOperationException (409), InvalidRequestException (400), ResourceNotFoundException (404); DisassociateAssets declares ConflictingOperationException (409), InvalidRequestException (400), ResourceNotFoundException (404); DeleteAsset declares ConflictingOperationException (409), InvalidRequestException (400), ResourceNotFoundException (404).

## Steps to Reproduce
1. Re-replay behavioral check iotsitewise.associate-assets-duplicate-rejected: create a parent model with a hierarchy over a child model, create one parent asset and two child assets, AssociateAssets the first child, then issue the IDENTICAL AssociateAssets again — observe HTTP 409 ConflictingOperationException with the 'already in progress' sentence instead of HTTP 400 InvalidRequestException 'Child asset already has a parent'.
2. Re-replay iotsitewise.associate-assets-concurrent-duplicate-race: fire several identical AssociateAssets calls concurrently — observe every loser carries the emulator's 'An operation associating ... is already in progress.' message rather than 'A hierarchy with ID <hierarchyId> already exists'.
3. Re-replay iotsitewise.disassociate-assets-never-associated-rejected: POST /assets/{assetId}/disassociate for a (parent, hierarchy, child) triple that was never associated — observe the 404 message names the association state and three ids instead of "A hierarchy with ID <hierarchyId> doesn't exist".
4. Re-replay iotsitewise.delete-asset-conflict-when-hierarchy-associated-rejected: associate a child, then DELETE /assets/{parentAssetId} — observe the 400 message is the emulator's sentence with the asset id interpolated.
