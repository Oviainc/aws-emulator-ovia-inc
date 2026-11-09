Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.blob-id-is-content-addressed-git-sha1
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit.blob-id-is-content-addressed-git-sha1|run#16|AssertionError|assert message == expected_message, (
Issue-Commit: 0cee419aadfb4868f9ccd3aa23bc5045ac08a29b

# CodeCommit GetBlob's BlobIdDoesNotExistException message is an emulator-authored sentence that names no blob id

> _Note: this issue was identified against the code base at commit `0cee419aadfb4868f9ccd3aa23bc5045ac08a29b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBlob

## Description
One net-new cause: GetBlob rejects an absent blob id with the right status and the right error code but its own sentence.

Cause 1 of 1: BlobIdDoesNotExistException carries the wrong message text and drops the blob id.
AWS: Smithy (com-amazonaws-codecommit-codecommit-20150413) GetBlob takes repositoryName and blobId (both @required, blobId targets ObjectId) and models BlobIdDoesNotExistException (errorKind client, httpError 400 [defaulted]). Recorded behavior supplies the wire message: for a well-formed git object id that was never stored in the repository, AWS answers 400 BlobIdDoesNotExistException with 'AWS CodeCommit could not find the blob <blobId> in the repository.' — 'AWS CodeCommit could not find the blob ' and ' in the repository.' are FIXED text and the one variable part is the blobId copied verbatim from the request.
Emulator: the same status (400) and the same error code, but message 'The specified blob does not exist in the repository.' — a different sentence that names no value (it is the shape's documentation-style prose, not AWS's wire message).
Why: the message is the only part of this refusal that identifies WHICH id failed. A caller resolving several blob ids in one pass (a diff walker, a mirror, an audit tool) cannot tell from the emulator's reply which id was missing, and any client or test asserting AWS's sentence fails.
Fix: the GetBlob handler's absent-blob path in the codecommit service (services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/get_blob.rs) — build the message from the requested blobId instead of returning the fixed doc-style sentence.
Traces: codecommit.blob-id-is-content-addressed-git-sha1 (the check's earlier legs — PutFile blobId equals the git blob SHA-1, GetFile/GetBlob round trip the bytes — all pass; only this message diverges).

Note: no open issue carries this check's replay key. Open issue 20261102T130134Z-codecommit-content-read-and-commit-write-errors-carry.md files the same doc-string-as-message pattern for GetDifferences/GetCommit/PutFile/CreateCommit, but not for GetBlob's BlobIdDoesNotExistException and not for this check.

## Evidence
Check codecommit.blob-id-is-content-addressed-git-sha1 (mismatch): "AssertionError: expected message 'AWS CodeCommit could not find the blob ad4f7fb7ef7d7576b0f811e7716bc33c25276467 in the repository.', got 'The specified blob does not exist in the repository.'". The error code assertion before it (code == 'BlobIdDoesNotExistException') passed, so status and code are already right; the blob id in the expected text is the per-run id the check computed and is the template's variable part, not a literal requirement.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cc.create_repository(repositoryName=R); cc.put_file(repositoryName=R, branchName='main', filePath='f', fileContent=b'x', commitMessage='m').
3. Compute the git blob object id of bytes that were never committed (sha1 of b'blob <len>\\x00' + content) and call cc.get_blob(repositoryName=R, blobId=<that id>).
4. Expect 400 BlobIdDoesNotExistException with message 'AWS CodeCommit could not find the blob <that id> in the repository.'; observe 'The specified blob does not exist in the repository.'
5. Re-replay codecommit.blob-id-is-content-addressed-git-sha1.
