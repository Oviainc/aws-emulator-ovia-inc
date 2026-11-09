Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit__codepipeline__glue__iam__lambda.refarch-glue-fresh-repository-has-no-branches-or-default
Issue-Key: codecommit__codepipeline__glue__iam__lambda.refarch-glue-fresh-repository-has-no-branches-or-default|run#13|AssertionError|assert (code, message) == ('CommitDoesNotExistException', expected_message), (
Issue-Commit: d6116e0d07f52bb90b2d3a4f4f7e7d1efbb0ae5a

# REOPENS: CodeCommit GetFolder's CommitDoesNotExistException uses the shared commit-SPECIFIER wording instead of AWS's 'Could not find commit <specifier>'

> _Note: this issue was identified against the code base at commit `d6116e0d07f52bb90b2d3a4f4f7e7d1efbb0ae5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFolder

## Description
REOPENS 20261023T185908Z-codecommit-8-net-new-behavioral-root-causes-the.md: its fix left the call refused. That resolved report's Cause 8 claimed to fix exactly this check and exactly this assertion (GetFolder on an unseeded repository must answer CommitDoesNotExistException with 'Could not find commit HEAD'). The fix did resolve the absent commitSpecifier to HEAD — the message no longer interpolates an empty string — but it routed the refusal through the crate's SHARED commit-specifier message helper, so the call is still refused with a message that is not AWS's for this operation. This is one cause, not a new defect.

Cause 1 of 1: GetFolder words CommitDoesNotExistException with the shared specifier template rather than its own text.
AWS: Smithy com.amazonaws.codecommit#GetFolder takes repositoryName (@required, RepositoryName, len 1..100, pattern ^[\w\.-]+$), folderPath (@required, Path) and an optional commitSpecifier (CommitName), and models CommitDoesNotExistException (errorKind client, httpError 400 [defaulted]) with a single `message` member; the shape's documentation is only 'The specified commit does not exist or no commit was specified, and the specified repository has no default branch.', so it does not state the wire text. The commitSpecifier member documents the default AWS applies: 'If no specifier is provided, the folder content is returned as it exists in the HEAD commit.' Recorded behavior supplies the wire message: GetFolder(folderPath='/') against a repository that has no commits yet is refused 400 CommitDoesNotExistException with 'Could not find commit HEAD'. The template's FIXED text is 'Could not find commit ' and its one variable part is the commit specifier the request resolved to — the request's commitSpecifier when it names one, and the literal HEAD when it names none. No repository name, folder path or commit id appears in the message.
Emulator: the same status (400) and the same error code, but the message 'AWS CodeCommit could not find the commit specifier HEAD. Either the specifier does not exist, or the commit has not yet been pushed to the remote repository.' — a different, two-sentence template. That is the wording AWS uses for the comparison/merge operations (GetDifferences, GetMergeOptions), and the handler obtains it from the crate-shared helper, so GetFolder inherits a sibling operation's text.
Why: a client reading a freshly created repository hits this refusal on its very first content read, and that is the one it has to recognize to tell 'the repository is empty' from 'the folder is missing'. A caller or test matching AWS's sentence for GetFolder never matches, and the emulator's variant asserts something AWS does not say here — that the commit may merely be unpushed — which is false for a repository that has never had a commit. It also means the two AWS templates for this one error code are not distinguishable in the emulator: every CodeCommit operation that resolves a specifier now answers with the comparison wording.
Fix: the CommitDoesNotExistException construction on the GetFolder handler's commit-resolution path, and the shared message helper it calls — GetFolder must word this refusal itself (fixed prefix plus the resolved specifier) instead of reusing the specifier-wording template the comparison/merge operations need. Keep the HEAD defaulting that the resolved report already landed; only the sentence is wrong. Related open report 20261102T130134Z-codecommit-content-read-and-commit-write-errors-carry.md Cause 1 requires the two-sentence specifier wording for GetDifferences, so the two templates must coexist rather than one replacing the other; that report does not carry this check's replay key and does not cover GetFolder.
Traces: codecommit__codepipeline__glue__iam__lambda.refarch-glue-fresh-repository-has-no-branches-or-default.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (534/793 passed). Failing check codecommit__codepipeline__glue__iam__lambda.refarch-glue-fresh-repository-has-no-branches-or-default [mismatch], unmarked by the report (not ALREADY FILED, not OWNED).

Assertion that fails, verbatim from the report:
"GetFolder on the root of an unseeded repository raised 'CommitDoesNotExistException': 'AWS CodeCommit could not find the commit specifier HEAD. Either the specifier does not exist, or the commit has not yet been pushed to the remote repository.', expected CommitDoesNotExistException: 'Could not find commit HEAD' — a FolderDoesNotExistException or a RepositoryDoesNotExistException would both point the operator at the wrong missing thing"

Status (400) and error code (CommitDoesNotExistException) already match; $.message is the only divergence. In the expected message 'Could not find commit ' is fixed text and 'HEAD' is the resolved commit specifier (the request named none), not a per-run value — the generated repository name in the same check IS per-run and correctly appears in neither message.

Source of the divergence in the checkout: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/get_folder.rs line 220 builds the error with crate::state::commit_does_not_exist_message(&specifier), and services/service-com-amazonaws-codecommit-codecommit-20150413/src/state.rs lines 803-808 define that helper as the two-sentence 'AWS CodeCommit could not find the commit specifier {specifier}. Either the specifier does not exist, or the commit has not yet been pushed to the remote repository.' template. The HEAD default itself is correct (state.rs line 788, DEFAULT_COMMIT_SPECIFIER = "HEAD", applied at get_folder.rs lines 214-217).

## Steps to Reproduce
1. Boot the emulator from this checkout and point botocore at it; cc = client('codecommit').
2. cc.create_repository(repositoryName=R) and write nothing to it (no PutFile, no CreateCommit, no git push), so the repository has no commits and no default branch.
3. cc.get_folder(repositoryName=R, folderPath='/') with NO commitSpecifier.
4. Expect HTTP 400, Error.Code 'CommitDoesNotExistException', Error.Message exactly 'Could not find commit HEAD'. Observe the code and status correct but the message 'AWS CodeCommit could not find the commit specifier HEAD. Either the specifier does not exist, or the commit has not yet been pushed to the remote repository.'
5. Repeat with an explicit unresolvable specifier (cc.get_folder(repositoryName=R, folderPath='/', commitSpecifier='main')) to confirm the template's one variable slot: the message must be 'Could not find commit main'.
6. Confirm the comparison wording is NOT broken by the fix: GetDifferences/GetMergeOptions with an unresolvable specifier must keep the two-sentence 'AWS CodeCommit could not find the commit specifier <specifier>. ...' text.
7. Re-replay codecommit__codepipeline__glue__iam__lambda.refarch-glue-fresh-repository-has-no-branches-or-default; it must pass.
