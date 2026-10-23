Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.get-repository-not-found codecommit.list-repositories-reflects-created codecommit.repository-create-get-delete codecommit.create-repository-duplicate codecommit.put-file-on-nonexistent-branch-blocked
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit.get-repository-not-found|run#10|AssertionError|assert message == f"{missing_name} for {ctx.account}", message
Issue-Key: codecommit|GetRepository|RepositoryDoesNotExistException
Issue-Key: codecommit.repository-create-get-delete|_assert_not_found#17|AssertionError|assert err.get("Message") == expected_message, f'expected the message {expected_message!r}, got {err.get("Message")!r}'
Issue-Key: codecommit.create-repository-duplicate|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codecommit.put-file-on-nonexistent-branch-blocked|run#10|AssertionError|assert message == "The specified branch does not exist", (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeCommit client-error messages do not follow AWS's request-derived templates (3 causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepository

## Description
Three message-template causes. In all three the HTTP status (400) and the error code are correct; only the message text diverges, and in each case the text AWS produces is a fixed template built from the request (and the caller's account), so the template is the requirement — not the recorded literal.

Cause 1 of 3: RepositoryDoesNotExistException carries the emulator's own prose instead of "<repositoryName> for <accountId>".
AWS: Smithy models RepositoryDoesNotExistException (client, httpError 400) on GetRepository, PutFile, UpdateRepositoryDescription and friends. Recorded behavior: the message is exactly the requested repository name, a space, the word "for", a space, and the 12-digit account id of the caller — e.g. for a request naming 'sim2real-behav-repo-014d3a3c9e44' under account 000000000000 the message is 'sim2real-behav-repo-014d3a3c9e44 for 000000000000'.
Emulator: 'the repository sim2real-behav-repo-014d3a3c9e44 does not exist' — a different template that names no account.
Why: operators and tools that parse or match on this message (log scrapers, and the many AWS samples that echo it) cannot tell which account the lookup was attempted in, and a differential client comparing emulator and AWS output flags every not-found path.
Fix: the message construction for RepositoryDoesNotExistException in the codecommit crate's error/validation layer (services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/get_repository.rs and the shared repository-lookup helper it uses).
Traces: codecommit.get-repository-not-found, codecommit.list-repositories-reflects-created, codecommit.repository-create-get-delete.

Cause 2 of 3: RepositoryNameExistsException message is not "Repository named <name> already exists".
AWS: Smithy models RepositoryNameExistsException (client, 400) on CreateRepository. Recorded behavior: the message is 'Repository named <repositoryName> already exists', built from the request.
Emulator: 'the repository name sim2real-behav-repo-662f5bd3c58e already exists'.
Why: idempotent-create wrappers that distinguish "already exists" by message, and users reading the message, see text no AWS SDK sample or doc shows.
Fix: the duplicate-name rejection in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_repository.rs.
Traces: codecommit.create-repository-duplicate.

Cause 3 of 3: BranchDoesNotExistException message is not the fixed string AWS returns.
AWS: Smithy models BranchDoesNotExistException (client, 400) on PutFile, DeleteFile and CreateCommit. Recorded behavior: the message is the constant 'The specified branch does not exist' (it names neither branch nor repository).
Emulator: 'the branch sim2real-behav-ghost-branch-597b0f3e6639 does not exist in the repository sim2real-behav-repo-0cc0f343e26d'.
Why: the same parse/compare breakage as above on the write path; the emulator is also more informative than AWS, which hides a real-AWS ambiguity (a tag name resolves to this same message) that clients are written against.
Fix: the branch-resolution rejection shared by src/ops/put_file.rs and src/ops/delete_file.rs in the codecommit crate.
Traces: codecommit.put-file-on-nonexistent-branch-blocked.

## Evidence
Cause 1 — expected message 'sim2real-behav-repo-014d3a3c9e44 for 000000000000'; actual 'the repository sim2real-behav-repo-014d3a3c9e44 does not exist' (code RepositoryDoesNotExistException, HTTP 400, both correct). Same assertion failed in codecommit.get-repository-not-found ('the repository sim2real-behav-missing-40896fb02afb does not exist') and codecommit.list-repositories-reflects-created ('the repository sim2real-behav-repo-18b544ac7a63 does not exist').
Cause 2 — expected "Repository named sim2real-behav-repo-662f5bd3c58e already exists"; actual "the repository name sim2real-behav-repo-662f5bd3c58e already exists".
Cause 3 — expected 'The specified branch does not exist'; actual 'the branch sim2real-behav-ghost-branch-597b0f3e6639 does not exist in the repository sim2real-behav-repo-0cc0f343e26d'.
Names and the account id vary per run; the template is the requirement.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cc.get_repository(repositoryName='never-created') -> expect 400 RepositoryDoesNotExistException with message '<name> for <accountId>'; observe 'the repository <name> does not exist'.
3. cc.create_repository(repositoryName=R) twice -> expect 400 RepositoryNameExistsException 'Repository named <R> already exists'; observe 'the repository name <R> already exists'.
4. cc.put_file(repositoryName=R, branchName='ghost-branch', filePath='f.txt', fileContent=b'x') -> expect 400 BranchDoesNotExistException 'The specified branch does not exist'; observe the repository-naming variant.
5. Re-replay the check ids in trace_ids.
