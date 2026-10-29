Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-branch-and-commit-conflict-errors
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-branch-and-commit-conflict-errors codecommit-branch-and-commit-workflow codecommit-branch-isolation-blob-diff codecommit-branch-merge-strategies codecommit-create-pull-request-error-paths codecommit-delete-repository-and-verify-gone codecommit-file-folder-blob-content-operations codecommit-get-blob-differences-error-paths codecommit-get-blob-differences-errors codecommit-multi-file-directory-differences codecommit-put-file-parent-commit-errors codecommit-repo-binary-file-diff codecommit-repo-lifecycle-with-commit-and-branch codecommit-repository-creation-error-paths codecommit-repository-triggers-and-tagging codecommit-merge-pull-request-validation-and-not-found-errors codecommit-pull-request-comment-error-paths codecommit-repository-lifecycle-and-metadata
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|CreateRepository|2xx->2xx|$.repositoryMetadata.kmsKeyId
Issue-Key: codecommit|GetRepository|2xx->2xx|$.repositoryMetadata.kmsKeyId
Issue-Key: codecommit|UpdateRepositoryEncryptionKey|2xx->2xx|$.kmsKeyId
Issue-Key: codecommit|UpdateRepositoryEncryptionKey|2xx->2xx|$.originalKmsKeyId
Issue-Key: codecommit|BatchGetRepositories|2xx->2xx|$.repositories[*].cloneUrlHttp
Issue-Key: codecommit|BatchGetRepositories|2xx->2xx|$.repositories[*].cloneUrlSsh
Issue-Key: codecommit|BatchGetRepositories|2xx->2xx|$.repositories[*].kmsKeyId
Issue-Key: codecommit|ListTagsForResource|2xx->2xx|$.tagrisId
Issue-Commit: 170d008231b721c2b3436293a90aa925526f50a2

# CodeCommit repository metadata omits kmsKeyId, returns a bare key id instead of a key ARN, and drops tagrisId (5 causes)

> _Note: this issue was identified against the code base at commit `170d008231b721c2b3436293a90aa925526f50a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
FIVE net-new causes in repository metadata serialization: the encryption-key members of RepositoryMetadata are never populated (or are populated in the wrong form), and two further response members are dropped. A fixer must close ALL FIVE before submitting.

=== Cause 1 of 5: CreateRepository omits repositoryMetadata.kmsKeyId ===
AWS: Smithy RepositoryMetadata.kmsKeyId (target KmsKeyId, pattern ^[a-zA-Z0-9:/_-]+$) documented as 'The ID of the Key Management Service encryption key used to encrypt and decrypt the repository.' Every CodeCommit repository is encrypted, so the member is ALWAYS present; recorded behavior for a repository created without an explicit key shows the AWS-managed default alias 'alias/aws/codecommit'.
Emulator: the member is absent from repositoryMetadata entirely.
Why: a client reading the key a new repository is encrypted with (to audit it, or to pass it to UpdateRepositoryEncryptionKey) gets null and cannot distinguish 'default AWS-managed key' from 'unknown'. This single cause affects 15 recorded steps, so most repository-creating workflows see it.
Fix: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_repository.rs plus the RepositoryMetadata projection in the crate's state module — default the stored key to the service alias and always serialize it.
Traces: codecommit-branch-and-commit-conflict-errors#setup#1 (plus 14 more setup/task steps across codecommit-branch-and-commit-workflow, -branch-isolation-blob-diff, -branch-merge-strategies, -create-pull-request-error-paths, -delete-repository-and-verify-gone, -file-folder-blob-content-operations, -get-blob-differences-error-paths, -get-blob-differences-errors, -multi-file-directory-differences, -put-file-parent-commit-errors, -repo-binary-file-diff, -repo-lifecycle-with-commit-and-branch, -repository-creation-error-paths, -repository-triggers-and-tagging).

=== Cause 2 of 5: GetRepository omits repositoryMetadata.kmsKeyId ===
AWS: returns the repository's current encryption key in the same member, in the form it was set (the key ARN after UpdateRepositoryEncryptionKey, the service alias otherwise).
Emulator: absent.
Why: as Cause 1, on the read path every client uses.
Fix: src/ops/get_repository.rs / the shared RepositoryMetadata projection.
Traces: codecommit-merge-pull-request-validation-and-not-found-errors#task#6 (plus 4 more steps in codecommit-pull-request-comment-error-paths and codecommit-repository-lifecycle-and-metadata).

=== Cause 3 of 5: UpdateRepositoryEncryptionKey returns a bare key id and omits originalKmsKeyId ===
AWS: Smithy UpdateRepositoryEncryptionKeyOutput has repositoryId, kmsKeyId and originalKmsKeyId. Recorded behavior: kmsKeyId is the FULL key ARN ('arn:aws:kms:<region>:<account>:key/<keyId>') and originalKmsKeyId carries the key the repository used BEFORE the call.
Emulator: kmsKeyId is the bare key UUID with no ARN wrapper, and originalKmsKeyId is absent.
Why: a client that stores the returned kmsKeyId and feeds it to KMS or compares it with a key ARN from kms:DescribeKey never matches; and without originalKmsKeyId it cannot roll the change back, which is the member's whole purpose.
Fix: src/ops/update_repository_encryption_key.rs — serialize the key in ARN form and record/return the previous key.
Traces: codecommit-repository-lifecycle-and-metadata#task#8.

=== Cause 4 of 5: BatchGetRepositories key form/presence is inconsistent and the list order does not follow the request ===
AWS: returns one RepositoryMetadata per requested name, each carrying kmsKeyId in ARN form, in the order recorded for the request's repositoryNames.
Emulator: one entry carries the bare key UUID, the other omits kmsKeyId altogether, and the two entries come back in the opposite order from the recording.
Why: a client that zips the response against the repositoryNames it sent attributes each repository's metadata to the wrong repository, and the key form problem of Cause 3 reappears here. NOTE: the cloneUrlHttp/cloneUrlSsh host and port also differ, but that is the local environment's endpoint and is NOT part of this finding.
Fix: src/ops/batch_get_repositories.rs — project kmsKeyId through the shared metadata projection and emit entries in request order.
Traces: codecommit-repository-lifecycle-and-metadata#task#10.

=== Cause 5 of 5: ListTagsForResource omits the tagrisId member ===
AWS: ListTagsForResourceOutput models only tags/nextToken, so this is recorded behavior only: the recorded responses carry an additional top-level `tagrisId` member (a generated id for the tagging record) alongside tags.
Emulator: the member is absent.
Why: clients that round-trip the full response body (recording proxies, SDK-level response comparison) see a structurally different document. The id's VALUE is generated per run and is not a requirement — only the member's presence is.
Fix: src/ops/list_tags_for_resource.rs — emit the member with the repository's generated tagging id.
Traces: codecommit-repository-triggers-and-tagging#task#5 (also #task#7).

## Evidence
Cause 1 — status 200 as expected; expected $.repositoryMetadata.kmsKeyId 'alias/aws/codecommit', actual missing. 15 steps clustered.
Cause 2 — status 200 as expected; expected $.repositoryMetadata.kmsKeyId present (the repository's current key), actual missing. 5 steps clustered.
Cause 3 — status 200 as expected; expected $.kmsKeyId 'arn:aws:kms:<region>:<account>:key/<keyId>' and $.originalKmsKeyId present; actual $.kmsKeyId '<keyId>' (bare UUID) and $.originalKmsKeyId missing.
Cause 4 — status 200 as expected; expected $.repositories[i].kmsKeyId in ARN form for both entries and the entries in the recorded request order; actual one bare UUID, one None, and the two entries transposed. (cloneUrl host/port differences in the same step are environment-specific and excluded.)
Cause 5 — status 200 as expected; expected a top-level $.tagrisId member, actual missing; its value is a per-run generated id.

## Steps to Reproduce
1. Replay codecommit-branch-and-commit-conflict-errors#setup#1 (CreateRepository) and check $.repositoryMetadata.kmsKeyId.
2. Replay codecommit-merge-pull-request-validation-and-not-found-errors#task#6 (GetRepository) and check the same member.
3. Replay codecommit-repository-lifecycle-and-metadata#task#8 (UpdateRepositoryEncryptionKey) and check $.kmsKeyId form and $.originalKmsKeyId.
4. Replay codecommit-repository-lifecycle-and-metadata#task#10 (BatchGetRepositories for two repositories) and check per-entry kmsKeyId plus entry order against the requested repositoryNames.
5. Replay codecommit-repository-triggers-and-tagging#task#5 (ListTagsForResource) and check for the tagrisId member.
