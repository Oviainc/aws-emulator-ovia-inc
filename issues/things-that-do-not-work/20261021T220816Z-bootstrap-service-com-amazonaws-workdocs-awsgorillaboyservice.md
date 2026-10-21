Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the com-amazonaws-workdocs-awsgorillaboyservice (WorkDocs) emulator

_Filed: 2026-10-21T22:08:16Z_
_Operation: service-com-amazonaws-workdocs-awsgorillaboyservice | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 44 operations in `src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the crate holds zero state. No client workflow works.

Implement a FIRST, SMALL, stateful slice: the document lifecycle inside a folder (restJson1, SDK id `WorkDocs`, signing name `workdocs`). These 6 operations, in order, must work end-to-end against persisted state:

1. **CreateFolder** — `POST /api/v1/folders` -> 201. Input: `ParentFolderId` (required), `Name`. Must verify the parent folder exists (else `EntityNotExistsException`, HTTP 404), mint a new folder id, store the folder, and return `Metadata` (FolderMetadata: `Id`, `Name`, `CreatorId`, `ParentFolderId`, `CreatedTimestamp`, `ModifiedTimestamp`, `ResourceState` = ACTIVE, `Signature`). The returned `Id` must be the key a later GetFolder/DescribeFolderContents resolves.
2. **InitiateDocumentVersionUpload** — `POST /api/v1/documents` -> 201. Input: `Name`, `ParentFolderId`, `ContentType`, `DocumentSizeInBytes`, `ContentCreatedTimestamp`, `ContentModifiedTimestamp`, optional `Id`. Must create a document record plus its first version in state with `VersionStatus` = INITIALIZED (`ResourceState` = ACTIVE) under the named parent folder, and return `Metadata` (DocumentMetadata: `Id`, `CreatorId`, `ParentFolderId`, `CreatedTimestamp`, `ModifiedTimestamp`, `LatestVersionMetadata`, `ResourceState`) and `UploadMetadata` (`UploadUrl` + `SignedHeaders`). A repeat with an `Id` that already exists must fail `EntityAlreadyExistsException` (409); an unknown `ParentFolderId` must fail `EntityNotExistsException` (404).
3. **UpdateDocumentVersion** — `PATCH /api/v1/documents/{DocumentId}/versions/{VersionId}` -> 200. Input `VersionStatus` = ACTIVE. Must flip the stored version's status from INITIALIZED to ACTIVE and update the document's `LatestVersionMetadata`. Unknown document/version -> `EntityNotExistsException` (404); a version not in INITIALIZED state -> `ProhibitedStateException` (409).
4. **GetDocument** — `GET /api/v1/documents/{DocumentId}` -> 200. Must return the `Metadata` that was actually stored in step 2/3 (same `Id`, `Name`, `ParentFolderId`, `ContentType`, `DocumentSizeInBytes`, version status), and `CustomMetadata` only when `IncludeCustomMetadata` is true. Unknown id -> `EntityNotExistsException` (404), NOT a synthesized document.
5. **DescribeFolderContents** — `GET /api/v1/folders/{FolderId}/contents` -> 200. Must list the `Folders` and `Documents` actually parented to `FolderId` from state, honour `Type` (ALL|DOCUMENT|FOLDER), `Sort` (DATE|NAME) + `Order` (ASCENDING|DESCENDING), and `Limit`/`Marker` paging (emit `Marker` only when a further page exists; an opaque, resolvable token). Unknown folder -> `EntityNotExistsException` (404).
6. **DeleteDocument** — `DELETE /api/v1/documents/{DocumentId}` -> 204 with an EMPTY body. Must remove the document (and its versions) from state so the following GetDocument answers `EntityNotExistsException` (404) and DescribeFolderContents no longer lists it. Unknown id -> 404.

Rules for the slice:
- NEVER STUBS: every one of the six must compute its response from persisted state. Hardcoded/echoed metadata, a minted id that is never stored, or an empty 200/201 are all failures of this issue.
- A root folder must exist for step 1 to have a valid `ParentFolderId`. Seed one root folder per (synthetic) user at state init and have `DescribeRootFolders` return it, OR document the seeded root id; everything else stays 501.
- Status codes and error shapes come from the model: 201 for CreateFolder/InitiateDocumentVersionUpload, 200 for UpdateDocumentVersion/GetDocument/DescribeFolderContents, 204 for DeleteDocument; `EntityNotExistsException` 404, `EntityAlreadyExistsException` 409, `ProhibitedStateException` 409, `InvalidArgumentException` 400.
- Timestamps must be real and monotone: `ModifiedTimestamp` must advance on UpdateDocumentVersion.

## State Model

In `ComAmazonawsWorkdocsAwsgorillaboyserviceState` (reached through `crate::state::service_state(&*self.engine)`; one shared engine per process):

- `folders: HashMap<String /*FolderId*/, Folder>` where `Folder { id, name, creator_id, parent_folder_id: Option<String>, created: SystemTime, modified: SystemTime, resource_state: ResourceStateType, signature: Option<String>, labels: Vec<String>, custom_metadata: HashMap<String,String> }`
- `documents: HashMap<String /*DocumentId*/, Document>` where `Document { id, name, creator_id, parent_folder_id: String, created: SystemTime, modified: SystemTime, resource_state: ResourceStateType, latest_version_id: String, versions: BTreeMap<String /*VersionId*/, DocumentVersion>, labels: Vec<String>, custom_metadata: HashMap<String,String> }`
- `DocumentVersion { id, name, content_type: Option<String>, size_in_bytes: Option<i64>, status: DocumentVersionStatus /*INITIALIZED|ACTIVE*/, created: SystemTime, modified: SystemTime, content_created: Option<SystemTime>, content_modified: Option<SystemTime>, creator_id, signature: Option<String>, thumbnail: HashMap<String,String>, source: HashMap<String,String> }`
- `folder_children: ` derived by scanning `parent_folder_id` (or an explicit `HashMap<String, Vec<ChildRef>>` kept in sync) so DescribeFolderContents is a state read, not a guess.
- `root_folders: HashMap<String /*UserId*/, String /*FolderId*/>` seeded at init.
- `next_id: u64` (or a UUID source) for minting folder/document/version ids — ids MUST be stored, never minted and discarded.

Done = a client script running steps 1-6 in sequence succeeds with correct, stateful bodies (the document GetDocument returns is the one uploaded; DescribeFolderContents lists exactly the created children; after DeleteDocument the read 404s). All other operations (comments, permissions, labels, users, notifications, search, paths, restore) remain 501 and are filed separately as gap issues.

## Evidence

OPERATIONS.md at the crate root documents all 44 operations and their wire contracts: `## CreateFolder` (`POST /api/v1/folders -> 201`, input `ParentFolderId` required, output `Metadata` FolderMetadata), `## InitiateDocumentVersionUpload` (`POST /api/v1/documents -> 201`, output `Metadata` + `UploadMetadata`), `## UpdateDocumentVersion` (`PATCH /api/v1/documents/{DocumentId}/versions/{VersionId} -> 200`, `VersionStatus` one of ACTIVE), `## GetDocument` (`GET /api/v1/documents/{DocumentId} -> 200`, output `Metadata` + `CustomMetadata`, `IncludeCustomMetadata` default false), `## DescribeFolderContents` (`GET /api/v1/folders/{FolderId}/contents -> 200`, @paginated(inputToken=Marker, outputToken=Marker), outputs `Folders`/`Documents`/`Marker`, query params Sort/Order/Limit/Type), `## DeleteDocument` (`DELETE /api/v1/documents/{DocumentId} -> 204`, no output members). Wire protocol section: restJson1, SDK id WorkDocs, signing name/endpoint prefix `workdocs`. Error HTTP codes (EntityNotExistsException 404, EntityAlreadyExistsException 409, ProhibitedStateException 409, InvalidArgumentException 400) are taken from the same entries. Proof of all-stubs: every file under services/service-com-amazonaws-workdocs-awsgorillaboyservice/src/ops/ is a scaffold — e.g. src/ops/create_folder.rs lines 4-6 carry `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 180 is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateFolder"))`, with line 170 `let _ = (&input, ctx, &self.engine);` showing input and state are untouched. No tests exist beyond tests/state_roundtrip.rs (a compiled example of the state API), so nothing asserts this behaviour today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK WorkDocs client at it (region any, signing name `workdocs`).
2. Discover a root folder id (DescribeRootFolders or the documented seeded root) and call CreateFolder with ParentFolderId=<root>, Name="bootstrap-folder" — expect 201 and Metadata.Id; today: HTTP 501 NotImplemented.
3. Call InitiateDocumentVersionUpload with ParentFolderId=<new folder id>, Name="hello.txt", ContentType="text/plain", DocumentSizeInBytes=11 — expect 201 with Metadata.Id, Metadata.LatestVersionMetadata.Id (status INITIALIZED) and UploadMetadata.UploadUrl.
4. Call UpdateDocumentVersion(DocumentId=<doc id>, VersionId=<version id>, VersionStatus=ACTIVE) — expect 200 and the stored version flipped to ACTIVE.
5. Call GetDocument(DocumentId=<doc id>) — expect 200 with Metadata.Name == "hello.txt", ParentFolderId == <new folder id>, LatestVersionMetadata.Status == ACTIVE (values from state, not echoes of this request).
6. Call DescribeFolderContents(FolderId=<new folder id>, Type=ALL) — expect 200 listing exactly that one document in Documents and no Folders; repeat with Limit=1 and follow Marker to prove paging resolves.
7. Call DeleteDocument(DocumentId=<doc id>) — expect 204 with an empty body.
8. Call GetDocument(DocumentId=<doc id>) again — expect 404 EntityNotExistsException, and DescribeFolderContents on the folder now returns an empty Documents list.
9. Call GetDocument with a never-created id — expect 404 EntityNotExistsException, not 501 and not a synthesized document.
