Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the ECR PUBLIC (SpencerFrontendService) emulator

_Filed: 2026-10-21T23:17:53Z_
_Operation: service-com-amazonaws-ecrpublic-spencerfrontendservice | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 23 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST, SMALL stateful slice: the public-repository + image lifecycle, in this order.

1. **CreateRepository** — input `repositoryName` (required, len 2..205, pattern `^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*$`), optional `catalogData` (RepositoryCatalogDataInput), optional `tags`. Must INSERT a new repository into state and return `repository` (Repository{repositoryArn, registryId, repositoryName, repositoryUri, createdAt}) plus `catalogData`. ARN form `arn:aws:ecr-public::<registryId>:repository/<repositoryName>`, URI form `public.ecr.aws/<registryAlias>/<repositoryName>`, `createdAt` set to insertion time. Re-creating an existing name must fail with `RepositoryAlreadyExistsException` (HTTP 400); a name violating the length/pattern constraint must fail with `InvalidParameterException`.
2. **DescribeRepositories** — input optional `registryId`, `repositoryNames`, `nextToken`, `maxResults` (1..1000). Must return `repositories` read back from state: the full set when `repositoryNames` is absent, exactly the named subset when present, and `RepositoryNotFoundException` when a requested name is not in state. Honour `maxResults` and emit/accept `nextToken` for the remainder (model marks the op `@paginated(items=repositories)`).
3. **PutImage** — input `repositoryName` (required), `imageManifest` (required, len 1..4194304), optional `imageManifestMediaType`, `imageTag`, `imageDigest`. Must store the manifest under the repository, compute `imageDigest` as `sha256:<hex>` over the manifest bytes when not supplied, and return `image` (Image{registryId, repositoryName, imageId{imageDigest,imageTag}, imageManifest, imageManifestMediaType}). Unknown repository → `RepositoryNotFoundException`; a supplied `imageDigest` that does not match the computed digest → `ImageDigestDoesNotMatchException`; pushing the identical digest+tag again → `ImageAlreadyExistsException`.
4. **DescribeImages** — input `repositoryName` (required), optional `imageIds`, `nextToken`, `maxResults`. Must return `imageDetails` (ImageDetail{registryId, repositoryName, imageDigest, imageTags, imageSizeInBytes, imagePushedAt, imageManifestMediaType}) derived from the images actually stored by PutImage: `imageSizeInBytes` from the stored manifest length, `imagePushedAt` from the stored push time, `imageTags` from the stored tag set. Filtering by `imageIds` (digest or tag) must work; a filter matching nothing → `ImageNotFoundException`; unknown repository → `RepositoryNotFoundException`. Honour `maxResults`/`nextToken`.
5. **BatchDeleteImage** — input `repositoryName` (required), `imageIds` (required, 1..100). Must REMOVE the matching images from state and return the deleted `imageIds` in `imageIds`, with every unmatched identifier reported in `failures` (ImageFailure with `failureCode`/`failureReason`), not silently ignored. A following DescribeImages must no longer show the deleted image.
6. **DeleteRepository** — input `repositoryName` (required), `force` (default false). Must remove the repository from state and return the deleted `repository`. When the repository still holds images and `force` is false → `RepositoryNotEmptyException`; unknown name → `RepositoryNotFoundException`. After success, DescribeRepositories for that name must return `RepositoryNotFoundException`.

Every one of these six must compute its response from persisted state held in the shared engine (reached via `crate::state::service_state(&*self.engine)`). No canned bodies, no echoing the input back, no generating an ID that is never stored: a create followed by a describe must return exactly what was created, and a delete must be observable by the next reader.

## State Model

In `src/state.rs` (`ComAmazonawsEcrpublicSpencerfrontendserviceState`):

- `registry_id: String` — the fixed 12-digit account id used for ARNs/`registryId` (e.g. "000000000000"), plus `registry_alias: String` used to build `repositoryUri`.
- `repositories: BTreeMap<String /* repositoryName */, Repository>` where
  `Repository { name: String, arn: String, registry_id: String, uri: String, created_at: SystemTime, catalog_data: RepositoryCatalogData, tags: BTreeMap<String,String>, images: BTreeMap<String /* imageDigest */, StoredImage> }`.
  Ordered map so DescribeRepositories/DescribeImages paginate deterministically.
- `StoredImage { digest: String /* "sha256:..." */, manifest: String, manifest_media_type: Option<String>, artifact_media_type: Option<String>, size_in_bytes: i64, pushed_at: SystemTime, tags: BTreeSet<String> }`.
- `tag_index`/`tags` on the repository keyed by tag name → digest so tag-based `imageIds` lookups in DescribeImages/BatchDeleteImage resolve to a stored image (and a tag re-push moves the tag between digests).

The remaining 17 operations (layer upload trio, BatchCheckLayerAvailability, repository policies, catalog-data getters/setters, DescribeRegistries, DescribeImageTags, GetAuthorizationToken, tagging) stay 501 for now and are filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-ecrpublic-spencerfrontendservice/OPERATIONS.md` lists all 23 operations with the input/output members and errors quoted above (CreateRepository lines 45-49, DeleteRepository 51-55, DescribeImages 63-68, DescribeRepositories 84-89, PutImage 127-131, BatchDeleteImage 33-37); wire protocol is awsJson1_1 with `X-Amz-Target: SpencerFrontendService.<Operation>` (OPERATIONS.md lines 8-15). Output shape fields come from the Smithy model (`Repository`, `Image`, `ImageDetail`, `ImageIdentifier`). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 23 handler files, i.e. the whole service answers HTTP 501 and persists nothing. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an ecr-public client at it (endpoint prefix `api.ecr-public`, signing name `ecr-public`).
2. CreateRepository{repositoryName: "my-team/app"} → expect 200 with repository.repositoryName == "my-team/app", a repositoryArn, a repositoryUri and a createdAt; actual: HTTP 501 NotImplemented.
3. CreateRepository with the same name again → expect RepositoryAlreadyExistsException (400).
4. DescribeRepositories{} → expect the created repository in `repositories`; DescribeRepositories{repositoryNames:["nope/x"]} → expect RepositoryNotFoundException.
5. PutImage{repositoryName: "my-team/app", imageManifest: "<manifest json>", imageTag: "v1"} → expect 200 with image.imageId.imageDigest == sha256 of the manifest and imageId.imageTag == "v1".
6. DescribeImages{repositoryName: "my-team/app"} → expect one ImageDetail whose imageDigest matches the PutImage response, imageTags == ["v1"], imageSizeInBytes == manifest byte length, imagePushedAt populated.
7. BatchDeleteImage{repositoryName: "my-team/app", imageIds:[{imageTag:"v1"}]} → expect the identifier echoed in `imageIds` and empty `failures`; then DescribeImages{repositoryName:"my-team/app"} must report ImageNotFoundException / an empty imageDetails list.
8. DeleteRepository{repositoryName: "my-team/app"} → expect 200 with the deleted repository; then DescribeRepositories{repositoryNames:["my-team/app"]} → expect RepositoryNotFoundException.
9. Re-run with an image still present and force=false → expect RepositoryNotEmptyException.
