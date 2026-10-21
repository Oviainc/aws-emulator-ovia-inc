Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the codecommit emulator

_Filed: 2026-10-21T21:37:53Z_
_Operation: service-com-amazonaws-codecommit-codecommit-20150413 | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 80 operations in `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding no state, so no CodeCommit client can do anything. Implement a FIRST, SMALL happy-path slice: the repository lifecycle plus one file write/read, statefully, over awsJson1_1 (`POST /`, `X-Amz-Target: CodeCommit_20150413.<Operation>`).

Happy-path operations, in order:

1. **CreateRepository** — input `repositoryName` (required, len 1..100, pattern `^[\w\.-]+$`), `repositoryDescription`, `tags`, `kmsKeyId`. Must persist a new repository and return `repositoryMetadata` with freshly generated `repositoryId` (uuid), `accountId`, `repositoryName`, `repositoryDescription` as given, `Arn` = `arn:aws:codecommit:<region>:<account>:<repositoryName>`, `cloneUrlHttp`/`cloneUrlSsh` derived from the name, `creationDate`/`lastModifiedDate` set to now. A second create of the same name must fail with `RepositoryNameExistsException` (HTTP 400). `defaultBranch` is absent until the first commit creates a branch.
2. **PutFile** — input `repositoryName`, `branchName` (required), `fileContent` (blob, required), `filePath` (required), optional `fileMode` (default `NORMAL`), `parentCommitId`, `commitMessage`, `name`, `email`. Must create the branch if the repository is empty, store the file bytes/mode under (repo, branch, path), append a commit, and return the newly generated `commitId`, `blobId` and `treeId` — all computed, not canned. Unknown repository → `RepositoryDoesNotExistException`; a `parentCommitId` that is not the branch tip → `ParentCommitIdOutdatedException`. The first PutFile on a new repository must also set `repositoryMetadata.defaultBranch`.
3. **GetFile** — input `repositoryName`, `filePath` (required), optional `commitSpecifier` (defaults to the default branch). Must return the stored bytes in `fileContent`, with `commitId`, `blobId`, `filePath`, `fileMode` and `fileSize` matching what PutFile wrote. Unknown path → `FileDoesNotExistException`.
4. **GetBranch** — input `repositoryName`, `branchName`. Must return `branch` = BranchInfo{`branchName`, `commitId` = the tip commit written by PutFile}. Unknown branch → `BranchDoesNotExistException`.
5. **GetRepository** — input `repositoryName` (required). Must return the SAME `repositoryMetadata` that CreateRepository returned (same id/arn/creationDate, `defaultBranch` now populated, `lastModifiedDate` advanced by the PutFile). Unknown name → `RepositoryDoesNotExistException`.
6. **ListRepositories** — input `nextToken`, `sortBy` (`repositoryName` | `lastModifiedDate`), `order` (`ascending` | `descending`). Must return `repositories` as RepositoryNameIdPair{`repositoryName`, `repositoryId`} for every live repository, honoring sortBy/order, and omit deleted ones.
7. **DeleteRepository** — input `repositoryName` (required). Must remove the repository (and its branches/files/commits) and return its `repositoryId`. A subsequent GetRepository must fail with `RepositoryDoesNotExistException`, and ListRepositories must no longer list it. Deleting a non-existent repository returns an empty/absent `repositoryId` (idempotent, no error).

Every response must be computed from the persisted store reached via `crate::state::service_state(&*self.engine)` — no hardcoded ids, no echoing the input back without storing it, no random id without a corresponding stored object. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each handler you implement.

## State Model

In `src/state.rs` (the per-service store held by the shared engine):

- `repositories: BTreeMap<String /*repositoryName*/, Repository>`
- `Repository { repository_id: String /*uuid*/, account_id: String, name: String, description: Option<String>, default_branch: Option<String>, arn: String, clone_url_http: String, clone_url_ssh: String, kms_key_id: Option<String>, creation_date: SystemTime, last_modified_date: SystemTime, tags: BTreeMap<String,String>, branches: BTreeMap<String /*branchName*/, Branch>, commits: BTreeMap<String /*commitId*/, Commit>, blobs: BTreeMap<String /*blobId*/, Vec<u8>> }`
- `Branch { name: String, commit_id: String /*tip*/ }`
- `Commit { commit_id: String, tree_id: String, parents: Vec<String>, message: Option<String>, author: UserInfo, committer: UserInfo, files: BTreeMap<String /*filePath*/, FileEntry> }` — the `files` map is the full tree snapshot at that commit, so GetFile at any `commitSpecifier` is a lookup, not a replay.
- `FileEntry { blob_id: String, file_mode: FileModeTypeEnum, size: i64 }`
- `UserInfo { name: Option<String>, email: Option<String>, date: String /*unix seconds + GMT offset*/ }`

Ids: `commitId`/`treeId`/`blobId` as 40-hex-char SHA-1-shaped strings (hash of content + parent for stability), `repositoryId` as a uuid. Out-of-scope operations (pull requests, approval rule templates, merges, comments, triggers, diffs, tags) stay at 501 and are filed as individual gap issues later.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/` matches every operation file (create_repository.rs, get_repository.rs, list_repositories.rs, put_file.rs, get_file.rs, get_branch.rs, delete_repository.rs, ...), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501 with no state. Input/output members and error names above are taken from the crate's OPERATIONS.md (CreateRepository, PutFile, GetFile, GetBranch, GetRepository, ListRepositories, DeleteRepository sections) and the Smithy shapes RepositoryMetadata, BranchInfo, RepositoryNameIdPair, Commit queried from `smithy-model/com-amazonaws-codecommit-codecommit-20150413.json`. Protocol (awsJson1_1, target prefix `CodeCommit_20150413`) from OPERATIONS.md "Wire protocol".

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI codecommit client at it.
2. `aws codecommit create-repository --repository-name demo-repo --repository-description 'bootstrap demo'` → expect repositoryMetadata with a generated repositoryId, Arn, cloneUrl*, creationDate (today: 501).
3. Repeat the same create-repository call → expect RepositoryNameExistsException (HTTP 400).
4. `aws codecommit put-file --repository-name demo-repo --branch-name main --file-path README.md --file-content fileb://README.md --commit-message 'init'` → expect generated commitId/blobId/treeId (today: 501).
5. `aws codecommit get-file --repository-name demo-repo --file-path README.md` → expect fileContent byte-identical to what was put, with matching blobId/commitId/fileMode/fileSize.
6. `aws codecommit get-branch --repository-name demo-repo --branch-name main` → expect branch.commitId equal to the commitId returned by put-file.
7. `aws codecommit get-repository --repository-name demo-repo` → expect the same repositoryId/Arn/creationDate as the create response, with defaultBranch == main.
8. `aws codecommit list-repositories` → expect demo-repo listed with its repositoryId.
9. `aws codecommit delete-repository --repository-name demo-repo` → expect the repositoryId back; then get-repository → RepositoryDoesNotExistException and list-repositories no longer lists it.
