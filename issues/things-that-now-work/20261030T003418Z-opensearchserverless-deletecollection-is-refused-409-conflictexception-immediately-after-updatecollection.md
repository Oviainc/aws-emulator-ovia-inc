Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearchserverless.delete-collection-deletion-protection-rejected
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: opensearchserverless|DeleteCollection|ConflictException
Issue-Commit: 67ed68e040d396203b765d4dc2b0964aebb19c3c

# DeleteCollection is refused 409 ConflictException immediately after UpdateCollection turns deletion protection off, so a protected collection can never be deleted

> _Note: this issue was identified against the code base at commit `67ed68e040d396203b765d4dc2b0964aebb19c3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCollection

## Description
REOPENS 20261029T191222Z-opensearchserverless-10-net-new-causes-in-the-refusal.md: its fix left the call refused.

That issue's Cause 2 reworded DeleteCollection's deletion-protection refusal, and that part now matches: the ValidationException for a protected collection is accepted by the check. The same check now fails one call later, on the RESTORE step: the DeleteCollection that AWS accepts once protection is disabled is refused.

Cause 1 of 1: a metadata-only UpdateCollection parks the collection in a status DeleteCollection refuses from.

AWS: Smithy: UpdateCollection takes id + deletionProtection and answers updateCollectionDetail; DeleteCollection models ConflictException (409), whose own model documentation scopes it to "When deleting a resource, thrown when the resource is not in the ACTIVE, FAILED, or UPDATE_FAILED state". Recorded behavior: after UpdateCollection(id=<id>, deletionProtection='DISABLED') returns, the IMMEDIATELY following DeleteCollection(id=<id>) is accepted with HTTP 200 and deleteCollectionDetail.status == 'DELETING', and the collection then purges (BatchGetCollection reports collectionErrorDetails errorCode NOT_FOUND, errorMessage 'The specified Collection is not found.'). A deletionProtection/description change is metadata only and does not leave the collection in a state from which a delete is refused.

Emulator: UpdateCollection applies the change and then calls record.begin_update(updated_at), moving the collection into UPDATING; DeleteCollection's DELETABLE_STATUSES gate then refuses the next call with ConflictException (409) and the message "Collection cannot be deleted because it is not in 'ACTIVE', 'FAILED', or 'UPDATE_FAILED' status." (that text is entirely fixed, built from the handler's own status list). The refusal surfaces to the caller as botocore.errorfactory.ConflictException, uncaught, because the check expects that call to succeed.

Why: the documented and only way to remove a deletion-protected collection — disable protection, then delete — is unavailable. A client following AWS's sequence gets a 409 whose message blames the collection's status, with nothing telling it how long the self-imposed UPDATING window lasts, so protected collections leak and any teardown/cleanup path that AWS completes in two calls never completes here.

Fix: the UpdateCollection handler's state transition and DeleteCollection's status gate in the opensearchserverless crate — a metadata-only update must not leave the collection in a status its sibling DeleteCollection refuses from (either do not enter UPDATING for such an update, or settle it before the handler returns).

Traces: opensearchserverless.delete-collection-deletion-protection-rejected.

## Evidence
Expected (recorded against AWS): update_collection(id=<protected id>, deletionProtection='DISABLED') then delete_collection(id=<protected id>) -> HTTP 200, deleteCollectionDetail.id == <protected id>, deleteCollectionDetail.status == 'DELETING', then BatchGetCollection reports [{'id': <id>, 'errorCode': 'NOT_FOUND', 'errorMessage': 'The specified Collection is not found.'}].
Actual (emulator): botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the DeleteCollection operation: Collection cannot be deleted because it is not in 'ACTIVE', 'FAILED', or 'UPDATE_FAILED' status.
Source: services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/update_collection.rs:262 (record.begin_update(updated_at)) and services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/delete_collection.rs:200-210 (the DELETABLE_STATUSES gate that emits the quoted message). The whole quoted message is fixed text; nothing in it comes from the request.

## Steps to Reproduce
1. opensearchserverless.create_collection(name=<n>, type='SEARCH', encryptionConfig={'aWSOwnedKey': True}, deletionProtection='ENABLED').
2. Poll batch_get_collection(ids=[<id>]) until status == 'ACTIVE'.
3. delete_collection(id=<id>) -> ValidationException (deletion protection) — as AWS does.
4. update_collection(id=<id>, deletionProtection='DISABLED').
5. delete_collection(id=<id>) -> expected HTTP 200 with status 'DELETING'; observed 409 ConflictException "Collection cannot be deleted because it is not in 'ACTIVE', 'FAILED', or 'UPDATE_FAILED' status."
