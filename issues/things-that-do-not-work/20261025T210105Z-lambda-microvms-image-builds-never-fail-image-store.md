Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.image-build-failure-surfaces-state-reason-on-every-build lambda-microvms.activating-a-failed-image-version-still-rejects-run lambda-microvms.image-created-in-one-region-is-not-visible-in-another lambda-microvms.representation-managed-microvm-image-fields-round-trip
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms.image-build-failure-surfaces-state-reason-on-every-build|run#10|AssertionError|assert image["state"] == "CREATE_FAILED", (
Issue-Key: lambda-microvms.activating-a-failed-image-version-still-rejects-run|run#9|AssertionError|assert failed["state"] == "CREATE_FAILED", (
Issue-Key: lambda-microvms.image-created-in-one-region-is-not-visible-in-another|_error#7|AssertionError|raise AssertionError("the call unexpectedly SUCCEEDED; real AWS rejects it")
Issue-Key: lambda-microvms|GetMicrovmImage|ResourceNotFoundException
Issue-Commit: 5b5a943942747caef372cc4ffaa7be31fd45b661

# lambda-microvms: image builds never fail, image store is not region-scoped, and managed base images are 404 instead of 403

> _Note: this issue was identified against the code base at commit `5b5a943942747caef372cc4ffaa7be31fd45b661`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrovmImage

## Description
This issue bundles 3 distinct root causes in the lambda-microvms emulator, all grounded in failing behavioral checks. A fixer must close all three (Cause 1 of 3, Cause 2 of 3, Cause 3 of 3) before submitting.

=== Cause 1 of 3: the asynchronous image build always succeeds; CREATE_FAILED / FAILED is unreachable ===
Failing checks: lambda-microvms.image-build-failure-surfaces-state-reason-on-every-build, lambda-microvms.activating-a-failed-image-version-still-rejects-run

AWS: CreateMicrovmImage is documented in this service's Smithy model as "Creates a MicroVM image from the specified code artifact and base image. The build is asynchronous - the image transitions from CREATING to CREATED on success, or CREATE_FAILED on failure. Use GetMicrovmImage to poll for completion." MicrovmImageState (GetMicrovmImageOutput$state, @required) enumerates CREATE_FAILED, and MicrovmImageVersionState enumerates FAILED; GetMicrovmImageVersionOutput$stateReason is modeled as "The reason for the current state. For example, one or more builds failed.", and GetMicrovmImageOutput carries latestFailedImageVersion "The latest failed version of the MicroVM image, if any" alongside the optional latestActiveImageVersion. The recorded behavior (us-west-2, 2026-09-11) for an image whose codeArtifact.uri points into a bucket that does not exist is: the build runs, then the image settles state=CREATE_FAILED with latestFailedImageVersion set to the version and latestActiveImageVersion ABSENT; the version settles state=FAILED, status=INACTIVE, stateReason="The S3 bucket does not exist."; EVERY per-chipset-generation build record settles buildState=FAILED with the SAME stateReason, and a failed build carries no snapshotBuild member. The stateReason text "The S3 bucket does not exist." is entirely FIXED text for this failure class - it has no variable parts. The downstream refusal is also recorded: RunMicrovm against a version that was force-activated but has no successful build answers ResourceNotFoundException (HTTP 404) with the message "No image build found for MicroVM image <imageArn> and version <imageVersion>", where "No image build found for MicroVM image " and " and version " are fixed text and <imageArn>/<imageVersion> are taken from the request and the emulator's own state - never from any one recording.

Emulator: the failure path does not exist. CreateMicrovmImage validates only the SHAPE of codeArtifact.uri (crate::state::s3_object_uri) and then unconditionally schedules a SUCCESSFUL build (build_due_at = now + IMAGE_BUILD_SPAN), and materialize_due applies only the CREATING -> CREATED transition. Both checks fail at the first assertion after polling: "AssertionError: image settled in 'CREATED'; this artifact cannot be fetched so the build must fail" and "AssertionError: image settled in 'CREATED'; this check needs a FAILED version". Because CREATE_FAILED is unreachable, latestFailedImageVersion is never populated, version state FAILED / status INACTIVE / stateReason are never produced, no build record is ever FAILED, and the RunMicrovm "No image build found ..." refusal is unreachable as well.

Why: a client that relies on polling GetMicrovmImage to distinguish a successful build from a broken one is told every build succeeded. It will then hand an unbuildable image to RunMicrovm, get an unmodeled success instead of the ResourceNotFoundException AWS raises, and never see the diagnostic (stateReason) that is the only machine-readable explanation of why its artifact was rejected. Any retry/alerting logic keyed on CREATE_FAILED, latestFailedImageVersion, or buildState=FAILED is dead code against the emulator.

Fix: the build outcome belongs in the image build state machine - the deferred-transition logic in the service state (materialize_due and the build_due_at bookkeeping it drives) plus the place CreateMicrovmImage/UpdateMicrovmImage schedule it. The build must resolve the codeArtifact S3 location against the emulator's own object store and, when the bucket is absent, settle image=CREATE_FAILED + latestFailedImageVersion (leaving latestActiveImageVersion unset), version=FAILED/INACTIVE with the fixed stateReason, and every per-generation build record FAILED with the same stateReason and no snapshotBuild. RunMicrovm's handler must then refuse an ACTIVE-but-unbuilt version with the modeled ResourceNotFoundException and the message template above.

Traces: lambda-microvms.image-build-failure-surfaces-state-reason-on-every-build (asserts image CREATE_FAILED, version FAILED/INACTIVE/stateReason, every build FAILED with the same reason, no snapshotBuild); lambda-microvms.activating-a-failed-image-version-still-rejects-run (needs a FAILED version, then asserts UpdateMicrovmImageVersion accepts ACTIVE while state stays FAILED, image becomes UPDATED, and RunMicrovm answers ResourceNotFoundException). Citations: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/create_microvm_image.rs lines 255-261 (URI shape-only validation) and 388-391 (build_due_at always scheduled as success); services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs lines 9-24 and 39-44 (the clock-owed transitions, which model only the success move).

=== Cause 2 of 3: the MicroVM image store is global, not per-region ===
Failing check: lambda-microvms.image-created-in-one-region-is-not-visible-in-another

AWS: a MicroVM image is a regional typed resource. An identifier minted by CreateMicrovmImage in one region is answered by a client pinned to that region and REFUSED by a client pinned to any other region, with exactly the same code and shape as an identifier nobody ever created there: GetMicrovmImage models ResourceNotFoundException (httpError 404, "The specified resource does not exist.") and the recorded message is "MicroVMImage not found for MicroVMImageID: <imageArn>" - "MicroVMImage not found for MicroVMImageID: " is fixed text and <imageArn> is the identifier the request carried. The image also does not appear in ListMicrovmImages in the foreign region. (Recorded behavior; the model states the error but not the regional scoping.)

Emulator: there is no region dimension in the store at all. CreateMicrovmImage reads the request's region only to FORMAT the ARN (crate::state::microvm_image_arn(&region, &input.name)) and then inserts into store.images keyed by name alone, with store.arn_index mapping the ARN to that name; GetMicrovmImage resolves the identifier out of the same single map regardless of which region the request was signed for. The check's foreign-region GetMicrovmImage therefore SUCCEEDS and returns the image: "AssertionError: the call unexpectedly SUCCEEDED; real AWS rejects it". The same global map means ListMicrovmImages in the foreign region would list it too.

Why: a client that uses region pinning as an isolation boundary - a multi-region deployment, a disaster-recovery copy step, or a test asserting that a resource was NOT replicated - gets the opposite answer. Code that treats a 404 from a second region as "not yet replicated, create it here" will instead read a phantom image whose ARN names a different region, and cross-region leakage between concurrently running test suites is invisible.

Fix: the service state store must be partitioned by region (images, arn_index and every per-region listing index keyed by the request region, which crate::state::request_region already derives), and every read/list/mutate handler must scope its lookup to the requesting region, answering ResourceNotFoundException with the message template above for an identifier that exists only in another region.

Traces: lambda-microvms.image-created-in-one-region-is-not-visible-in-another (creates in ctx.region, confirms the home-region read, then expects ResourceNotFoundException from a client pinned to a second region and expects the ARN absent from that region's ListMicrovmImages). Citations: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/create_microvm_image.rs lines 318 and 396-411 (region used only for the ARN text; insert keyed by name); services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image.rs lines 180-193 (identifier resolved out of the single global map, no region check).

=== Cause 3 of 3: GetMicrovmImage on an AWS-managed base image ARN answers 404 ResourceNotFoundException instead of 403 AccessDeniedException ===
Failing check: lambda-microvms.representation-managed-microvm-image-fields-round-trip

AWS: the ARNs ListManagedMicrovmImages returns are AWS-owned (the account segment is literally `aws`: arn:<partition>:lambda:<region>:aws:microvm-image:<name>). Those images exist, but the calling principal has no permission to read them through the customer-facing GetMicrovmImage, so the call is DENIED rather than reported missing: AccessDeniedException, which this service models as a service-common error (errorKind client, httpError 403, "You do not have sufficient access to perform this action."). Recorded behavior: GetMicrovmImage on an ARN just returned by ListManagedMicrovmImages answers AccessDeniedException. The message names the calling principal's own ARN, so its variable part is caller identity and only the error CODE and the 403 status are required here; the check deliberately asserts the code only.

Emulator: GetMicrovmImage runs the managed ARN through crate::state::resolve_image_reference, which does not match any image in the account's own store, falls into ImageReference::Absent, and returns ResourceNotFoundException (404) built by crate::state::image_not_found_message. Replay: "AssertionError: ResourceNotFoundException" where AccessDeniedException was expected. Note the rest of this check passes - the managed items' imageArn pattern and createdAt/updatedAt timestamp types round-trip correctly - so the divergence is confined to the authorization verdict for an aws-owned identifier.

Why: 403 and 404 mean different things to a client. A caller that receives 404 for an AWS-managed base image concludes the base image it was just told to build from does not exist and aborts or retries forever, instead of recognizing a permission boundary it is never meant to cross. SDK retry and error-classification layers also treat the two differently.

Fix: GetMicrovmImage's handler (and any other customer-facing read that accepts a MicrovmImageIdentifier) must classify an identifier in the AWS-owned managed namespace BEFORE the own-account lookup - the emulator already has that predicate in crate::state (managed_microvm_image_id / is_published_managed_base_image, used by CreateMicrovmImage) - and return the modeled AccessDeniedException (403) for it rather than falling through to the not-found path.

Traces: lambda-microvms.representation-managed-microvm-image-fields-round-trip (asserts GetMicrovmImage on an ARN from ListManagedMicrovmImages is denied with AccessDeniedException). Citations: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image.rs lines 180-208 (Absent -> ResourceNotFoundException, no managed-namespace branch); services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/create_microvm_image.rs lines 270-278 (the managed-namespace predicate that already exists).

No existing open issue covers any of these three causes: list_existing_issues(query='microvm') returned none, and the eight other failing clusters in this suite are marked ALREADY FILED against unimplemented scaffold stubs (GetMicrovmImageVersion, TagResource, ListMicrovmImages, ListManagedMicrovmImageVersions, ListMicrovmImageBuilds, ListTags, UpdateMicrovmImage, and EC2's DescribeVpcEndpointServices) and are deliberately not duplicated here.

## Evidence
Behavioral suite: 0/36 passed. 4 failing clusters were not already filed; they cluster into the 3 causes above.

Cause 1 of 3 - build never fails:
  Expected (AWS): GetMicrovmImage -> state=CREATE_FAILED, latestFailedImageVersion=<version>, latestActiveImageVersion absent; GetMicrovmImageVersion -> state=FAILED, status=INACTIVE, stateReason="The S3 bucket does not exist."; every build record buildState=FAILED with the same stateReason and no snapshotBuild; RunMicrovm -> ResourceNotFoundException (404) "No image build found for MicroVM image <imageArn> and version <imageVersion>".
  Actual (emulator): the image settles state='CREATED' for a codeArtifact.uri whose bucket does not exist.
    check_lambda_microvms_image_build_failure_surfaces_state_reason_on_every_build.py:50
    AssertionError: image settled in 'CREATED'; this artifact cannot be fetched so the build must fail
    check_lambda_microvms_activating_a_failed_image_version_still_rejects_run.py:44
    AssertionError: image settled in 'CREATED'; this check needs a FAILED version

Cause 2 of 3 - no region isolation:
  Expected (AWS): GetMicrovmImage from a second region for an image created in the home region -> ResourceNotFoundException (404), message "MicroVMImage not found for MicroVMImageID: <imageArn>"; the ARN absent from that region's ListMicrovmImages.
  Actual (emulator): the foreign-region GetMicrovmImage returns the image (2xx).
    check_lambda_microvms_image_created_in_one_region_is_not_visible_in_another.py:42 -> _error():130
    AssertionError: the call unexpectedly SUCCEEDED; real AWS rejects it
  (Home-region positive control passed, so the divergence is the missing regional boundary, not a broken create.)

Cause 3 of 3 - wrong refusal for an AWS-managed base image:
  Expected (AWS): GetMicrovmImage(imageIdentifier=<arn:...:lambda:<region>:aws:microvm-image:...> from ListManagedMicrovmImages) -> AccessDeniedException, HTTP 403.
  Actual (emulator): ResourceNotFoundException, HTTP 404.
    check_lambda_microvms_representation_managed_microvm_image_fields_round_trip.py:62
    AssertionError: ResourceNotFoundException
  (The same check's ARN-pattern and timestamp-type assertions on ListManagedMicrovmImages items passed.)

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json):
  CreateMicrovmImage doc: "The build is asynchronous - the image transitions from CREATING to CREATED on success, or CREATE_FAILED on failure."
  MicrovmImageState: CREATING | CREATED | CREATE_FAILED | UPDATING | UPDATED | UPDATE_FAILED | DELETING | DELETE_FAILED | DELETED
  GetMicrovmImageOutput: latestFailedImageVersion (optional), latestActiveImageVersion (optional)
  GetMicrovmImageVersionOutput: state @required, status @required ("ACTIVE (can be used by RunMicrovm) or INACTIVE"), stateReason ("The reason for the current state. For example, one or more builds failed.")
  ResourceNotFoundException: errorKind client, httpError 404, message @required
  AccessDeniedException: errorKind client, httpError 403 (service common_errors)
  RunMicrovm errors: ConflictException, InsufficientCapacityException, ResourceNotFoundException, ServiceQuotaExceededException

## Steps to Reproduce
Cause 1 of 3 (build never fails):
1. mvm = client('lambda-microvms'); base = first ARN from ListManagedMicrovmImages.
2. CreateMicrovmImage(name=<unique>, baseImageArn=base, buildRoleArn=<an IAM role that trusts lambda.amazonaws.com>, codeArtifact={'uri': 's3://<unique>-absent/code.zip'}) -- a bucket that does not exist. Observe state='CREATING' (correct).
3. Poll GetMicrovmImage until state leaves CREATING.
   Emulator: state='CREATED'. AWS: state='CREATE_FAILED', latestFailedImageVersion=<version>, no latestActiveImageVersion.
4. GetMicrovmImageVersion(imageIdentifier, imageVersion) -- AWS: state='FAILED', status='INACTIVE', stateReason='The S3 bucket does not exist.'
5. ListMicrovmImageBuilds / GetMicrovmImageBuild for that version -- AWS: every build buildState='FAILED' with the same stateReason and no snapshotBuild member.
6. UpdateMicrovmImageVersion(status='ACTIVE') on the FAILED version -- AWS accepts it (returns status=ACTIVE, state stays FAILED) and the image becomes UPDATED with latestActiveImageVersion=<version>; then RunMicrovm(imageIdentifier) -- AWS: ResourceNotFoundException 'No image build found for MicroVM image <imageArn> and version <imageVersion>', and ListMicrovms scoped to the image stays empty.

Cause 2 of 3 (region isolation):
1. home = client('lambda-microvms', region=R1); foreign = client('lambda-microvms', region=R2), R2 != R1.
2. image_arn = home.CreateMicrovmImage(...)['imageArn'].
3. home.GetMicrovmImage(imageIdentifier=image_arn) -- succeeds (correct).
4. foreign.GetMicrovmImage(imageIdentifier=image_arn).
   Emulator: 2xx, returns the image. AWS: 404 ResourceNotFoundException, 'MicroVMImage not found for MicroVMImageID: <image_arn>'.
5. foreign.ListMicrovmImages() -- AWS: image_arn absent.

Cause 3 of 3 (managed base image authorization):
1. items = client('lambda-microvms').ListManagedMicrovmImages()['items']; take arn = items[0]['imageArn'] (account segment is 'aws').
2. client.GetMicrovmImage(imageIdentifier=arn).
   Emulator: ResourceNotFoundException, HTTP 404. AWS: AccessDeniedException, HTTP 403.
