Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.approval-rule-content-required-rejected codecommit.batch-describe-merge-conflicts-agrees-with-the-per-path-reads codecommit.batch-get-commits-limit-exceeded-rejected codecommit.branch-name-is-tag-name-rejected codecommit.client-request-token-required-rejected codecommit.comment-content-required-rejected codecommit.commit-create-advances-branch-head codecommit.commit-create-then-get-chain codecommit.commit-id-required-rejected codecommit.commit-message-unicode-preserved codecommit.commit-timestamp-precision-preserved codecommit.create-commit-with-no-effective-change-rejected codecommit.empty-branch-name-rejected codecommit.get-blob-differences-between-commits codecommit.get-differences-between-commits codecommit.get-differences-reports-exact-changed-paths-and-blob-ids codecommit.get-folder-lists-committed-files codecommit.get-folder-partitions-files-subfolders-and-symlinks codecommit.identical-file-content-dedupes-to-one-blob-id codecommit.pull-request-approval-rule-lifecycle codecommit.pull-request-lifecycle-create-merge-close codecommit.pull-request-reopen-after-close-rejected codecommit.unicode-commit-message-round-trip codecommit.update-title-on-closed-pull-request-rejected
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|CreateCommit|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codecommit CreateCommit: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCommit

## Description
`CreateCommit` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_commit.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

24 recorded unit(s) fail on their `CreateCommit` call, so each one unblocks only when `CreateCommit` works: `codecommit.approval-rule-content-required-rejected`, `codecommit.batch-describe-merge-conflicts-agrees-with-the-per-path-reads`, `codecommit.batch-get-commits-limit-exceeded-rejected`, `codecommit.branch-name-is-tag-name-rejected`, `codecommit.client-request-token-required-rejected`, `codecommit.comment-content-required-rejected`, `codecommit.commit-create-advances-branch-head`, `codecommit.commit-create-then-get-chain`, `codecommit.commit-id-required-rejected`, `codecommit.commit-message-unicode-preserved`, `codecommit.commit-timestamp-precision-preserved`, `codecommit.create-commit-with-no-effective-change-rejected`, `codecommit.empty-branch-name-rejected`, `codecommit.get-blob-differences-between-commits`, `codecommit.get-differences-between-commits`, `codecommit.get-differences-reports-exact-changed-paths-and-blob-ids`, `codecommit.get-folder-lists-committed-files`, `codecommit.get-folder-partitions-files-subfolders-and-symlinks`, `codecommit.identical-file-content-dedupes-to-one-blob-id`, `codecommit.pull-request-approval-rule-lifecycle` (+4 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_commit.rs`. Replaying the corpus, the emulator refused the `CreateCommit` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateCommit operation: operation CreateCommit is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codecommit emulator
Call CreateCommit and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
