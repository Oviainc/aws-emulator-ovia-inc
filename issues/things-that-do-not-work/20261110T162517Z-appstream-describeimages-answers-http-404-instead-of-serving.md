Issue-Source: trace-sweep-divergence
Issue-Trace: appstream-catalog-describe-images
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appstream-catalog-describe-images
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream|DescribeImages|2xx->4xx|$.Images
Issue-Key: appstream|DescribeImages|2xx->4xx|status
Issue-Commit: e88affa0c81a58fb8423ccb328f6804dccd04335

# appstream DescribeImages answers HTTP 404 instead of serving the account's image catalog

> _Note: this issue was identified against the code base at commit `e88affa0c81a58fb8423ccb328f6804dccd04335`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Cause 1 of 1: the AppStream DescribeImages read path has no image catalog behind it, so every catalog read — unfiltered, name-filtered, ARN-filtered, visibility-filtered and paged — is refused with HTTP 404 instead of returning an Images list.

AWS: AppStream 2.0 DescribeImages is a plain paginated read of the images visible to the calling account: the AWS-published base images (Visibility PUBLIC) plus any images the account owns or has had shared with it. Smithy (com-amazonaws-appstream-photonadminproxyservice): DescribeImages input members Names (StringList), Arns (ArnList), Type (VisibilityType enum), NextToken (@length min 1), MaxResults (DescribeImagesMaxResults, @range 0..25); output members Images (ImageList) and NextToken; modeled errors InvalidParameterCombinationException (HTTP 400) and ResourceNotFoundException (HTTP 404). A call that names no image — or that names an image which does exist — is answered 200 with Images populated; ResourceNotFoundException is reserved for a Names/Arns filter naming an image that does not exist, and is never the answer to an unfiltered catalog listing. Each Image AWS returns is fully projected: Name, Arn, DisplayName, Description, Platform, State, Visibility, ImageType, ImageBuilderSupported, InstanceFamily, SupportedInstanceFamilies, RootVolumeConfig, Applications, ImageErrors, Byol, CreatedTime and the agent/recipe metadata members. The AWS-published base-image set is inventory, not a per-run value: the recording shows PUBLIC RHEL8 Graphics-G4dn base images in State AVAILABLE with ImageType NATIVE and InstanceFamily GRAPHICS_G4, and the specific names, timestamps and recipe ARNs in any one recording are example values, not requirements.

Emulator: every step of the catalog fixture is refused. The replay detail is `status: expected 200, got 404` and `$.Images: missing (expected [...], got None)` — all 8 steps of appstream-catalog-describe-images (task#1 through task#8) diverge identically, so no filter form, no visibility selection and no NextToken page is served. HTTP 404 is the status the model binds to ResourceNotFoundException, i.e. the emulator reports the image catalog as absent rather than empty-or-populated.

Why: a client cannot enumerate AppStream images at all. Choosing an ImageName/ImageArn for a fleet or an image builder, validating that an image exists before referencing it, and distinguishing a genuinely missing image (ResourceNotFoundException) from a healthy catalog are all impossible — and because the refusal is a 404 rather than an empty 200, SDK callers see a hard not-found error on a read that AWS always succeeds, so retry and fallback logic that treats not-found as fatal aborts the whole provisioning path.

Fix: belongs in the appstream crate's DescribeImages handler and the image catalog in its state — services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops.rs (the DescribeImages op) over an image collection seeded in src/state.rs with the AWS-published PUBLIC base images and extended with images created in-emulator. The handler must answer 200 with the Images list (empty list, not 404, when nothing matches a visibility filter), project every modeled Image member, apply the Names/Arns/Type filters, honour MaxResults within its 0..25 range with NextToken paging, and raise ResourceNotFoundException only for a Names/Arns filter naming an image that does not exist.

Traces: fixture appstream-catalog-describe-images, steps task#1..task#8 (all 8 steps of the cluster). Note: the resolved issue 20261109T123922Z-appstream-cloudformation-describeimages-is-an-unimplemented-scaffold-stub.md covered the earlier 501 scaffold form of this operation and does not cover this 404; no open issue owns this cluster (owned_by is empty in the replay report).

## Evidence
Expected (recorded AWS response, fixture appstream-catalog-describe-images step task#1): HTTP 200 with an Images list of AWS-published base images, each fully projected — e.g. an entry with Name/DisplayName 'AppStream-Graphics-G4dn-RHEL8-<date>', Arn 'arn:aws:appstream:<region>::image/<name>', Visibility 'PUBLIC', State 'AVAILABLE', ImageType 'NATIVE', InstanceFamily 'GRAPHICS_G4', ImageBuilderSupported true, RootVolumeConfig {VolumeSizeInGb, VolumeType}, Applications [], ImageErrors [], Byol false, CreatedTime, SupportedInstanceFamilies ['Graphics G4'] (the concrete names, dates and recipe ARNs are the recording's example values).

Actual (emulator): HTTP 404 with no Images member. Replay detail, verbatim:
  status: expected 200, got 404
  $.Images: missing (expected [{...AWS-published base images...}], got None)

Cluster: service=appstream|DescribeImages|status=200->404|keys=$.Images,status — affected_count 8, every step of the fixture: appstream-catalog-describe-images#task#1 .. #task#8. owned_by: [] and stub_path: "" (net-new, not blocked by an unimplemented-operation stub).

## Steps to Reproduce
1. Boot the emulator from this checkout (the appstream service is bootstrapped; /__admin/health answers 200).
2. Call AppStream DescribeImages with no filters (e.g. aws appstream describe-images --endpoint-url <emulator>).
3. Observe HTTP 404 with no Images member, instead of HTTP 200 with the account-visible image catalog (the AWS-published PUBLIC base images).
4. Repeat with --type PUBLIC, with --names <a base image name>, with --arns <a base image arn> and with --max-results 1 to walk NextToken: each is refused 404 the same way.
5. Re-replay fixture appstream-catalog-describe-images (steps task#1..task#8) to confirm the fix.
