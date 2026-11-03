Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: greengrassv2.batch-associate-client-device-lenient-nonexistent-core-device greengrassv2.delete-deployment-nonexistent-not-found greengrassv2.delete-active-deployment-validation-rejected greengrassv2.delete-deployment-active-conflict-rejected greengrassv2.get-core-device-nonexistent-not-found greengrassv2.core-device-nucleus-native-protocol-unreachable greengrassv2.get-and-delete-core-device-nonexistent-not-found greengrassv2.get-component-version-artifact-undeclared-artifact-rejected greengrassv2.resolve-component-candidates-match-and-missing greengrassv2.resolve-component-candidates-requires-device-auth-rejected greengrassv2.resolve-component-candidates-unknown-version-not-found-rejected greengrassv2.untag-resource-nonexistent-arn-rejected
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrassv2.batch-associate-client-device-lenient-nonexistent-core-device|run#6|AssertionError|assert code is None, f"expected a successful (non-raising) response, got code={code!r}"
Issue-Key: greengrassv2.delete-deployment-nonexistent-not-found|run#7|AssertionError|assert code == "ResourceNotFoundException", code
Issue-Key: greengrassv2.delete-active-deployment-validation-rejected|run#14|AssertionError|assert code == "ValidationException", f"expected ValidationException deleting an ACTIVE deployment, got {code!r}"
Issue-Key: greengrassv2.delete-deployment-active-conflict-rejected|run#11|AssertionError|assert code == "ValidationException", f"got code={code!r}"
Issue-Key: greengrassv2.get-core-device-nonexistent-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: greengrassv2.core-device-nucleus-native-protocol-unreachable|run#9|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: greengrassv2.get-and-delete-core-device-nonexistent-not-found|run#5|AssertionError|assert delete_code == "ResourceNotFoundException", (
Issue-Key: greengrassv2.get-component-version-artifact-undeclared-artifact-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: greengrassv2.resolve-component-candidates-match-and-missing|run#7|AssertionError|assert code_real == "AccessDeniedException", f"expected AccessDeniedException, got {code_real!r}"
Issue-Key: greengrassv2.resolve-component-candidates-requires-device-auth-rejected|run#11|AssertionError|assert code == "AccessDeniedException", f"expected data-plane auth boundary rejection, got code={code!r}"
Issue-Key: greengrassv2.resolve-component-candidates-unknown-version-not-found-rejected|run#9|AssertionError|assert code_real == "AccessDeniedException", f"expected AccessDeniedException, got {code_real!r}"
Issue-Key: greengrassv2.untag-resource-nonexistent-arn-rejected|run#5|AssertionError|assert code == "NotFoundException", (
Issue-Commit: 3ecb0d6b353755c874093f3eab62bbd62a85b9a2

# greengrassv2: 7 net-new unimplemented operations answer 501 NotImplemented, so core-device, deployment-delete, artifact, resolve and untag paths never refuse or act as AWS does

> _Note: this issue was identified against the code base at commit `3ecb0d6b353755c874093f3eab62bbd62a85b9a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDeployment

## Description
Seven greengrassv2 operations are still scaffold stubs (`//! SCAFFOLDED, NOT IMPLEMENTED` in their handler file) and answer HTTP 501 with error code `NotImplemented` and message `operation <Name> is scaffolded but not implemented in this emulator`. Each is its own handler file and its own root cause; none of them is the operation named by the stub issues the replay report already filed for this service (GetServiceRoleForAccount, AssociateServiceRoleToAccount, BatchDisassociateClientDeviceFromCoreDevice, CancelDeployment, TagResource, ListTagsForResource, ListComponentVersions, ListDeployments, ListEffectiveDeployments, ListClientDevicesAssociatedWithCoreDevice, UpdateConnectivityInfo). 7 causes to close.

Cause 1 of 7: BatchAssociateClientDeviceWithCoreDevice is unimplemented.
AWS: Smithy: BatchAssociateClientDeviceWithCoreDevice POST /greengrass/v2/coreDevices/{coreDeviceThingName}/associateClientDevices, success 200, output member `errorEntries` (AssociateClientDeviceWithCoreDeviceErrorList, len 0..100); modeled errors AccessDeniedException(403)/InternalServerException(500)/ResourceNotFoundException(404)/ThrottlingException(429)/ValidationException(400). Recorded behavior: the call is LENIENT — a `coreDeviceThingName` that was never registered as a core device is still accepted, answering 200 with `errorEntries: []`, and the association is afterwards visible from ListClientDevicesAssociatedWithCoreDevice.
Emulator: 501, code `NotImplemented`.
Why: a client cannot associate client devices with a core device at all, and the documented no-existence-check-at-request-time leniency can never be observed.
Fix: the operation's handler, services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/batch_associate_client_device_with_core_device.rs (`handle`), plus the association list in the service state.
Traces: greengrassv2.batch-associate-client-device-lenient-nonexistent-core-device.

Cause 2 of 7: DeleteDeployment is unimplemented.
AWS: Smithy: DeleteDeployment DELETE /greengrass/v2/deployments/{deploymentId}, success 204 (no body); modeled errors AccessDeniedException(403)/ConflictException(409)/InternalServerException(500)/ResourceNotFoundException(404)/ThrottlingException(429)/ValidationException(400). Recorded behavior: (a) a `deploymentId` that was never created is refused 404 `ResourceNotFoundException` with message `Deployment not found` — entirely fixed text, no variable part; (b) a deployment still in `ACTIVE` status is refused 400 `ValidationException` with message `Invalid Input: Greengrass deployment must be canceled before deletion` — entirely fixed text — and the deployment keeps status ACTIVE; (c) after CancelDeployment has taken effect the delete succeeds 204 and a later GetDeployment/CancelDeployment on that id is refused 404 `ResourceNotFoundException` / `Deployment not found`.
Emulator: 501, code `NotImplemented`, for every one of those paths.
Why: deployments can never be deleted, the cancel-before-delete gate never fires, and clean-up code that relies on a 404 for an already-deleted deployment loops or aborts.
Fix: the operation's handler, services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/delete_deployment.rs (`handle`), and the deployment store it must read/remove.
Traces: greengrassv2.delete-deployment-nonexistent-not-found, greengrassv2.delete-active-deployment-validation-rejected, greengrassv2.delete-deployment-active-conflict-rejected.

Cause 3 of 7: GetCoreDevice is unimplemented.
AWS: Smithy: GetCoreDevice GET /greengrass/v2/coreDevices/{coreDeviceThingName}, success 200 with coreDeviceThingName/coreVersion/platform/architecture/runtime/status/lastStatusUpdateTimestamp/tags; modeled errors include ResourceNotFoundException(404). Recorded behavior: for a thing name that no Greengrass nucleus ever checked in under (including a freshly created IoT thing), the call is refused 404 `ResourceNotFoundException` with message `<coreDeviceThingName> can not be found` — the variable part is the requested core-device thing name taken from the request, the fixed text is ` can not be found`.
Emulator: 501, code `NotImplemented`.
Why: a client can never read core-device metadata, and the "an IoT thing is not a core device until it checks in" boundary is unobservable.
Fix: the operation's handler, services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/get_core_device.rs (`handle`), reading the core-device registry in the service state.
Traces: greengrassv2.get-core-device-nonexistent-not-found, greengrassv2.core-device-nucleus-native-protocol-unreachable.

Cause 4 of 7: DeleteCoreDevice is unimplemented.
AWS: Smithy: DeleteCoreDevice DELETE /greengrass/v2/coreDevices/{coreDeviceThingName}, success 204; modeled errors include ResourceNotFoundException(404). Recorded behavior: for a name never registered as a core device the call is refused 404 `ResourceNotFoundException` with the same message shape as GetCoreDevice — `<coreDeviceThingName> can not be found`, the name taken from the request.
Emulator: 501, code `NotImplemented`.
Why: core devices can never be deregistered, and a delete of a nonexistent core device neither 404s nor no-ops.
Fix: the operation's handler, services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/delete_core_device.rs (`handle`).
Traces: greengrassv2.get-and-delete-core-device-nonexistent-not-found.

Cause 5 of 7: GetComponentVersionArtifact is unimplemented.
AWS: Smithy: GetComponentVersionArtifact GET /greengrass/v2/components/{arn}/artifacts/{artifactName+}, success 200 with required `preSignedUrl`; modeled errors AccessDeniedException(403)/InternalServerException(500)/ResourceNotFoundException(404)/ThrottlingException(429)/ValidationException(400). Recorded behavior: asking for an artifact name the component version's recipe never declares is refused 400 `ValidationException` with message `Component (<componentName>:<componentVersion>) or Artifact (<artifactName>) for account <accountId> does not exist` — fixed text is `Component (`, `) or Artifact (`, `) for account `, ` does not exist`; the component name/version come from the ARN in the request, the artifact name from the request, the account id from the caller's account.
Emulator: 501, code `NotImplemented`.
Why: artifact download URLs can never be obtained, and the undeclared-artifact refusal class is missing entirely.
Fix: the operation's handler, services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/get_component_version_artifact.rs (`handle`), plus the recipe-artifact lookup in the component state.
Traces: greengrassv2.get-component-version-artifact-undeclared-artifact-rejected.

Cause 6 of 7: ResolveComponentCandidates is unimplemented.
AWS: Smithy: ResolveComponentCandidates POST /greengrass/v2/resolveComponentCandidates, success 200 with `resolvedComponentVersions`; modeled errors AccessDeniedException(403)/ConflictException(409)/InternalServerException(500)/ResourceNotFoundException(404)/ThrottlingException(429)/ValidationException(400). Recorded behavior: this is a core-device data-plane operation, so a caller signing with ordinary IAM SigV4 credentials is refused 403 `AccessDeniedException` with message `Unable to determine service/operation name to be authorized` — entirely fixed text, no variable part — and this is the answer regardless of whether the named candidate component version exists, is DEPLOYABLE, or was never created.
Emulator: 501, code `NotImplemented`.
Why: the data-plane auth boundary is not enforced; a client (or an SDK test) that expects the 403 for IAM credentials instead sees a 501, and nothing distinguishes the control-plane credential class from core-device credentials.
Fix: the operation's handler, services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/resolve_component_candidates.rs (`handle`), and its credential-class check.
Traces: greengrassv2.resolve-component-candidates-match-and-missing, greengrassv2.resolve-component-candidates-requires-device-auth-rejected, greengrassv2.resolve-component-candidates-unknown-version-not-found-rejected.

Cause 7 of 7: UntagResource is unimplemented.
AWS: Smithy: UntagResource DELETE /tags/{resourceArn} with required `tagKeys` query list, success 200; modeled errors InternalServerException(500)/ResourceNotFoundException(404)/ValidationException(400). Recorded behavior: a well-formed greengrass component-version ARN that names no existing resource is refused 404 with error code `NotFoundException` and message `Resource was not found` — entirely fixed text (note the code is `NotFoundException`, not the modeled `ResourceNotFoundException`); for a real tagged resource the call succeeds and the key disappears from ListTagsForResource.
Emulator: 501, code `NotImplemented`.
Why: tags can never be removed, and the not-found refusal class for the tagging surface is absent.
Fix: the operation's handler, services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/untag_resource.rs (`handle`), plus the tag store shared with TagResource/ListTagsForResource.
Traces: greengrassv2.untag-resource-nonexistent-arn-rejected.

## Evidence
All seven surface as the same emulator response shape, differing only in the operation name:

- Expected (AWS, per cause above): 200/204 success, or the modeled refusal — e.g. 404 `ResourceNotFoundException` / `Deployment not found`; 400 `ValidationException` / `Invalid Input: Greengrass deployment must be canceled before deletion`; 404 `ResourceNotFoundException` / `<name> can not be found`; 400 `ValidationException` / `Component (<name>:<version>) or Artifact (<artifact>) for account <account> does not exist`; 403 `AccessDeniedException` / `Unable to determine service/operation name to be authorized`; 404 `NotFoundException` / `Resource was not found`.
- Actual (emulator): HTTP 501, error code `NotImplemented`, message `operation <Name> is scaffolded but not implemented in this emulator`.

Replay excerpts:
- batch-associate-client-device-lenient-nonexistent-core-device: `AssertionError: expected a successful (non-raising) response, got code='NotImplemented'`
- delete-deployment-nonexistent-not-found: `AssertionError: NotImplemented` (assert code == "ResourceNotFoundException")
- delete-active-deployment-validation-rejected: `AssertionError: expected ValidationException deleting an ACTIVE deployment, got 'NotImplemented'`
- delete-deployment-active-conflict-rejected: `AssertionError: got code='NotImplemented'`
- get-core-device-nonexistent-not-found: `AssertionError: expected ResourceNotFoundException, got 'NotImplemented'`
- core-device-nucleus-native-protocol-unreachable: `AssertionError: expected ResourceNotFoundException for a thing with no Nucleus check-in, got 'NotImplemented'`
- get-and-delete-core-device-nonexistent-not-found: `AssertionError: expected ResourceNotFoundException from DeleteCoreDevice on an unregistered thing name, got 'NotImplemented'`
- get-component-version-artifact-undeclared-artifact-rejected: `AssertionError: expected ValidationException, got 'NotImplemented'`
- resolve-component-candidates-match-and-missing / -unknown-version-not-found-rejected: `AssertionError: expected AccessDeniedException, got 'NotImplemented'`
- resolve-component-candidates-requires-device-auth-rejected: `AssertionError: expected data-plane auth boundary rejection, got code='NotImplemented'`
- untag-resource-nonexistent-arn-rejected: `AssertionError: expected NotFoundException for a nonexistent resourceArn, got code='NotImplemented'`

The handler files confirm the cause: each begins with `//! SCAFFOLDED, NOT IMPLEMENTED` and ends in `Err(crate::scaffolded::Scaffolded::not_implemented("<Name>"))` (verified in delete_deployment.rs:189, get_core_device.rs:191, delete_core_device.rs, batch_associate_client_device_with_core_device.rs, resolve_component_candidates.rs, untag_resource.rs).

## Steps to Reproduce
1. Boot the emulator and point a greengrassv2 client at it.
2. Cause 1: `batch_associate_client_device_with_core_device(coreDeviceThingName="<never-registered>", entries=[{"thingName": "<client>"}])` — expect 200 with `errorEntries: []`, observe 501 NotImplemented.
3. Cause 2: `delete_deployment(deploymentId="<never-created>")` — expect 404 ResourceNotFoundException `Deployment not found`; then `create_deployment(targetArn=<real thing arn>)` and `delete_deployment` on it while ACTIVE — expect 400 ValidationException `Invalid Input: Greengrass deployment must be canceled before deletion`; both answer 501.
4. Cause 3/4: `get_core_device(coreDeviceThingName="<unregistered>")` and `delete_core_device(coreDeviceThingName="<unregistered>")` — expect 404 `<name> can not be found`, observe 501.
5. Cause 5: create a component version from an inline recipe declaring no artifacts, then `get_component_version_artifact(arn=<that arn>, artifactName="does-not-exist.zip")` — expect 400 ValidationException with the Component/Artifact sentence, observe 501.
6. Cause 6: `resolve_component_candidates(platform=..., componentCandidates=[...])` with ordinary IAM SigV4 credentials — expect 403 AccessDeniedException `Unable to determine service/operation name to be authorized`, observe 501.
7. Cause 7: `untag_resource(resourceArn="arn:aws:greengrass:<region>:<account>:components:<missing>:versions:1.0.0", tagKeys=["env"])` — expect 404 NotFoundException `Resource was not found`, observe 501.
