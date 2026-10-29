Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-branch-and-commit-workflow
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-branch-and-commit-workflow codecommit-branch-merge-strategies codecommit-file-folder-blob-content-operations codecommit-commit-update-diff-and-file-delete codecommit-get-blob-differences-error-paths codecommit-get-blob-differences-errors codecommit-multi-file-directory-differences codecommit-repo-new-file-diff-against-empty
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|GetCommit|2xx->2xx|$.commit.additionalData
Issue-Key: codecommit|GetCommit|2xx->2xx|$.commit.author.email
Issue-Key: codecommit|GetCommit|2xx->2xx|$.commit.author.name
Issue-Key: codecommit|GetCommit|2xx->2xx|$.commit.committer.email
Issue-Key: codecommit|GetCommit|2xx->2xx|$.commit.committer.name
Issue-Key: codecommit|ListFileCommitHistory|2xx->2xx|$.revisionDag[*]
Issue-Key: codecommit|GetFolder|2xx->2xx|$.folderPath
Issue-Key: codecommit|GetBlobDifferences|2xx->2xx|$.hunks[*]
Issue-Commit: 170d008231b721c2b3436293a90aa925526f50a2

# CodeCommit commit, folder and diff responses omit modeled members or normalize them wrongly (4 causes)

> _Note: this issue was identified against the code base at commit `170d008231b721c2b3436293a90aa925526f50a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCommit

## Description
FOUR net-new causes: successful (HTTP 200) responses on the commit/folder/diff read paths are structurally wrong — modeled members are dropped or normalized into a different value than AWS returns. A fixer must close ALL FOUR before submitting.

=== Cause 1 of 4: GetCommit drops commit.author/committer name and email, and commit.additionalData ===
AWS: Smithy Commit has author and committer of type UserInfo {name, email, date} plus additionalData. Recorded behavior: every member is serialized — name and email carry the committing identity, date is the git timestamp with GMT offset, and additionalData is present as an EMPTY STRING when the commit carries no extra data.
Emulator: author and committer carry only `date`; name, email and additionalData are absent.
Why: a client rendering commit history or attributing a change reads author.name/author.email and gets null for every commit, and it cannot distinguish 'no additional data' from 'member not implemented'.
Fix: the Commit projection used by services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/get_commit.rs — populate UserInfo.name/email from the commit's stored identity and always emit additionalData.
Traces: codecommit-branch-and-commit-workflow#task#8 (also codecommit-branch-merge-strategies#task#12 and #task#17).

=== Cause 2 of 4: ListFileCommitHistory embeds the same truncated Commit ===
AWS: each revisionDag entry's `commit` is a full Commit, so author/committer carry name and email and additionalData is present.
Emulator: the embedded commit again has author/committer with `date` only and no additionalData — the same projection defect on a second operation, so both call sites must be fixed (or both routed through one corrected projection).
Why: file history is exactly where authorship is displayed; it is null for every revision.
Fix: the shared Commit projection plus src/ops/list_file_commit_history.rs.
Traces: codecommit-file-folder-blob-content-operations#task#9.

=== Cause 3 of 4: GetFolder echoes the repository root as '/' instead of the empty string ===
AWS: GetFolderOutput.folderPath is @required and echoes the requested folderPath. Recorded behavior: for the repository root the value is the EMPTY STRING ''.
Emulator: '/'.
Why: clients concatenate folderPath with child names to build paths; a leading '/' yields '//<name>' and paths that do not match the ones GetFolder's own `files` entries report.
Fix: src/ops/get_folder.rs — echo the caller's normalized path, with the root represented as ''.
Traces: codecommit-commit-update-diff-and-file-delete#task#8.

=== Cause 4 of 4: GetBlobDifferences hunk beforeStartLine is 0 for a pure-addition hunk ===
AWS: GetBlobDifferencesOutput.hunks is @required (DiffHunkList) with per-hunk beforeStartLine/beforeLineCount/afterStartLine/afterLineCount and a changes list. Recorded behavior: for a hunk that only ADDS lines (beforeLineCount 0), beforeStartLine is 1 — the 1-based position in the BEFORE blob at which the insertion happens.
Emulator: beforeStartLine 0; every other field of the hunk matches.
Why: a client applying or rendering the hunk computes the insertion point from beforeStartLine; 0 is not a valid 1-based line number and shifts every reconstructed diff by one.
Fix: the hunk-building code behind src/ops/get_blob_differences.rs — emit 1-based start lines for empty before-ranges.
Traces: codecommit-get-blob-differences-error-paths#task#5 (also codecommit-get-blob-differences-errors#task#6, codecommit-multi-file-directory-differences#task#6, codecommit-repo-new-file-diff-against-empty#task#4).

## Evidence
Cause 1 — status 200 as expected; expected $.commit.author.name / $.commit.author.email / $.commit.committer.name / $.commit.committer.email present and $.commit.additionalData '' ; actual all five missing. 3 steps clustered. (The recorded identity string and the commit date are environment values, not requirements; the members' presence is.)
Cause 2 — status 200 as expected; expected $.revisionDag[0].commit to include author.name/email, committer.name/email and additionalData; actual only author.date and committer.date.
Cause 3 — status 200 as expected; expected $.folderPath '' for the repository root, actual '/'.
Cause 4 — status 200 as expected; expected $.hunks[0].beforeStartLine 1 with beforeLineCount 0, actual beforeStartLine 0. 4 steps clustered.

## Steps to Reproduce
1. Replay codecommit-branch-and-commit-workflow#task#8 (GetCommit) and check commit.author/committer name+email and commit.additionalData.
2. Replay codecommit-file-folder-blob-content-operations#task#9 (ListFileCommitHistory) and check the embedded commit's identity members.
3. Replay codecommit-commit-update-diff-and-file-delete#task#8 (GetFolder on the repository root) and check $.folderPath.
4. Replay codecommit-get-blob-differences-error-paths#task#5 (GetBlobDifferences for a newly added file) and check $.hunks[0].beforeStartLine.
