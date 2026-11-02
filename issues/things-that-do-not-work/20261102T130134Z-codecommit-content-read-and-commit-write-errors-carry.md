Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-gpn-traj-empty-repository-content-reads
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-gpn-traj-empty-repository-content-reads codecommit-gpn-traj-seeding-main-on-an-empty-repository
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|GetDifferences|4xx->4xx|$.message
Issue-Key: codecommit|GetCommit|4xx->4xx|$.message
Issue-Key: codecommit|PutFile|4xx->4xx|$.__type
Issue-Key: codecommit|CreateCommit|4xx->4xx|$.message
Issue-Related: 20261029T222310Z-codecommit-git-object-and-commit-specifier-not-found.md
Issue-Commit: 48155f868f031d161db2772517918769f1926511

# CodeCommit content-read and commit-write errors carry internal prose and the wrong exception type (4 causes)

> _Note: this issue was identified against the code base at commit `48155f868f031d161db2772517918769f1926511`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDifferences

## Description
FOUR net-new causes sharing one fault: on the content-read and commit-write validation paths the emulator returns the right HTTP status but substitutes its own internal sentence (in two cases the Smithy shape's documentation string, in two cases lowercase developer prose) for AWS's error message, and in one case it also picks a sibling exception code, classifying a malformed value as a missing one. A fixer must close ALL FOUR causes before submitting.

=== Cause 1 of 4: GetDifferences CommitDoesNotExistException message loses AWS's second sentence and the specifier wording ===
AWS: Smithy GetDifferences models CommitDoesNotExistException (errorKind client, httpError 400 [defaulted]) with a single `message` member; the shape's doc is only 'The specified commit does not exist or no commit was specified, and the specified repository has no default branch.' Recorded behavior supplies the wire message: for an unresolvable commit specifier (here the afterCommitSpecifier of an empty repository) AWS answers 400 with 'AWS CodeCommit could not find the commit specifier <commitSpecifier>. Either the specifier does not exist, or the commit has not yet been pushed to the remote repository.' Everything except <commitSpecifier> is FIXED text; the one variable part is the specifier copied verbatim from the request (beforeCommitSpecifier/afterCommitSpecifier).
Emulator: 400 with 'Could not find commit <commitSpecifier>' — a different sentence that drops the word 'specifier' and the whole second sentence.
Why: the dropped sentence is the actionable half for this error class — it is what tells a client that the ref may merely be unpushed rather than nonexistent, which is exactly the empty-repository case this step exercises. A client matching AWS's wording (or an operator reading it) gets neither.
Fix: the GetDifferences handler's commit-specifier resolution in the codecommit service (services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/get_differences.rs and the shared commit-specifier resolver it calls, which must emit the specifier wording rather than the raw-commit wording).
Note: open issue 20261029T222310Z-codecommit-git-object-and-commit-specifier-not-found.md reports this same sentence for GetMergeOptions, but it does not carry this GetDifferences step's replay key, so this cause is unexercised by its replay pointer.
Traces: codecommit-gpn-traj-empty-repository-content-reads#task#5 (representative).

=== Cause 2 of 4: GetCommit InvalidCommitIdException message is the Smithy doc string and names no value ===
AWS: Smithy GetCommit models InvalidCommitIdException (client, httpError 400 [defaulted]); its doc string is 'The specified commit ID is not valid.' and the input member `commitId` targets ObjectId. Recorded behavior: when commitId is not a git object id (here the branch name 'main' passed where a commit id is required) AWS answers 400 InvalidCommitIdException with 'The specified commit ID is not a valid Git object ID: <commitId>' — fixed prefix 'The specified commit ID is not a valid Git object ID: ' plus the rejected id taken verbatim from the request.
Emulator: 400 with 'The specified commit ID is not valid.' — the shape's documentation sentence reused as the wire message, naming nothing.
Why: AWS's message tells the client WHY the value was rejected (it is not a Git object ID — i.e. pass a commit id, not a ref) and WHICH value was rejected; the emulator's variant conveys neither, so a caller that passed a branch name cannot tell a malformed id from an unknown one.
Fix: the GetCommit handler's commitId validation in the codecommit service (src/ops/get_commit.rs) — build the message from the rejected id instead of returning the shape doc.
Note: the same doc-string-as-message pattern is already filed for a different shape in 20261029T222310Z-codecommit-pullrequestdoesnotexistexception-carries-the-smithy-doc-string-instead.md; that issue does not cover InvalidCommitIdException or this step.
Traces: codecommit-gpn-traj-empty-repository-content-reads#task#6.

=== Cause 3 of 4: PutFile classifies a malformed parentCommitId as a missing one (wrong exception code AND message) ===
AWS: Smithy PutFile models InvalidParentCommitIdException and ParentCommitDoesNotExistException as two DISTINCT errors (both client, httpError 400 [defaulted]). InvalidParentCommitIdException is documented 'The parent commit ID is not valid. The commit ID cannot be empty, and must match the head commit ID for the branch of the repository where you want to add or update a file.' — i.e. a FORMAT/consistency rejection; ParentCommitDoesNotExistException is documented 'The parent commit ID is not valid because it does not exist. The specified parent commit ID does not exist in the specified branch of the repository.' — an EXISTENCE rejection. Recorded behavior: a parentCommitId that is not a git object id at all (here the literal 'HEAD', which is not a 40-hex commit id) is rejected on FORM, before any lookup: 400 with $.__type 'InvalidParentCommitIdException' and message 'The parent commit ID is not valid. The commit ID is not a valid length for commit IDs in Git.' — entirely FIXED text; AWS deliberately echoes neither the id nor the repository name.
Emulator: 400 with $.__type 'ParentCommitDoesNotExistException' and message 'the parent commit id <parentCommitId> does not exist in the repository <repositoryName>' — the wrong error code, plus lowercase internal prose that leaks two request values AWS keeps out of the message.
Why: the error code is the part clients dispatch on. A write path that treats a parent-commit existence/outdated error as 'refetch the branch head and retry' will retry forever against the emulator on input that AWS rejects as permanently malformed, while a client that catches InvalidParentCommitIdException to report a bad argument never sees it. Ordering matters too: format validation must precede the existence lookup, otherwise a syntactically invalid id can only ever be reported as missing.
Fix: the PutFile input validation in the codecommit service (src/ops/put_file.rs) — validate parentCommitId's git-object-id form first and raise InvalidParentCommitIdException with AWS's fixed sentence, reserving ParentCommitDoesNotExistException for a well-formed id that is absent.
Traces: codecommit-gpn-traj-seeding-main-on-an-empty-repository#task#5.

=== Cause 4 of 4: CreateCommit FileEntryRequiredException message is lowercase internal prose ===
AWS: Smithy CreateCommit models FileEntryRequiredException (client, httpError 400 [defaulted]), documented 'The commit cannot be created because no files have been specified as added, updated, or changed (PutFile or DeleteFile) for the commit.' Recorded behavior: a CreateCommit that names no putFiles/deleteFiles/setFileModes entries answers 400 with 'A commit cannot be created because there are no changed files included in the commit. A commit requires at least one change.' — entirely FIXED text, no request values interpolated.
Emulator: the correct status and error code, but message 'the commit cannot be created because no file entries were specified' — lowercase, different sentence, and it drops the second sentence stating the rule ('A commit requires at least one change').
Why: this is a pure wording divergence on a value-free message, so any client or test asserting AWS's sentence for the empty-commit case fails, and the rule the second sentence states is not conveyed.
Fix: the CreateCommit handler's empty-change validation in the codecommit service (src/ops/create_commit.rs) — emit AWS's fixed sentence verbatim.
Traces: codecommit-gpn-traj-seeding-main-on-an-empty-repository#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 403 steps, 39 mismatched). Four net-new clusters, none carrying an owning issue or a stub path:

Cause 1 — service=codecommit|GetDifferences|status=400->400|keys=$.message, fixture codecommit-gpn-traj-empty-repository-content-reads step task#5, verdict mismatch. Status 400 expected and observed; $.message: value_mismatch (expected 'AWS CodeCommit could not find the commit specifier main. Either the specifier does not exist, or the commit has not yet been pushed to the remote repository.', got 'Could not find commit main'). Fixed text = everything but the specifier; 'main' is the request's commit specifier filling the template.

Cause 2 — service=codecommit|GetCommit|status=400->400|keys=$.message, fixture codecommit-gpn-traj-empty-repository-content-reads step task#6. $.message: value_mismatch (expected 'The specified commit ID is not a valid Git object ID: main', got 'The specified commit ID is not valid.'). Fixed text = 'The specified commit ID is not a valid Git object ID: '; 'main' is the rejected request value. The actual string is verbatim the InvalidCommitIdException doc in the Smithy model.

Cause 3 — service=codecommit|PutFile|status=400->400|keys=$.__type,$.message, fixture codecommit-gpn-traj-seeding-main-on-an-empty-repository step task#5. $.__type: value_mismatch (expected 'InvalidParentCommitIdException', got 'ParentCommitDoesNotExistException'); $.message: value_mismatch (expected 'The parent commit ID is not valid. The commit ID is not a valid length for commit IDs in Git.', got 'the parent commit id HEAD does not exist in the repository sim2real-probe-gpn-sd-repo-c3990700'). The expected message has NO variable parts; the actual one interpolates the request's parentCommitId ('HEAD') and repositoryName, which are per-run values AWS does not put in this message.

Cause 4 — service=codecommit|CreateCommit|status=400->400|keys=$.message, fixture codecommit-gpn-traj-seeding-main-on-an-empty-repository step task#6. $.message: value_mismatch (expected 'A commit cannot be created because there are no changed files included in the commit. A commit requires at least one change.', got 'the commit cannot be created because no file entries were specified'). Expected message is entirely fixed text; error code and status already match.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it.
2. Replay fixture codecommit-gpn-traj-empty-repository-content-reads step task#5 (GetDifferences against a repository with no commits, afterCommitSpecifier naming an unresolvable ref) and compare $.message against AWS's two-sentence 'could not find the commit specifier <specifier>' text.
3. Replay the same fixture's step task#6 (GetCommit with commitId set to a ref name rather than a git object id) and compare $.message against 'The specified commit ID is not a valid Git object ID: <commitId>'.
4. Replay fixture codecommit-gpn-traj-seeding-main-on-an-empty-repository step task#5 (PutFile with parentCommitId='HEAD') and compare BOTH $.__type (must be InvalidParentCommitIdException, not ParentCommitDoesNotExistException) and $.message (AWS's fixed 'The parent commit ID is not valid. The commit ID is not a valid length for commit IDs in Git.').
5. Replay the same fixture's step task#6 (CreateCommit with no putFiles/deleteFiles/setFileModes) and compare $.message against AWS's fixed two-sentence FileEntryRequiredException text.
6. Re-replay all four steps after the fix; all four must pass.
