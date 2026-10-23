Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.blob-id-is-content-addressed-git-sha1 codecommit.executable-and-symlink-file-modes-change-the-tree-id codecommit.delete-file-restores-prior-tree-id codecommit.concurrent-put-file-same-parent-single-winner codecommit.put-file-concurrent-writes-optimistic-lock codecommit.stale-parent-commit-id-rejected codecommit.supplied-conflict-resolution-produces-the-resolved-blob
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit.blob-id-is-content-addressed-git-sha1|run#8|AssertionError|assert out["blobId"] == expected, (
Issue-Key: codecommit.executable-and-symlink-file-modes-change-the-tree-id|run#10|AssertionError|assert normal["blobId"] == _blob_sha1(script), (
Issue-Key: codecommit.delete-file-restores-prior-tree-id|run#11|AssertionError|assert tree1 == root1, f"PutFile treeId {tree1!r} != computed root tree {root1!r}"
Issue-Key: codecommit.concurrent-put-file-same-parent-single-winner|run#18|AssertionError|assert winner["blobId"] == _blob_sha1(payloads[winner_path]), (
Issue-Key: codecommit.put-file-concurrent-writes-optimistic-lock|run#18|AssertionError|assert winning_blob_id == expected_blob_id, (
Issue-Key: codecommit.stale-parent-commit-id-rejected|run#11|AssertionError|assert put2["blobId"] == _blob_sha1(v2), f"setup PutFile blobId {put2['blobId']!r}"
Issue-Key: codecommit.supplied-conflict-resolution-produces-the-resolved-blob|run#13|AssertionError|assert untouched_blob == _blob_sha1(untouched), (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeCommit blobId/treeId are not git content-addressed SHA-1 object ids

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFile

## Description
Cause 1 of 1: the ObjectIds CodeCommit writes return (PutFile/DeleteFile/CreateCommit blobId and treeId) are not the git object ids of the content they describe.

AWS: CodeCommit is a git service and its ObjectIds are git object ids. Smithy (com-amazonaws-codecommit-codecommit-20150413): PutFile.output has commitId, blobId and treeId all @required targeting ObjectId; DeleteFile.output likewise (commitId/blobId/treeId/filePath required); CreateCommit.output carries commitId/treeId. The model does not state how the value is derived, so: recorded behavior only — these checks compute the id locally and real AWS agrees, i.e. blobId == sha1("blob <byte-length>\0" + file bytes) and treeId == the git tree object id of the resulting root tree (entries sorted, mode 100644 for NORMAL, 100755 for EXECUTABLE, 120000 for SYMLINK). Consequently a file-mode change alone changes the treeId, and removing a file restores the treeId the tree had before it was added.

Emulator: every id is unrelated to the content. Examples from the replay: PutFile for a 15-byte src/app.py reported blobId 'caff0d68447c437efc5271ea519fdeb2341488a6' where the git blob id of those bytes is 'b376c9941fda362c8d2c5c8ddb35db3e0b003402'; PutFile reported treeId '02971328b97590e35ec8b39fb3c1dc9a6cf4c02a' where the computed root tree is 'b8c1809ff9a3e20f6fcd7b1f310aa540a5542db3'; the NORMAL-mode leg of the file-mode check reported 'adadef827328106c0010050a57443dd16746842a' for 38 bytes whose git blob id is '1cd36f50290bf7dca6b052253a8a353501a50071'. The same divergence appears on the winning write of both concurrency checks and on the untouched file read back after a merge-conflict resolution, so it is one id-computation defect and not a per-operation one.

Why: a client that mirrors a CodeCommit repository into git, de-duplicates or verifies content by blob id, or compares a returned treeId against a local `git hash-object` / `git write-tree`, concludes the service stored different bytes than it sent. Tree ids that do not move with a mode change or do not revert on delete also break every "did the tree actually change" check built on them.

Fix: the object-id computation in the codecommit crate's content/tree state shared by services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/put_file.rs, delete_file.rs and create_commit.rs — ids must be derived from the git object encoding of the stored content rather than generated.

Traces: codecommit.blob-id-is-content-addressed-git-sha1, codecommit.executable-and-symlink-file-modes-change-the-tree-id, codecommit.delete-file-restores-prior-tree-id, codecommit.concurrent-put-file-same-parent-single-winner, codecommit.put-file-concurrent-writes-optimistic-lock, codecommit.stale-parent-commit-id-rejected, codecommit.supplied-conflict-resolution-produces-the-resolved-blob.

## Evidence
Expected: blobId == sha1("blob <len>\0"+bytes) and treeId == git root-tree id of the resulting tree.
Actual (AssertionErrors from the replay):
- "PutFile blobId for src/app.py (15 bytes) is 'caff0d68447c437efc5271ea519fdeb2341488a6'; the git blob object id of those bytes is 'b376c9941fda362c8d2c5c8ddb35db3e0b003402'"
- "PutFile treeId '02971328b97590e35ec8b39fb3c1dc9a6cf4c02a' != computed root tree 'b8c1809ff9a3e20f6fcd7b1f310aa540a5542db3'"
- "PutFile at NORMAL reported blobId 'adadef827328106c0010050a57443dd16746842a'; the git blob id of those 38 bytes is '1cd36f50290bf7dca6b052253a8a353501a50071'"
- "winning PutFile blobId '42037784bc12749b6c345ea3c7066319c42521e2' for racer-0.txt" / "winning PutFile blobId 'c0dda7eef5846abe6b5917d8106d17af98ca4b10' != sha1 git object id of its own payload '44e82b58855eca6ac96107fc3fabddc2158ab638'"
- "untouched.txt blobId on main is 'b483f57d9dbc91c743feba9eca98c700f0a96000', the git blob id of those bytes is 'f0f551d7e2fa3f4ad9af434f8ed8293c0161dfa6'"
- "setup PutFile blobId 'bd965fae20eb0dd136c5fb90b7c99ea1863ab46b'" (stale-parent check, same assertion)
The hex values differ per run; the requirement is the derivation, not the literals.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cc = client('codecommit'); cc.create_repository(repositoryName=R).
3. body = b'print("hello")\n'; out = cc.put_file(repositoryName=R, branchName='main', filePath='src/app.py', fileContent=body, commitMessage='seed').
4. Compare out['blobId'] with hashlib.sha1(b'blob %d\0' % len(body) + body).hexdigest() -> expect equality; observe an unrelated id.
5. Compute the git root-tree id for {'src/app.py': (100644, blob)} and compare with out['treeId'] -> expect equality; observe an unrelated id.
6. Re-put the same path with fileMode='EXECUTABLE' and observe whether treeId changes as git requires.
7. Re-replay the check ids in trace_ids.

## Resolution

The derivation named in "Fix" is in place: object ids are computed by
`src/state/git_object.rs`'s object-name function, which hashes the git object encoding
`"<kind> <len>\0" + payload` with **SHA-1**, so `blobId`, `treeId` and `commitId` are the
object names `git hash-object` / `git write-tree` produce for the same content. The
encoding is git's own (real nested tree objects, `100644`/`100755`/`120000`/`40000` modes,
git's entry order with a subtree's name compared as if it ended in `/`, the commit object
layout). Every operation that mints an id goes through the `state` helpers over it —
`PutFile` and `CreateCommit` call `state::blob_id`/`state::tree_id`/`state::commit_id`, and
`GetFolder` reports `state::tree_id` of the folder it lists — and every reader (`GetFile`,
`GetBlob`, `GetCommit`, `ListFileCommitHistory`, `GetBranch`) reports the ids the writer
persisted, so all of them follow the rule through state. The id function arrived with the
git smart-HTTP transport (issue
`20261022T171422Z-codecommit-serves-no-git-smart-http-transport-and.md`), which needs the
same object names on the wire; what this change adds is the test that PINS the rule for the
control-plane responses this issue was filed about.

Covered by `tests/all/git_object_ids.rs`, which checks blob, tree, file-mode, entry-order
and commit ids over the wire against an encoder written independently of the service and
against the object names git itself publishes.

Still 501 scaffolds, each left to its own stub issue: `DeleteFile`, `CreateBranch` and the
merge operations. So of this issue's seven trace ids, every one's object-id assertion (each
is an assertion on a `PutFile` response) is answered here; the checks that go on to call
`DeleteFile` (`delete-file-restores-prior-tree-id`) or a merge with a supplied conflict
resolution (`supplied-conflict-resolution-produces-the-resolved-blob`) will next diverge on
those scaffolds' `NotImplemented`, not on an object id. Treat the object-id derivation as
fixed and those later steps as blocked on those operations' own issues.

