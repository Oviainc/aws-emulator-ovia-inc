Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.comment-lifecycle-post-get-update-delete codecommit.delete-comment-content-twice-rejected codecommit.put-comment-reaction-invalid-value-rejected
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|PostCommentForComparedCommit|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codecommit PostCommentForComparedCommit: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostCommentForComparedCommit

## Description
`PostCommentForComparedCommit` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/post_comment_for_compared_commit.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `PostCommentForComparedCommit` call, so each one unblocks only when `PostCommentForComparedCommit` works: `codecommit.comment-lifecycle-post-get-update-delete`, `codecommit.delete-comment-content-twice-rejected`, `codecommit.put-comment-reaction-invalid-value-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/post_comment_for_compared_commit.rs`. Replaying the corpus, the emulator refused the `PostCommentForComparedCommit` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PostCommentForComparedCommit operation: operation PostCommentForComparedCommit is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codecommit emulator
Call PostCommentForComparedCommit and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
