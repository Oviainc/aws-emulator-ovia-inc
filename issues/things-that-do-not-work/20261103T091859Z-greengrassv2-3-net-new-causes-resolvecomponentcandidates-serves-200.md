Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: greengrassv2.resolve-component-candidates-match-and-missing greengrassv2.resolve-component-candidates-requires-device-auth-rejected greengrassv2.resolve-component-candidates-unknown-version-not-found-rejected greengrassv2.cancel-deployment-nonexistent-not-found greengrassv2.delete-component-in-use-by-active-deployment-lenient
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrassv2.resolve-component-candidates-match-and-missing|_client_error#6|AssertionError|raise AssertionError(
Issue-Key: greengrassv2.resolve-component-candidates-requires-device-auth-rejected|_client_error#10|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: greengrassv2.resolve-component-candidates-unknown-version-not-found-rejected|_error_code#8|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: greengrassv2.cancel-deployment-nonexistent-not-found|run#5|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: greengrassv2.delete-component-in-use-by-active-deployment-lenient|run#23|AssertionError|assert mutate_code == "NotFoundException", (
Issue-Commit: c1544a9590ded4504bea161b6ee6981cbcc184a7

# greengrassv2: 3 net-new causes — ResolveComponentCandidates serves 200 to IAM SigV4 callers instead of 403 AccessDeniedException, and CancelDeployment / TagResource not-found refusals are unreachable (501)

> _Note: this issue was identified against the code base at commit `c1544a9590ded4504bea161b6ee6981cbcc184a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ResolveComponentCandidates

## Description
Three independent root causes on the greengrassv2 surface, all found by replaying the behavioral suite against the booted emulator. 3 causes to close.

Related-but-not-covering open issues (named per the rules, none of them carries these failures' replay keys): 20261103T065241Z-greengrassv2-7-net-new-unimplemented-operations-answer-501.md (its Cause 6 described ResolveComponentCandidates as answering 501 NotImplemented — it no longer does; it now answers 200, which is the net-new defect in Cause 1 below), 20261103T065241Z-greengrassv2-canceldeployment-stub.md and 20261103T065241Z-greengrassv2-tagresource-stub.md (both are 'implement the stub' items; Causes 2 and 3 state the AWS-observable refusal classes those handlers must produce once implemented, which the replay still cannot reach).

Cause 1 of 3: ResolveComponentCandidates does not enforce the core-device data-plane credential boundary — it resolves and returns component versions to an ordinary IAM SigV4 caller.
AWS: Smithy: ResolveComponentCandidates POST /greengrass/v2/resolveComponentCandidates, success 200 with `resolvedComponentVersions`; modeled errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). Recorded behavior: this is a core-device (Greengrass nucleus) data-plane operation, so a request signed with ordinary IAM SigV4 control-plane credentials is refused HTTP 403, error code `AccessDeniedException`, message `Unable to determine service/operation name to be authorized`. That message is entirely fixed text — it has no variable part taken from the request or from state. The refusal is returned BEFORE any existence or platform resolution, so it is the same answer whether the named candidate is a real DEPLOYABLE component version, a component that was never created, or a version of a real component that was never published (e.g. 9.9.9).
Emulator: HTTP 200 with a populated `resolvedComponentVersions` list (arn / componentName / componentVersion / recipe / vendorGuidance=ACTIVE / message=NONE) for every one of those three candidate classes — no credential-class check at all, and no not-found path either.
Why: the data-plane auth boundary silently does not exist. A client or SDK test that expects 403 AccessDeniedException for IAM credentials sees a success instead; worse, recipe bytes of a component version are handed to a caller that real AWS never authorizes for this operation, so code written against the emulator passes locally and is denied in production.
Fix: the operation's handler and its credential-class check — services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/resolve_component_candidates.rs (`handle`): classify the caller's credentials and refuse non-core-device (plain IAM SigV4) callers with 403 `AccessDeniedException` / `Unable to determine service/operation name to be authorized` before touching component state.
Traces: greengrassv2.resolve-component-candidates-match-and-missing, greengrassv2.resolve-component-candidates-requires-device-auth-rejected, greengrassv2.resolve-component-candidates-unknown-version-not-found-rejected.

Cause 2 of 3: CancelDeployment never answers — its not-found refusal and its ACTIVE→CANCELED transition are both unreachable.
AWS: Smithy: CancelDeployment POST /greengrass/v2/deployments/{deploymentId}/cancel, success 200 with optional `message`; modeled errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). Recorded behavior: (a) a `deploymentId` that was never created is refused HTTP 404, error code `ResourceNotFoundException`, message `Deployment not found` — entirely fixed text, no variable part; (b) cancelling a deployment that is currently `ACTIVE` succeeds and the deployment's `deploymentStatus` observed through GetDeployment becomes `CANCELED`.
Emulator: HTTP 501, error code `NotImplemented`, message `operation CancelDeployment is scaffolded but not implemented in this emulator`, for both paths.
Why: a client can neither cancel a deployment nor distinguish a bogus deployment id from a real one; cleanup/teardown logic that keys on 404 `ResourceNotFoundException` for an unknown deployment id never terminates, and the CANCELED terminal state is unobservable (which in turn blocks the cancel-before-delete gate on DeleteDeployment).
Fix: the operation's handler, services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/cancel_deployment.rs (`handle`), plus the deployment store it must look the id up in and whose status it must transition.
Traces: greengrassv2.cancel-deployment-nonexistent-not-found.

Cause 3 of 3: TagResource never answers, so the tagging surface's not-found refusal class is absent — including its distinct error code.
AWS: Smithy: TagResource POST /tags/{resourceArn} with required `resourceArn` (GenericV2ARN, pattern `^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):(components|deployments|coreDevices):`) and required `tags` (TagMap, 1..200), success 200 with an empty body; modeled errors InternalServerException(500), ResourceNotFoundException(404), ValidationException(400). Recorded behavior: a well-formed greengrass component-version ARN that names a resource which does not exist (e.g. a component version that was just deleted) is refused HTTP 404 with error code `NotFoundException` — note this differs from the modeled `ResourceNotFoundException` and from the code GetComponent/DescribeComponent use for the same missing component — and message `Resource was not found`, entirely fixed text with no variable part. The same replay shows DescribeComponent on that deleted ARN correctly refusing 404 `ResourceNotFoundException` with `Component (<componentName>:<componentVersion>) does not exist in account <accountId>.`, so only the tagging path is missing.
Emulator: HTTP 501, error code `NotImplemented`, message `operation TagResource is scaffolded but not implemented in this emulator`.
Why: a mutation against a dead resource id is neither performed nor refused, so a client cannot tell a deleted resource from a live one through the tagging surface, and code that branches on the tagging surface's own `NotFoundException` code never takes that branch.
Fix: the operation's handler, services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/tag_resource.rs (`handle`), plus the tag store shared with UntagResource/ListTagsForResource; the existence check must emit the tagging surface's `NotFoundException` code rather than `ResourceNotFoundException`.
Traces: greengrassv2.delete-component-in-use-by-active-deployment-lenient.

## Evidence
Cause 1 — ResolveComponentCandidates (expected 403 `AccessDeniedException` / `Unable to determine service/operation name to be authorized`; actual HTTP 200 with resolved versions):
- greengrassv2.resolve-component-candidates-match-and-missing: `AssertionError: real AWS rejects resolve_component_candidates with plain IAM credentials, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'resolvedComponentVersions': [{'arn': 'arn:aws:greengrass:<region>:<account>:components:<name>:versions:1.0.0', 'componentName': '<name>', 'componentVersion': '1.0.0', 'recipe': b'{...}', 'vendorGuidance': 'ACTIVE', 'message': 'NONE'}]}` — for a candidate naming a real component version AND the same 200 for one naming a component that was never created.
- greengrassv2.resolve-component-candidates-requires-device-auth-rejected: `AssertionError: expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'resolvedComponentVersions': [...]}` — candidate is a component version verified DEPLOYABLE first.
- greengrassv2.resolve-component-candidates-unknown-version-not-found-rejected: `AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'resolvedComponentVersions': [...]}` — including for componentVersion 9.9.9, which was never published.
(ARNs, component names, account id, region and timestamps above are per-run values; the requirement is the 403/`AccessDeniedException`/fixed message, not those values.)

Cause 2 — CancelDeployment (expected 404 `ResourceNotFoundException` / `Deployment not found`, then a successful cancel moving the deployment to CANCELED; actual 501 `NotImplemented`):
- greengrassv2.cancel-deployment-nonexistent-not-found: `AssertionError: expected ResourceNotFoundException, got 'NotImplemented'` (the emulator's body is `operation CancelDeployment is scaffolded but not implemented in this emulator`). The check's positive control — cancel a real ACTIVE deployment and poll GetDeployment until `deploymentStatus == 'CANCELED'` — is never reached.

Cause 3 — TagResource (expected 404 `NotFoundException` / `Resource was not found`; actual 501 `NotImplemented`):
- greengrassv2.delete-component-in-use-by-active-deployment-lenient: `AssertionError: expected TagResource on the deleted component to be rejected as not-found, got code='NotImplemented'`. Everything before that assertion passed, i.e. DeleteComponent on a version referenced by an ACTIVE deployment succeeded and DescribeComponent on it then refused 404 `ResourceNotFoundException` with the expected message template `Component (<componentName>:<componentVersion>) does not exist in account <accountId>.` — so the divergence is isolated to the tagging path.

## Steps to Reproduce
Boot the emulator and point botocore clients for `greengrassv2` (ordinary IAM SigV4 credentials) at it.

Cause 1:
1. `create_component_version(inlineRecipe=<minimal 2020-01-25 recipe for NAME:1.0.0, linux/amd64 manifest>)` and wait for DescribeComponent to report componentState DEPLOYABLE.
2. `resolve_component_candidates(platform={'name':'linux','attributes':{'os':'linux','architecture':'amd64'}}, componentCandidates=[{'componentName': NAME, 'componentVersion': '1.0.0', 'versionRequirements': {}}])` — expect HTTP 403 `AccessDeniedException` / `Unable to determine service/operation name to be authorized`; observe HTTP 200 with resolvedComponentVersions.
3. Repeat step 2 with a componentName that was never created, and again with NAME and componentVersion '9.9.9' — both must give the identical 403; both answer 200.

Cause 2:
1. `cancel_deployment(deploymentId='<never-created-id>')` — expect HTTP 404 `ResourceNotFoundException` / `Deployment not found`; observe HTTP 501 `NotImplemented`.
2. `create_thing` an IoT thing, `create_deployment(targetArn=<thingArn>)`, then `cancel_deployment(deploymentId=<that id>)` — expect 200 and GetDeployment to report `deploymentStatus == 'CANCELED'`; observe 501 `NotImplemented`.

Cause 3:
1. `create_component_version(...)` for NAME:1.0.0, wait for DEPLOYABLE, then `delete_component(arn=<that arn>)`.
2. Confirm `describe_component(arn=<that arn>)` now refuses 404 `ResourceNotFoundException` (it does).
3. `tag_resource(resourceArn=<that same deleted component arn>, tags={'probe':'x'})` — expect HTTP 404 with error code `NotFoundException` and message `Resource was not found`; observe HTTP 501 `NotImplemented`.
