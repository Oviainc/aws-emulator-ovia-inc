Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.disassociate-assets-never-associated-rejected iotsitewise.concurrent-asset-model-updates-etag-race iotsitewise.update-asset-model-etag-precondition-mismatch-rejected iotsitewise.update-asset-model-precondition-etag-mismatch-rejected iotsitewise.delete-asset-model-ifnonematch-active-version-rejected iotsitewise.delete-asset-model-conflict-when-asset-exists-rejected iotsitewise.delete-asset-model-with-asset-conflict-rejected iotsitewise.too-many-tags-rejected iotsitewise.asset-lifecycle-creating-to-active-with-model iotsitewise.describe-asset-nonexistent-not-found iotsitewise.describe-asset-not-found iotsitewise.asset-model-lifecycle-creating-to-active iotsitewise.asset-model-lifecycle-reaches-active iotsitewise.create-asset-duplicate-external-id-rejected
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.disassociate-assets-never-associated-rejected|run#8|AssertionError|assert code == "ResourceNotFoundException", f"unexpected code: {code}"
Issue-Key: iotsitewise.concurrent-asset-model-updates-etag-race|run#3|KeyError|etag = sw.describe_asset_model(assetModelId=model_id)["eTag"]
Issue-Key: iotsitewise.update-asset-model-etag-precondition-mismatch-rejected|run#5|KeyError|current_etag = described0["eTag"]
Issue-Key: iotsitewise.update-asset-model-precondition-etag-mismatch-rejected|run#4|KeyError|etag0 = original["eTag"]
Issue-Key: iotsitewise.delete-asset-model-ifnonematch-active-version-rejected|_expect_rejected#3|AssertionError|raise AssertionError(f"expected the conditional delete to be rejected, but it SUCCEEDED: {result!r}")
Issue-Key: iotsitewise.delete-asset-model-conflict-when-asset-exists-rejected|run#5|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: iotsitewise.delete-asset-model-with-asset-conflict-rejected|run#5|AssertionError|assert error.get("Code") == "InvalidRequestException", (
Issue-Key: iotsitewise.too-many-tags-rejected|run#6|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r} ({message!r})"
Issue-Key: iotsitewise.asset-lifecycle-creating-to-active-with-model|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iotsitewise.describe-asset-nonexistent-not-found|run#11|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: iotsitewise.describe-asset-not-found|run#13|AssertionError|assert message == f"Asset with ID {bogus_id} not found", f"unexpected message: {message!r}"
Issue-Key: iotsitewise.asset-model-lifecycle-creating-to-active|run#19|AssertionError|assert describe_message == expected_message, (
Issue-Key: iotsitewise.asset-model-lifecycle-reaches-active|run#16|AssertionError|assert final_message == expected_message, f"expected message {expected_message!r}, got {final_message!r}"
Issue-Key: iotsitewise.create-asset-duplicate-external-id-rejected|run#12|AssertionError|assert message == "Another asset is already using the same externalId", f"unexpected message: {message!r}"
Issue-Commit: 00a15b53a34d04728621f3d09bcc33a2157eb48f

# IoT SiteWise control plane: 6 divergences — DisassociateAssets answers 501, DescribeAssetModel omits eTag, DeleteAssetModel ignores conditional headers and misclassifies the dependent-asset conflict, tag-count violation uses an unmodeled error, not-found/already-exists messages built by the wrong rule

> _Note: this issue was identified against the code base at commit `00a15b53a34d04728621f3d09bcc33a2157eb48f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAssetModel

## Description
Six independent root causes in the iotsitewise control plane, all grounded in failing behavioral checks of this suite. Each must be closed before this issue is resolved.

=== Cause 1 of 6: DisassociateAssets is an unimplemented scaffold stub ===
AWS: Smithy: DisassociateAssets, POST /assets/{assetId}/disassociate (host prefix api.), required members assetId/hierarchyId/childAssetId; modeled errors ConflictingOperationException (409), InvalidRequestException (400), ResourceNotFoundException (404), ThrottlingException (429). Recorded behavior: DisassociateAssets on a (parent, hierarchyId, child) triple that was never associated is refused with 404 ResourceNotFoundException and changes nothing; after associating the same triple the identical call succeeds (recorded message example: "A hierarchy with ID <hierarchyId> doesn't exist").
Emulator: every DisassociateAssets call returns the scaffold refusal — error code `NotImplemented` with message "operation DisassociateAssets is scaffolded but not implemented in this emulator".
Why: a client can never remove a child from an asset hierarchy, and the negative path returns an error code that is not in the operation's modeled error set, so SDK `except ResourceNotFoundException` handlers never match and generic retry logic sees an unknown failure instead of a 404.
Fix: the DisassociateAssets handler and the hierarchy-association state it mutates (services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/disassociate_assets.rs).
Traces: iotsitewise.disassociate-assets-never-associated-rejected.

=== Cause 2 of 6: DescribeAssetModel response omits the modeled eTag member ===
AWS: Smithy: DescribeAssetModelResponse.eTag -> ETag, bound as httpHeader "ETag", pattern ^[\w-]{43}$. The same model binds UpdateAssetModel/DeleteAssetModel ifMatch -> If-Match (ETag) and declares PreconditionFailedException (httpError 412), so the describe-read ETag is the token the conditional-write path consumes.
Emulator: DescribeAssetModel returns no eTag at all — clients reading `described["eTag"]` fail with KeyError 'eTag'.
Why: optimistic concurrency against an asset model is impossible: a client cannot obtain the ETag it must echo in If-Match, so every conditional update/delete degrades to an unconditional one and lost-update protection silently disappears.
Fix: the DescribeAssetModel handler/serializer plus the asset-model state, which must carry a 43-character ETag and rotate it on each mutation.
Traces: iotsitewise.concurrent-asset-model-updates-etag-race, iotsitewise.update-asset-model-etag-precondition-mismatch-rejected, iotsitewise.update-asset-model-precondition-etag-mismatch-rejected.

=== Cause 3 of 6: DeleteAssetModel ignores the conditional request headers and performs the delete ===
AWS: Smithy: DeleteAssetModel.ifNoneMatch -> httpHeader If-None-Match (pattern ^\*$), DeleteAssetModel.matchForVersionType -> httpHeader Match-For-Version-Type, and the operation models PreconditionFailedException with httpError 412 (required members message, resourceId, resourceArn). Recorded behavior: DeleteAssetModel with If-None-Match: * and Match-For-Version-Type: ACTIVE against a model that DOES have an ACTIVE version is refused 412 PreconditionFailedException (recorded message example: "Precondition failed for provided If-None-Match value.") and the model survives in state ACTIVE; the identical call without the header succeeds (202, state DELETING).
Emulator: the conditional call is accepted — HTTP 202 with {assetModelId, assetModelStatus: {state: DELETING}} — i.e. the precondition is never evaluated.
Why: a client using the conditional header as a guard against deleting a model that is already live loses exactly the resource the guard exists to protect; the delete is destructive and unrecoverable.
Fix: precondition evaluation in the DeleteAssetModel handler (If-None-Match / Match-For-Version-Type) and the 412 PreconditionFailedException serialization.
Traces: iotsitewise.delete-asset-model-ifnonematch-active-version-rejected.

=== Cause 4 of 6: DeleteAssetModel with a live dependent asset returns the wrong error class ===
AWS: Smithy: DeleteAssetModel models BOTH InvalidRequestException (httpError 400) and ConflictingOperationException (httpError 409). Recorded behavior: while an asset instantiated from the model still exists, DeleteAssetModel is refused with 400 InvalidRequestException (recorded message example: "Asset model had assets when delete was called"); once the dependent asset is deleted the same call succeeds.
Emulator: the same call is refused with ConflictingOperationException (409).
Why: the two classes mean different things to a client — 409 ConflictingOperationException signals a transient operation conflict that callers and SDK/waiter logic retry, while 400 InvalidRequestException is a terminal request error that must make the caller delete the dependent assets first. The emulator turns a terminal precondition into an infinite retry loop.
Fix: the dependency check in the DeleteAssetModel handler and its error mapping.
Traces: iotsitewise.delete-asset-model-conflict-when-asset-exists-rejected, iotsitewise.delete-asset-model-with-asset-conflict-rejected.

=== Cause 5 of 6: tag-count constraint violation answered with an unmodeled ValidationException ===
AWS: Smithy: TagResourceRequest.tags is required with @length min 1 / max 50; TagResource's modeled errors are ConflictingOperationException(409), InternalFailureException(500), InvalidRequestException(400), LimitExceededException(410), ResourceNotFoundException(404), ThrottlingException(429), TooManyTagsException(400), UnauthorizedException(401) — `ValidationException` is NOT a shape of com.amazonaws.iotsitewise at all. Recorded behavior: TagResource with 51 tags is refused with InvalidRequestException and the request-shape validation message "1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50", and no tags are attached.
Emulator: refuses with error code `ValidationException` (message observed: "1 validation error detected: Value '51 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50").
Why: the error code is not in the service's model, so no generated SDK exception class exists for it; `except InvalidRequestException` never fires and the caller sees an unmodeled error for a plain input-shape violation.
Fix: the request-validation layer for this service must raise the service's own InvalidRequestException for @length/@range violations instead of a generic ValidationException envelope.
Traces: iotsitewise.too-many-tags-rejected.

=== Cause 6 of 6: not-found and already-exists messages are built by the wrong rule ===
AWS: both error messages are built from the request. ResourceNotFoundException (Smithy: httpError 404, required member message) carries the requested identifier in the form "Asset with ID <assetId> not found" for asset reads/deletes and "Asset Model with ID <assetModelId> not found" for asset-model reads; CreateAsset with an assetExternalId already in use is refused with ResourceAlreadyExistsException (Smithy: modeled on CreateAsset, httpError 409) naming the colliding dimension — recorded message "Another asset is already using the same externalId". The identifier-bearing form is recorded consistently across five independent checks; the externalId text is recorded behavior only.
Emulator: emits its own templates instead — "The asset <id> could not be found.", "The asset model <id> could not be found.", and for the duplicate external id "The asset <id> already exists." where <id> is the generated assetId of the rejected CreateAsset rather than any statement about the externalId collision, so the message names the wrong resource and the wrong dimension.
Why: the error codes are right but the message rule is not: callers and operators that parse or log the rejection reason get a template AWS never emits, and the duplicate-externalId rejection points at an asset id that does not exist, sending diagnosis after the wrong resource.
Fix: error construction for the iotsitewise not-found and already-exists paths (asset, asset model, and the CreateAsset external-id uniqueness check) — the message must be derived from the requested identifier/dimension as AWS does.
Traces: iotsitewise.asset-lifecycle-creating-to-active-with-model, iotsitewise.describe-asset-nonexistent-not-found, iotsitewise.describe-asset-not-found, iotsitewise.asset-model-lifecycle-creating-to-active, iotsitewise.asset-model-lifecycle-reaches-active, iotsitewise.create-asset-duplicate-external-id-rejected.

## Evidence
Cause 1 — expected ResourceNotFoundException from DisassociateAssets, got: AssertionError: unexpected code: NotImplemented ("operation DisassociateAssets is scaffolded but not implemented in this emulator").
Cause 2 — KeyError: 'eTag' raised on DescribeAssetModel output in three checks (described0["eTag"], original["eTag"]).
Cause 3 — AssertionError: expected the conditional delete to be rejected, but it SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'assetModelId': '<id>', 'assetModelStatus': {'state': 'DELETING'}}; AWS: 412 PreconditionFailedException, model stays ACTIVE.
Cause 4 — AssertionError: expected InvalidRequestException, got ConflictingOperationException; and "deleting an asset model with a live asset returned 'ConflictingOperationException', expected 'InvalidRequestException'".
Cause 5 — AssertionError: expected InvalidRequestException, got 'ValidationException' ("1 validation error detected: Value '51 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50").
Cause 6 — AssertionError: expected message 'Asset with ID <id> not found', got 'The asset <id> could not be found.'; expected 'Asset Model with ID <id> not found', got 'The asset model <id> could not be found.'; expected 'Another asset is already using the same externalId', got 'The asset <id> already exists.'

## Steps to Reproduce
1. Boot the emulator and point a botocore iotsitewise client at it.
2. Cause 1: create two active asset models (parent with a hierarchy to the child model) and one asset each, then call DisassociateAssets(assetId=parent, hierarchyId, childAssetId=child) for a triple never associated -> observe NotImplemented instead of 404 ResourceNotFoundException.
3. Cause 2: CreateAssetModel, wait for ACTIVE, DescribeAssetModel -> response has no eTag member.
4. Cause 3: CreateAssetModel, wait ACTIVE, DeleteAssetModel(assetModelId, ifNoneMatch="*", matchForVersionType="ACTIVE") -> 202 DELETING instead of 412 PreconditionFailedException.
5. Cause 4: CreateAssetModel (with a measurement property), CreateAsset from it, wait ACTIVE, DeleteAssetModel -> ConflictingOperationException (409) instead of InvalidRequestException (400).
6. Cause 5: TagResource(resourceArn=<assetModelArn>, tags={51 entries}) -> ValidationException instead of InvalidRequestException.
7. Cause 6: DescribeAsset/DeleteAsset on an unknown asset id, DescribeAssetModel on a deleted model id, and CreateAsset reusing an assetExternalId already held by another asset -> compare Error.Message against the AWS templates above.
