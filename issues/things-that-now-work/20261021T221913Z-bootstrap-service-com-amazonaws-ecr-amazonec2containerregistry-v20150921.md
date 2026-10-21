Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7f122c3e96febe26bb2311916e16539819a4dcf1

# Bootstrap the ECR (Amazon EC2 Container Registry) emulator

_Filed: 2026-10-21T22:19:13Z_
_Operation: service-com-amazonaws-ecr-amazonec2containerregistry-v20150921 | Class: missing_

> _Note: this issue was identified against the code base at commit `7f122c3e96febe26bb2311916e16539819a4dcf1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 58 operations in `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501 `NotImplemented`, holding zero state. Nothing in the service works end to end.

This issue scopes the FIRST, minimal slice: the repository + image push/pull lifecycle, implemented statefully over the shared engine store (`crate::state::service_state(&*self.engine)`), on the `awsJson1_1` wire protocol (`POST /`, `X-Amz-Target: AmazonEC2ContainerRegistry_V20150921.<Op>`).

Implement these 6 operations, in this order:

1. **CreateRepository** — input `repositoryName` (required, len 2..256, pattern `^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/...)*$`), optional `registryId`, `tags`, `imageTagMutability` (MUTABLE|IMMUTABLE|...), `imageScanningConfiguration`, `encryptionConfiguration`. Output `repository` (Repository) with the fields COMPUTED and stored: `repositoryName`, `registryId` (12-digit account id), `repositoryArn` = `arn:aws:ecr:<region>:<registryId>:repository/<repositoryName>`, `repositoryUri` = `<registryId>.dkr.ecr.<region>.amazonaws.com/<repositoryName>`, `createdAt` (now), plus the echoed-and-persisted `imageTagMutability` (default `MUTABLE`), `imageScanningConfiguration`, `encryptionConfiguration` (default `{encryptionType: AES256}`). A second create with the same name must fail `RepositoryAlreadyExistsException` (HTTP 400).

2. **DescribeRepositories** — optional `repositoryNames`, `registryId`, `maxResults` (1..1000), `nextToken`. Output `repositories` (list<Repository>) read back from the store, plus `nextToken` when truncated (the op is `@paginated(inputToken=nextToken, outputToken=nextToken, items=repositories)`). An unknown name in `repositoryNames` must raise `RepositoryNotFoundException`. The Repository objects returned must be byte-for-byte what CreateRepository stored (same ARN, URI, createdAt), not re-synthesised from the request.

3. **PutImage** — input `repositoryName` (required), `imageManifest` (required, len 1..4194304), optional `imageManifestMediaType`, `imageTag`, `imageDigest`. Store an image record keyed by its digest: compute `sha256:<hex>` over the manifest bytes when `imageDigest` is absent, and when `imageDigest` IS supplied and disagrees with the computed digest, fail `ImageDigestDoesNotMatchException`. Record `imagePushedAt` (now) and `imageSizeInBytes` (manifest length is acceptable for the first slice). Output `image` (Image) = `{registryId, repositoryName, imageId:{imageDigest, imageTag}, imageManifest, imageManifestMediaType}`. Unknown repository → `RepositoryNotFoundException`. Pushing the same digest+tag again → `ImageAlreadyExistsException`; re-pushing an existing tag under `IMMUTABLE` mutability → `ImageTagAlreadyExistsException`. Pushing an existing tag under `MUTABLE` must MOVE the tag (remove it from the old digest and attach it to the new one) — an image left with no tags remains addressable by digest.

4. **ListImages** — input `repositoryName` (required), `maxResults`, `nextToken`, `filter` (`tagStatus`: TAGGED|UNTAGGED|ANY). Output `imageIds` (list<ImageIdentifier> = `{imageDigest, imageTag}`) derived from the stored images — one entry per (digest, tag) pair and one tagless entry for an untagged digest — honouring `maxResults`/`nextToken` and `filter.tagStatus`. Unknown repository → `RepositoryNotFoundException`.

5. **DescribeImages** — input `repositoryName` (required), optional `imageIds`, `filter`, `maxResults`, `nextToken`. Output `imageDetails` (list<ImageDetail>) built from the store: `registryId`, `repositoryName`, `imageDigest`, `imageTags` (ALL tags on that digest), `imageSizeInBytes`, `imagePushedAt`, `imageManifestMediaType`, plus `nextToken` when truncated. An `imageIds` entry that matches no stored image → `ImageNotFoundException`.

6. **DeleteRepository** — input `repositoryName` (required), `force` (boolean, default false). Remove the repository and all its images from the store and return the deleted `repository` (Repository) as it was stored. Deleting a repository that still holds images with `force=false` must fail `RepositoryNotEmptyException`; with `force=true` it must succeed and drop the images. A following DescribeRepositories / ListImages for that name must raise `RepositoryNotFoundException`.

NEVER STUBS: each of these six must compute its response from the persisted store. Hardcoded Repository/Image bodies, echoing the request back without storing, or minting a digest without recording it are failures of this issue. All remaining 52 operations (layer upload — InitiateLayerUpload/UploadLayerPart/CompleteLayerUpload/BatchCheckLayerAvailability, GetAuthorizationToken, GetDownloadUrlForLayer, policies, lifecycle policies, scanning, replication, signing, pull-through cache, repository creation templates, tags, account settings) stay at 501 and will be filed as individual gap issues later.

## State Model

A single service state struct in `src/state.rs`, reached via `crate::state::service_state(&*self.engine)` and mutated under its lock:

- `registry_id: String` — the emulator's 12-digit account id (e.g. `"000000000000"`), used for `registryId`, ARNs and repository URIs.
- `repositories: BTreeMap<String /* repositoryName */, Repo>` — BTreeMap so DescribeRepositories/ListImages have a stable, deterministic order for pagination tokens.
- `Repo { name: String, registry_id: String, arn: String, uri: String, created_at: SystemTime, image_tag_mutability: ImageTagMutability /* default MUTABLE */, image_tag_mutability_exclusion_filters: Vec<ImageTagMutabilityExclusionFilter>, image_scanning_config: Option<ImageScanningConfiguration>, encryption_config: EncryptionConfiguration /* default AES256 */, tags: Vec<Tag>, images: BTreeMap<String /* imageDigest */, ImageRecord>, tag_index: BTreeMap<String /* imageTag */, String /* imageDigest */> }`
- `ImageRecord { digest: String, tags: BTreeSet<String>, manifest: String, manifest_media_type: Option<String>, artifact_media_type: Option<String>, size_in_bytes: i64, pushed_at: SystemTime }`

`tag_index` is the authority for tag→digest so PutImage can move a mutable tag atomically and ListImages/DescribeImages can report every tag on a digest. Pagination `nextToken` is an opaque encoding of the last key returned from the relevant BTreeMap, resolved against live state on the next call.

Done = a client script (AWS CLI/SDK against the emulator endpoint) runs CreateRepository → DescribeRepositories → PutImage → ListImages → DescribeImages → BatchGetImage-free delete path → DeleteRepository with correct bodies at every step, not merely 2xx.

## Evidence

`OPERATIONS.md` at the crate root (58 operations, protocol `awsJson1_1`, target prefix `AmazonEC2ContainerRegistry_V20150921`) documents each signature cited above: CreateRepository (lines 63-67: in `repositoryName` required + `tags`/`imageTagMutability`/`imageScanningConfiguration`/`encryptionConfiguration`, out `repository`, errs `RepositoryAlreadyExistsException`), DescribeRepositories (162-167: `@paginated(items=repositories)`, out `repositories`+`nextToken`, err `RepositoryNotFoundException`), PutImage (268-272: `imageManifest` required, out `image`, errs `ImageAlreadyExistsException`/`ImageDigestDoesNotMatchException`/`ImageTagAlreadyExistsException`), ListImages (243-248: `@paginated(items=imageIds)`, `filter`), DescribeImages (129-134: `@paginated(items=imageDetails)`, err `ImageNotFoundException`), DeleteRepository (93-97: `force` default false, out `repository`, err `RepositoryNotEmptyException`). Field lists for Repository, Image, ImageDetail and ImageIdentifier come from `smithy_query` on the model (`repositoryArn`, `repositoryUri`, `createdAt`; `imageId`/`imageManifest`; `imageTags`/`imageSizeInBytes`/`imagePushedAt`). Proof of the all-stub state: `grep -rn '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches every operation file, including `create_repository.rs:4`, `put_image.rs:4`, `list_images.rs:4`, `describe_images.rs:4`, `describe_repositories.rs:4` and `delete_repository.rs:4`; each `handle` returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. No test asserts any of this behaviour (only the generated `tests/state_roundtrip.rs` exists).

## Steps to Reproduce

1. Start the emulator and point an ECR client at it (region us-east-1, any credentials).
2. CreateRepository{repositoryName:"my-app"} → expect 200 with repository.repositoryArn=arn:aws:ecr:us-east-1:000000000000:repository/my-app, repositoryUri=000000000000.dkr.ecr.us-east-1.amazonaws.com/my-app, createdAt set, imageTagMutability=MUTABLE. Actual: HTTP 501 NotImplemented.
3. CreateRepository{repositoryName:"my-app"} again → expect 400 RepositoryAlreadyExistsException.
4. DescribeRepositories{} → expect the one repository with exactly the ARN/URI/createdAt returned by step 2.
5. PutImage{repositoryName:"my-app", imageManifest:"{...}", imageTag:"v1"} → expect 200 with image.imageId.imageDigest=sha256:<digest of the manifest> and imageId.imageTag="v1".
6. PutImage same manifest/tag again → expect 400 ImageAlreadyExistsException.
7. ListImages{repositoryName:"my-app"} → expect imageIds=[{imageDigest:<same digest>, imageTag:"v1"}].
8. DescribeImages{repositoryName:"my-app"} → expect one imageDetail with imageTags=["v1"], imageSizeInBytes>0 and imagePushedAt set.
9. PutImage{repositoryName:"my-app", imageManifest:"{...v2...}", imageTag:"v1"} on a MUTABLE repo → expect 200 and ListImages now maps tag v1 to the NEW digest, old digest still listed untagged.
10. DescribeImages{repositoryName:"my-app", imageIds:[{imageTag:"nope"}]} → expect 400 ImageNotFoundException.
11. DeleteRepository{repositoryName:"my-app", force:false} → expect 400 RepositoryNotEmptyException; with force:true expect 200 returning the stored repository.
12. DescribeRepositories{repositoryNames:["my-app"]} → expect 400 RepositoryNotFoundException.
13. ListImages{repositoryName:"missing-repo"} → expect 400 RepositoryNotFoundException, not 501.
