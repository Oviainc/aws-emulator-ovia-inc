Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.delete-file-on-tag-named-branch-blocked codecommit.git-push-non-fast-forward-refused-by-the-server codecommit.git-push-over-https-is-readable-through-the-api codecommit.executable-and-symlink-file-modes-change-the-tree-id codecommit.stale-parent-commit-id-rejected codecommit__codepipeline__glue__iam__lambda.refarch-glue-branch-via-put-file-and-create-commit-agree codecommit.blob-id-is-content-addressed-git-sha1 codecommit.branch-delete-then-get-not-found codecommit.delete-default-branch-rejected codecommit.create-commit-with-no-effective-change-rejected codecommit__codepipeline__glue__iam__lambda.refarch-glue-fresh-repository-has-no-branches-or-default
Issue-Key: codecommit.delete-file-on-tag-named-branch-blocked|_run#24|RuntimeError|raise RuntimeError(
Issue-Key: codecommit.git-push-non-fast-forward-refused-by-the-server|_retry_git#27|Timeout|time.sleep(interval_s)
Issue-Key: codecommit.git-push-over-https-is-readable-through-the-api|_retry_git#28|Timeout|time.sleep(interval_s)
Issue-Key: codecommit.executable-and-symlink-file-modes-change-the-tree-id|run#29|KeyError|assert root["files"] == [], (
Issue-Key: codecommit|PutFile|ParentCommitIdOutdatedException
Issue-Key: codecommit__codepipeline__glue__iam__lambda.refarch-glue-branch-via-put-file-and-create-commit-agree|run#19|AssertionError|assert message == expected_message, (
Issue-Key: codecommit.blob-id-is-content-addressed-git-sha1|run#16|AssertionError|assert message == expected_message, (
Issue-Key: codecommit.branch-delete-then-get-not-found|run#15|AssertionError|assert message == expected_message, (
Issue-Key: codecommit.delete-default-branch-rejected|run#12|AssertionError|assert message == "", f"expected an empty message, got {message!r}"
Issue-Key: codecommit.create-commit-with-no-effective-change-rejected|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codecommit__codepipeline__glue__iam__lambda.refarch-glue-fresh-repository-has-no-branches-or-default|run#13|AssertionError|assert (code, message) == ('CommitDoesNotExistException', expected_message), (
Issue-Commit: 144a2fce0f9e3116760062dd946ebab581523079

# CodeCommit: 8 net-new behavioral root causes — the announced git clone URL is unusable, GetFolder omits its empty lists, and six error-message templates diverge

> _Note: this issue was identified against the code base at commit `144a2fce0f9e3116760062dd946ebab581523079`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFile

## Description
Eight net-new root causes in the codecommit crate, each grounded in a failing behavioral check that the replay report left unmarked (not ALREADY FILED, not OWNED). Related open or resolved issues are named per cause. A fixer must close all eight before submitting.

Cause 1 of 8: the git clone URL the service announces cannot be used by a standard git client — the transport now answers, but with a TLS certificate no client trusts, so every clone/push fails.
AWS: recorded behavior only. GetRepository.repositoryMetadata.cloneUrlHttp is an address the service answers git smart-HTTP on over TLS with a publicly trusted certificate: with IAM service-specific CodeCommit credentials, `git clone <cloneUrlHttp>` succeeds, `git push origin <tag>` creates a ref the control plane then sees, and a push that is not a fast-forward is refused inside receive-pack. CodeCommit has no API operation that creates a tag, so the tag-vs-branch and server-side push-refusal behaviors are reachable only over that transport.
Emulator: `git push` against the announced URL fails before any git protocol exchange: rc=128, stderr "fatal: unable to access 'https://127.0.0.1:34793/v1/repos/<repo>/': server verification failed: certificate signer not trusted. (CAfile: /etc/ssl/certs/ca-certificates.crt CRLfile: none)". The host/port vary per run and are not the requirement; the requirement is that the announced cloneUrlHttp be reachable by an unmodified git client. The two other git checks exhaust their clone/push poll and the harness reports 'Check timed out after 60.0s'. Hypothesis: those polls retry the same TLS rejection until their budget is gone.
Why: cloning cloneUrlHttp is the documented way to use a CodeCommit repository. A client that reads the metadata and clones it cannot reach the repository at all, and no server-side push refusal (non-fast-forward, branch-protection deny) or tag-created-by-git behavior is observable.
Note: the RESOLVED issue '20261022T171422Z-codecommit-serves-no-git-smart-http-transport-and.md' reported that nothing answered at cloneUrlHttp; something answers now, so that fix is partially landed and this is the residual defect. Open issue '20261023T072048Z-codecommit-10-behavioral-root-causes-three-operations-answer.md' Cause 5 describes the same family and owns four other check ids; none of the three ids below is on its replay list.
Fix: the git smart-HTTP transport's TLS termination for the codecommit crate, together with the cloneUrlHttp/cloneUrlSsh construction in its repository-metadata serializer — the announced URL must be an address and scheme the emulator serves with a certificate the test environment's client trust store accepts (or a scheme that needs none).
Traces: codecommit.delete-file-on-tag-named-branch-blocked (hard rc=128 TLS refusal, the grounding evidence), codecommit.git-push-non-fast-forward-refused-by-the-server, codecommit.git-push-over-https-is-readable-through-the-api.

Cause 2 of 8: GetFolder omits its empty collection members instead of serializing them as empty lists.
AWS: Smithy (com.amazonaws.codecommit#CodeCommit_20150413) GetFolderOutput has commitId (@required), folderPath (@required), treeId, and the four list members subFolders (FolderList), files (FileList), symbolicLinks (SymbolicLinkList) and subModules (SubModuleList). Recorded behavior: a folder that holds no files of a given kind still answers with the member present as an empty array — the check reads root["files"] and compares it to [] after asserting the subtree it did write.
Emulator: the member is absent from the JSON body, so the SDK's parsed response has no 'files' key at all and the client raises KeyError: 'files'.
Why: every client written against AWS indexes these members unconditionally (resp['files'], resp['subFolders']) because AWS always sends them; an emulator that omits them turns a normal empty-folder read into a crash in the caller rather than an empty iteration.
Fix: the output projection/serializer for services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/get_folder.rs — emit subFolders/files/symbolicLinks/subModules as empty arrays rather than skipping empty collections.
Traces: codecommit.executable-and-symlink-file-modes-change-the-tree-id.

Cause 3 of 8: ParentCommitIdOutdatedException carries emulator prose instead of AWS's fixed text.
AWS: Smithy models ParentCommitIdOutdatedException on PutFile, CreateCommit and DeleteFile as a client error with httpError 400. Recorded behavior: the message is the FIXED string 'The parent commit ID is not valid because it is not the current tip of the branch. You must provide the commit ID for the tip of the branch. The most common reason for this error is that your local repository is not synchronized to the latest changes in the AWS CodeCommit repository. Either synchronize your local repo by using the git pull command, or use GetBranch to retrieve the latest commit ID for the branch, and then try again.' — it names neither the stale id nor the branch, i.e. it is a constant for this class.
Emulator: 'the parent commit id <staleCommitId> is not the tip of the branch <branchName>' — a different, request-interpolated template. The HTTP status (400) and the error code are already correct; the assertion that fails is the one immediately after the code assertion.
Why: this is the optimistic-concurrency refusal every write path hits under contention. Clients surface or match the documented text (it is the one that tells the operator to re-read the tip); a different template breaks message-matching retry logic and loses the remediation instruction.
Fix: the ParentCommitIdOutdatedException message construction in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/put_file.rs and the shared parent-commit/tip validator used by create_commit.rs and delete_file.rs.
Traces: codecommit.stale-parent-commit-id-rejected.

Cause 4 of 8: ParentCommitIdRequiredException carries emulator prose instead of AWS's fixed text.
AWS: Smithy models ParentCommitIdRequiredException on PutFile and CreateCommit (client error, httpError 400) — it is raised when a write targets a branch that already has a tip and the request omits parentCommitId. Recorded behavior: the message is the FIXED string 'A parent commit ID is required. Either use GetBranch to retrieve the latest commit ID for the branch, or use a Git command such as git pull or git log, and then try again.'
Emulator: 'a parent commit id is required to write to the branch <branchName>' — request-interpolated, and it drops the remediation sentence. Status 400 and the error code already match.
Why: the same message-matching and remediation loss as Cause 3, on the first write a seeded branch receives.
Fix: the ParentCommitIdRequiredException message construction in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/put_file.rs and src/ops/create_commit.rs (shared parent-commit validator).
Traces: codecommit__codepipeline__glue__iam__lambda.refarch-glue-branch-via-put-file-and-create-commit-agree.

Cause 5 of 8: BlobIdDoesNotExistException does not use AWS's request-derived message template.
AWS: Smithy GetBlob takes repositoryName (@required) and blobId (@required, ObjectId) and models BlobIdDoesNotExistException as a client error with httpError 400. Recorded behavior: the message is built from the request as 'AWS CodeCommit could not find the blob <blobId> in the repository.' — it names the blob id and does NOT name the repository.
Emulator: 'the blob ID <blobId> does not exist in the repository <repositoryName>' — a different template that also echoes the repository name. Status 400 and the error code already match; the blobId content-addressing assertions in the same check now pass, so this message is the only remaining divergence.
Why: clients that parse or match the documented text when reconciling a mirrored object store get an unrecognized error, and the extra repository echo is not an AWS message shape.
Fix: the BlobIdDoesNotExistException message construction in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/get_blob.rs.
Traces: codecommit.blob-id-is-content-addressed-git-sha1.

Cause 6 of 8: GetBranch's BranchDoesNotExistException carries a message where AWS sends none.
AWS: Smithy GetBranch models BranchDoesNotExistException as a client error with httpError 400. Recorded behavior only: on this operation AWS's error body carries no message text — the SDK-parsed Error.Message is the empty string. Two independently authored checks assert the same empty message, on a branch deleted by DeleteBranch and on a branch deleted after UpdateDefaultBranch repointed the default.
Emulator: Error.Message is 'The specified branch does not exist'. The status (400) and the code already match.
Why: the presence or absence of Message is part of the response shape, not formatting latitude. A differential client or recorded-fixture comparison diverges on every missing-branch read, and code that branches on a non-empty message takes a path AWS never produces.
Fix: the BranchDoesNotExistException construction in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/get_branch.rs — this operation's not-found error must serialize with no message text.
Traces: codecommit.branch-delete-then-get-not-found, codecommit.delete-default-branch-rejected.

Cause 7 of 8: the no-effective-change rejection on CreateCommit carries emulator prose instead of AWS's fixed text.
AWS: Smithy CreateCommit models NoChangeException (and PutFile models SameFileContentException), both client errors with httpError 400. Recorded behavior: a CreateCommit whose putFiles reproduce the parent commit's tree exactly is refused with the FIXED message 'Cannot create the commit. At least one change is required to create a commit, but the files in the commit are exactly the same as the files in the parent commit. If you want to create an empty commit, use a Git client.'
Emulator: 'the commit cannot be created because it makes no change to the repository'. Status 400 and the error code already match, and the treeId assertions earlier in the same check now pass, so this message is the residual divergence.
Why: AWS's text states the rule and names the only workaround (use a git client for an empty commit); clients that surface it to a user, or match it, get neither.
Note: this check id is on the replay list of open issue '20261023T072048Z-codecommit-10-behavioral-root-causes-three-operations-answer.md' Cause 6 (object ids not content-addressed), whose replay key is this check's earlier tree-id assertion. That assertion now passes and the check fails later, on the message, so this is a distinct residual cause of the same check.
Fix: the NoChangeException message construction in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_commit.rs (and the SameFileContentException literal in put_file.rs, which shares the no-change detection).
Traces: codecommit.create-commit-with-no-effective-change-rejected.

Cause 8 of 8: GetFolder on an unseeded repository builds its CommitDoesNotExistException message from an empty commit specifier instead of AWS's text.
AWS: Smithy GetFolder takes repositoryName (@required), folderPath (@required) and an optional commitSpecifier, and models CommitDoesNotExistException (client, httpError 400). Recorded behavior: GetFolder(folderPath='/') against a repository that has no commits yet is refused with CommitDoesNotExistException and message 'Could not find commit HEAD' — AWS resolves the omitted commitSpecifier to HEAD and names HEAD in the message.
Emulator: the same code, but message 'the commit  does not exist in the repository <repositoryName>' — the default specifier is interpolated as an empty string (note the double space), so the message names no commit at all.
Why: the check's own words: a message naming nothing points the operator at the wrong missing thing. An emulator that interpolates an unresolved default also signals that the HEAD default itself is not being resolved, which is the behavior any client inspecting a fresh repository depends on.
Fix: the commitSpecifier default resolution and the CommitDoesNotExistException message construction in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/get_folder.rs — an absent commitSpecifier must resolve to HEAD and the error must name it.
Traces: codecommit__codepipeline__glue__iam__lambda.refarch-glue-fresh-repository-has-no-branches-or-default.

## Evidence
All quoted strings below are verbatim from the simparity-behavioral-aws-v1 replay report (154 checks, 37 passed). Generated repository names, commit ids, hosts and ports vary per run and are examples; the rule in each cause is the requirement.

Cause 1 — codecommit.delete-file-on-tag-named-branch-blocked [error]: RuntimeError: command ['git', 'push', 'origin', 'main'] failed (rc=128): stderr="fatal: unable to access 'https://127.0.0.1:34793/v1/repos/sim2real-behav-repo-48407887cff0/': server verification failed: certificate signer not trusted. (CAfile: /etc/ssl/certs/ca-certificates.crt CRLfile: none)". codecommit.git-push-non-fast-forward-refused-by-the-server [mismatch] and codecommit.git-push-over-https-is-readable-through-the-api [mismatch]: 'Check timed out after 60.0s' with no git exchange completed.

Cause 2 — codecommit.executable-and-symlink-file-modes-change-the-tree-id [error]: at `assert root["files"] == []` the client raises KeyError: 'files' — the GetFolder response has no 'files' member.

Cause 3 — codecommit.stale-parent-commit-id-rejected [mismatch]: "stale parentCommitId raised message 'the parent commit id 477fc635f8b5c21d7229d3df54da3d8936c61ff1 is not the tip of the branch main', expected 'The parent commit ID is not valid because it is not the current tip of the branch. You must provide the commit ID for the tip of the branch. The most common reason for this error is that your local repository is not synchronized to the latest changes in the AWS CodeCommit repository. Either synchronize your local repo by using the git pull command, or use GetBranch to retrieve the latest commit ID for the branch, and then try again.'"

Cause 4 — codecommit__codepipeline__glue__iam__lambda.refarch-glue-branch-via-put-file-and-create-commit-agree [mismatch]: "the ParentCommitIdRequiredException message should be 'A parent commit ID is required. Either use GetBranch to retrieve the latest commit ID for the branch, or use a Git command such as git pull or git log, and then try again.', got 'a parent commit id is required to write to the branch main'".

Cause 5 — codecommit.blob-id-is-content-addressed-git-sha1 [mismatch]: "expected message 'AWS CodeCommit could not find the blob ad4f7fb7ef7d7576b0f811e7716bc33c25276467 in the repository.', got 'the blob ID ad4f7fb7ef7d7576b0f811e7716bc33c25276467 does not exist in the repository sim2real-behav-repo-f0d3299ff81c'".

Cause 6 — codecommit.branch-delete-then-get-not-found [mismatch]: "expected message '', got 'The specified branch does not exist'" (the preceding `assert code == "BranchDoesNotExistException"` passed). codecommit.delete-default-branch-rejected [mismatch]: "expected an empty message, got 'The specified branch does not exist'" at the positive-control GetBranch on a deleted non-default branch.

Cause 7 — codecommit.create-commit-with-no-effective-change-rejected [mismatch]: "expected message 'Cannot create the commit. At least one change is required to create a commit, but the files in the commit are exactly the same as the files in the parent commit. If you want to create an empty commit, use a Git client.', got 'the commit cannot be created because it makes no change to the repository'".

Cause 8 — codecommit__codepipeline__glue__iam__lambda.refarch-glue-fresh-repository-has-no-branches-or-default [mismatch]: "GetFolder on the root of an unseeded repository raised 'CommitDoesNotExistException': 'the commit  does not exist in the repository sim2real-refarch-glue-repo-b957e7d6aa5f', expected CommitDoesNotExistException: 'Could not find commit HEAD'".

## Steps to Reproduce
Boot the emulator and point botocore at it; cc = client('codecommit').

1. Cause 1: meta = cc.create_repository(repositoryName=R)['repositoryMetadata']; take meta['cloneUrlHttp'] and run `git clone <cloneUrlHttp>` / `git push origin main` with an unmodified git client -> expect the clone and push to complete (and a non-fast-forward push to be refused by receive-pack); observe git exiting 128 with 'server verification failed: certificate signer not trusted', or the clone poll never succeeding.
2. Cause 2: cc.put_file(repositoryName=R, branchName='main', filePath='src/app.sh', fileContent=b'#!/bin/sh\n', fileMode='EXECUTABLE'); cc.get_folder(repositoryName=R, folderPath='/') -> expect the response to contain files=[] (and subFolders/symbolicLinks/subModules present) for a root holding only a subfolder; observe the 'files' key absent.
3. Cause 3: c1 = cc.put_file(... branchName='main', filePath='a.txt', fileContent=b'a')['commitId']; cc.put_file(... parentCommitId=c1, filePath='b.txt', fileContent=b'b'); then cc.put_file(... parentCommitId=c1, filePath='c.txt', fileContent=b'c') -> expect 400 ParentCommitIdOutdatedException with exactly the fixed text quoted in Cause 3; observe 'the parent commit id <c1> is not the tip of the branch main'.
4. Cause 4: with 'main' already carrying a tip, cc.put_file(repositoryName=R, branchName='main', filePath='d.txt', fileContent=b'd') with NO parentCommitId -> expect 400 ParentCommitIdRequiredException with 'A parent commit ID is required. Either use GetBranch to retrieve the latest commit ID for the branch, or use a Git command such as git pull or git log, and then try again.'; observe 'a parent commit id is required to write to the branch main'.
5. Cause 5: cc.get_blob(repositoryName=R, blobId='ad4f7fb7ef7d7576b0f811e7716bc33c25276467') for an id not in the repository -> expect 400 BlobIdDoesNotExistException with 'AWS CodeCommit could not find the blob <blobId> in the repository.'; observe the emulator's template naming the repository.
6. Cause 6: cc.create_branch(repositoryName=R, branchName='side', commitId=C); cc.delete_branch(repositoryName=R, branchName='side'); cc.get_branch(repositoryName=R, branchName='side') -> expect 400 BranchDoesNotExistException whose Error.Message is empty; observe 'The specified branch does not exist'.
7. Cause 7: cc.create_commit(repositoryName=R, branchName='main', parentCommitId=tip, putFiles=[the parent commit's files with identical bytes and paths]) -> expect 400 NoChangeException with the fixed text quoted in Cause 7; observe 'the commit cannot be created because it makes no change to the repository'.
8. Cause 8: cc.create_repository(repositoryName=R8) and write nothing; cc.get_folder(repositoryName=R8, folderPath='/') -> expect 400 CommitDoesNotExistException with 'Could not find commit HEAD'; observe 'the commit  does not exist in the repository R8'.
9. Re-replay every check id in trace_ids.
