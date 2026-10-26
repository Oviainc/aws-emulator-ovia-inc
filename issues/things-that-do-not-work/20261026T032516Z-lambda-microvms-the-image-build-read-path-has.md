Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.get-microvm-image-build-access-denied
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms.get-microvm-image-build-access-denied|run#9|AssertionError|assert real_list_code == "AccessDeniedException", real_list_code
Issue-Commit: 3fab586e83994b02c2f6ed8f8ac57fb3c1f534fc

# lambda-microvms: the image-build read path has no authorization verdict — GetMicrovmImageBuild/ListMicrovmImageBuilds answer 501 NotImplemented where AWS denies with AccessDeniedException (403)

> _Note: this issue was identified against the code base at commit `3fab586e83994b02c2f6ed8f8ac57fb3c1f534fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMicrovmImageBuild

## Description
This issue carries 1 distinct root cause in the lambda-microvms emulator, grounded in failing behavioral check lambda-microvms.get-microvm-image-build-access-denied. A fixer must close Cause 1 of 1 before submitting.

=== Cause 1 of 1: the lambda-microvms build-read authorization boundary does not exist; both build reads answer 501 NotImplemented instead of the modeled AccessDeniedException (403) ===
Failing check: lambda-microvms.get-microvm-image-build-access-denied

AWS: both build reads model AccessDeniedException as a client error with httpError 403 and the documented text "You do not have sufficient access to perform this action." (Smithy: GetMicrovmImageBuild errors = AccessDeniedException(403) | InternalServerException(500) | ResourceNotFoundException(404) | ThrottlingException(429) | ValidationException(400), GET /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}/builds/{buildId}; ListMicrovmImageBuilds errors are the identical set, GET /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}/builds; com.amazonaws.lambdamicrovms#AccessDeniedException = {message: String}, errorKind client, httpError 403). Recorded behavior adds WHEN that error is chosen and that it is chosen BEFORE any existence check: for the verification principal, a build read on an AWS-managed base image chain that unambiguously exists up to the version (an imageArn from ListManagedMicrovmImages plus an imageVersion from ListManagedMicrovmImageVersions on it) is DENIED — ListMicrovmImageBuilds on that real image/version answers AccessDeniedException, and GetMicrovmImageBuild on the same image/version with ANY buildId answers AccessDeniedException. The same code is returned for a well-formed but entirely nonexistent SAME-ACCOUNT triple (arn:aws:lambda:<region>:<accountId>:microvm-image/<name that was never created>, imageVersion "1.0", a buildId that does not exist): AccessDeniedException, never a permission-bypassing ResourceNotFoundException and never a silent 2xx. In the AccessDeniedException message AWS names the CALLING PRINCIPAL's own ARN, so the message's variable part is caller identity (not anything the request creates) and the requirement here is the error CODE AccessDeniedException with HTTP status 403; the only fixed text the model states for this error class is "You do not have sufficient access to perform this action." This mirrors the authorization boundary already recorded on GetMicrovmImage for aws-owned identifiers (see open issue 20261025T210105Z-lambda-microvms-image-builds-never-fail-image-store.md, Cause 3 of 3), extended here to the build-read plane and additionally to a nonexistent own-account identifier.

Emulator: neither build read has any behavior at all. services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image_build.rs and .../src/ops/list_microvm_image_builds.rs both still return Err(crate::scaffolded::Scaffolded::not_implemented(..)) from handle(), so every build read answers HTTP 501 with Error.Code "NotImplemented" and message "operation <Op> is scaffolded but not implemented in this emulator". The check's first probe therefore collects code "NotImplemented" where "AccessDeniedException" is required: AssertionError: NotImplemented at check_lambda_microvms_get_microvm_image_build_access_denied.py:41. Everything the check needs to reach that probe DID work — ListManagedMicrovmImages returned at least one Lambda-managed base image and ListManagedMicrovmImageVersions returned at least one version for it — so the divergence is confined to the build-read verdict and is not a setup failure. Note that the stub 501 is itself CORRECT for an unwritten operation: the defect filed here is that the operation has no authorization verdict, i.e. implementing it must produce the 403 above, not that 501 should be relabelled. ListMicrovmImageBuilds' bare stub status is already filed as 20261025T210105Z-lambda-microvms-listmicrovmimagebuilds-stub.md (grounded only in lambda-microvms.image-build-fans-out-one-build-per-chipset-generation); that issue does not describe the 403 boundary and no open issue covers GetMicrovmImageBuild at all, which is why this is filed separately and names them.

Why: a client cannot tell "you may not read this" from "this emulator has not been written". 501/NotImplemented is unmodeled for these operations, so SDK error-classification and retry layers treat it as a transport/server fault and may retry forever instead of surfacing a permission error, and a caller that distinguishes 403 from 404 to decide "the base image exists but I am not allowed to inspect its builds" vs "create it" gets neither answer. Worse, when the boundary is implemented as an existence check first, a caller learns whether an identifier exists in an account it has no permission on — the information leak AWS prevents by denying before the lookup.

Fix: the two build-read handlers and the shared authorization predicate they must consult — services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image_build.rs and .../src/ops/list_microvm_image_builds.rs, using the AWS-owned/managed-namespace predicate that already exists in crate::state (managed_microvm_image_id / is_published_managed_base_image, as used by create_microvm_image.rs) and the identifier resolver crate::state::resolve_image_reference. The verdict must be decided BEFORE the image/version/build lookup, so an aws-owned identifier and an unresolvable same-account identifier both return the modeled AccessDeniedException (403) rather than ResourceNotFoundException (404) or the scaffold's 501; delete each file's '//! SCAFFOLDED, NOT IMPLEMENTED:' marker line only once the handler is real.

Traces: lambda-microvms.get-microvm-image-build-access-denied (asserts, in order, ListMicrovmImageBuilds on a real managed image/version -> AccessDeniedException; GetMicrovmImageBuild on the same image/version with a probe buildId -> AccessDeniedException; GetMicrovmImageBuild on a bogus same-account image/version/build -> AccessDeniedException). Citations: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image_build.rs lines 3-6 (scaffold marker) and 166-184 (handle returns not_implemented); services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/list_microvm_image_builds.rs lines 3-6 and 159-177 (same).

## Evidence
Behavioral suite: 0/36 passed (mismatched 6, errored 30). Of the 11 reported failing clusters, 10 are marked ALREADY FILED against unimplemented scaffold stubs (EC2 DescribeVpcEndpointServices; lambda-microvms GetMicrovmImageVersion, TagResource, ListMicrovmImages, ListMicrovmImageBuilds, ListTags, UpdateMicrovmImage) or OWNED by 20261025T210105Z-lambda-microvms-image-builds-never-fail-image-store.md. This cluster is the one that is neither.

lambda-microvms.get-microvm-image-build-access-denied [mismatch]
  Setup that SUCCEEDED (so this is not a broken-setup failure):
    ListManagedMicrovmImages -> items[0].imageArn (an aws-owned managed base image ARN)
    ListManagedMicrovmImageVersions(imageIdentifier=<that arn>) -> items[0].imageVersion
  Probe 1 — expected (AWS): ListMicrovmImageBuilds(imageIdentifier=<managed arn>, imageVersion=<real version>) -> Error.Code "AccessDeniedException", HTTP 403 (message names the calling principal's ARN, so only the code/status are required).
    Actual (emulator): Error.Code "NotImplemented", HTTP 501, message "operation ListMicrovmImageBuilds is scaffolded but not implemented in this emulator".
    check_lambda_microvms_get_microvm_image_build_access_denied.py:41
    AssertionError: NotImplemented
  Probe 2 — expected (AWS): GetMicrovmImageBuild(imageIdentifier=<managed arn>, imageVersion=<real version>, buildId=<probe-build-...>) -> AccessDeniedException, HTTP 403.
    Actual (emulator): not reached (the check stops at probe 1), but the handler is the same scaffold and answers Error.Code "NotImplemented", HTTP 501.
  Probe 3 — expected (AWS): GetMicrovmImageBuild(imageIdentifier=arn:aws:lambda:<region>:<accountId>:microvm-image/<never-created>, imageVersion="1.0", buildId="build-does-not-exist") -> AccessDeniedException, HTTP 403 — denied BEFORE the existence check, never ResourceNotFoundException and never a 2xx.
    Actual (emulator): not reached; same scaffold 501.

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json):
  GetMicrovmImageBuild: GET /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}/builds/{buildId}, readonly; errors AccessDeniedException(client,403), InternalServerException(server,500), ResourceNotFoundException(client,404), ThrottlingException(client,429), ValidationException(client,400)
  ListMicrovmImageBuilds: GET /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}/builds, readonly, paginated; identical error set
  AccessDeniedException: errorKind client, httpError 403, members {message: String}, doc "You do not have sufficient access to perform this action."
  ResourceNotFoundException: errorKind client, httpError 404, message @required, doc "The specified resource does not exist."
  NotImplemented/501 is NOT a modeled outcome of either operation.

Code state: both handlers still carry the '//! SCAFFOLDED, NOT IMPLEMENTED:' census marker and return crate::scaffolded::Scaffolded::not_implemented(..).

## Steps to Reproduce
1. Boot the emulator and create a client for lambda-microvms (mvm) in any region.
2. images = mvm.list_managed_microvm_images()['items']; arn = images[0]['imageArn']  (succeeds today; the account segment is 'aws').
3. versions = mvm.list_managed_microvm_image_versions(imageIdentifier=arn)['items']; ver = versions[0]['imageVersion']  (succeeds today).
4. mvm.list_microvm_image_builds(imageIdentifier=arn, imageVersion=ver)
   Emulator: ClientError Code='NotImplemented', HTTP 501, "operation ListMicrovmImageBuilds is scaffolded but not implemented in this emulator".
   AWS: ClientError Code='AccessDeniedException', HTTP 403 (message names the calling principal's ARN).
5. mvm.get_microvm_image_build(imageIdentifier=arn, imageVersion=ver, buildId='probe-build-<unique>')
   Emulator: Code='NotImplemented', HTTP 501. AWS: Code='AccessDeniedException', HTTP 403.
6. account = client('sts').get_caller_identity()['Account']; bogus = f"arn:aws:lambda:{region}:{account}:microvm-image/missing-image-<unique>"
   mvm.get_microvm_image_build(imageIdentifier=bogus, imageVersion='1.0', buildId='build-does-not-exist')
   Emulator: Code='NotImplemented', HTTP 501. AWS: Code='AccessDeniedException', HTTP 403 — the denial precedes the existence check, so it must NOT be ResourceNotFoundException (404) and must not succeed.
7. Re-replay lambda-microvms.get-microvm-image-build-access-denied to confirm.
