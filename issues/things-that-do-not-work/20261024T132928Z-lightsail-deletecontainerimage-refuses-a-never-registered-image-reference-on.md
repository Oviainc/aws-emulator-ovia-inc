Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lightsail.container-image-registry-lifecycle
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|DeleteContainerImage|NotFoundException
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# DeleteContainerImage refuses a never-registered image reference on an existing container service instead of succeeding as a no-op

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteContainerImage

## Description
Cause 1 of 1: DeleteContainerImage applies image-level existence enforcement, so deleting an image reference that the container service's registry does not hold is refused instead of being accepted as an idempotent no-op.

AWS: Smithy model (services/service-com-amazonaws-lightsail-lightsail-20161128/smithy-model/com-amazonaws-lightsail-lightsail-20161128.json) binds DeleteContainerImage to DELETE /ls/api/2016-11-28/container-services/{serviceName}/images/{image} with success code 204, output shape DeleteContainerImageResult (empty), and errors AccessDeniedException(403), InvalidInputException(400), NotFoundException(404), RegionSetupInProgressException(428), ServiceException(500), UnauthenticatedException(401). The model does NOT state which of those applies to an unheld image name, so for the per-image behavior this is recorded behavior only: the recorded run against real AWS shows DeleteContainerImage validating only the PARENT resource — a nonexistent container service is refused with InvalidInputException and the message 'Service "<name>" does not exist.', while a reference that was never registered on an EXISTING service returns a bare success (no error body) and leaves GetContainerImages reporting an empty list. The check's own docstring records that an earlier revision of it assumed image-level enforcement symmetric with RegisterContainerImage, and that real AWS contradicted that.

Emulator: the handler looks the image name up in the service's stored registry and, when it is absent, returns the crate's does-not-exist envelope — wire error code NotFoundException, message 'Image ":sim2real-ls-cs-<id>.roundtrip.1" not found.' (observed by botocore as NotFoundException). The parent-resource cases are already correct: the missing-service refusal and the RegisterContainerImage-unpushed-digest refusal both matched.

Why: a client cannot make container-image cleanup idempotent. Teardown paths and retries that call DeleteContainerImage for a reference that is already gone (or was never registered, e.g. after a failed push/register) get a hard client error from the emulator where AWS returns success, so idempotent cleanup code and "delete all images then delete the service" loops abort against the emulator but pass against AWS. It also makes the operation's error vocabulary wrong: a refusal is produced where no refusal is modeled for that condition.

Fix: the DeleteContainerImage handler — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/delete_container_image.rs. The service-existence lookup should stay as the only refusal path; the image lookup must not be fatal — remove the stored entry when present and return the empty DeleteContainerImageOutput either way. The helper that builds the image-level not-found becomes unused. No change to state or serialization is needed. Related but distinct, still open: 20261024T095637Z-lightsail-registercontainerimage-wrong-behavior.md (RegisterContainerImage refuses every request, so the registry can never be populated) — that issue concerns registration, this one concerns deletion of an absent reference and is not covered by it.

Traces: lightsail.container-image-registry-lifecycle (the check reaches the final step, so the RegisterContainerImage-rejection and missing-service-InvalidInputException assertions before it already pass).

## Evidence
Check lightsail.container-image-registry-lifecycle, final step.

Request (after create_container_service(serviceName=<svc>, power=nano, scale=1) and get_container_images(<svc>) == []):
  lightsail.delete_container_image(serviceName="<svc>", image=":<svc>.roundtrip.1")

Expected (real AWS, recorded): success with an empty result body (modeled success code 204, DeleteContainerImageResult has no members); a following get_container_images(<svc>) still returns containerImages == [].

Actual (emulator): ClientError raised —
  botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the DeleteContainerImage operation: Image ":sim2real-ls-cs-d125ebe16795.roundtrip.1" not found.
The emulator's refusal comes from the image-name lookup in src/ops/delete_container_image.rs (position(|image| image.image == input.image).ok_or_else(|| image_does_not_exist(...))), which returns the crate's does_not_exist envelope. The generated name in the recorded message is an example only; the requirement is the absence of any error for this condition.

Assertions that PASSED in the same check and must keep passing: register_container_image with an unpushed digest -> code NotFoundException, message 'Image with digest "sha256:aaaa..." not found.'; delete_container_image against a nonexistent service -> code InvalidInputException, message 'Service "<missing>" does not exist.'

## Steps to Reproduce
1. Boot the emulator and point a lightsail client at it.
2. ls.create_container_service(serviceName="ls-cs-demo", power="nano", scale=1).
3. ls.get_container_images(serviceName="ls-cs-demo") -> containerImages == [] (passes today).
4. ls.delete_container_image(serviceName="ls-cs-demo", image=":ls-cs-demo.roundtrip.1") — an image reference that was never registered on this existing service.
5. Observe NotFoundException ('Image "..." not found.'). Real AWS returns a bare success here; only a nonexistent serviceName should be refused (with InvalidInputException).
6. ls.get_container_images(serviceName="ls-cs-demo") must still return [].
