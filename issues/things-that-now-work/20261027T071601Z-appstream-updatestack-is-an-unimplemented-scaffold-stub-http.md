Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream__ec2.asvpce-update-stack-attributes-to-delete-removes-access-endpoints
Issue-Key: appstream__ec2|UpdateStack|NotImplemented
Issue-Commit: 835c0567131aec0cb985cca0545fdf1d4bb32fe2

# appstream UpdateStack is an unimplemented scaffold stub: HTTP 501 NotImplemented instead of applying the update and returning the modified Stack

> _Note: this issue was identified against the code base at commit `835c0567131aec0cb985cca0545fdf1d4bb32fe2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: `appstream:UpdateStack` is a generated scaffold stub that answers HTTP 501 `NotImplemented` for every request, so no stack mutation path exists at all — neither applying members (`AccessEndpoints`, `UserSettings`, `ApplicationSettings`, `RedirectURL`, ...) nor removing them via `AttributesToDelete`.

Failing check ids for this cause: appstream__ec2.asvpce-update-stack-attributes-to-delete-removes-access-endpoints

AWS: Smithy model `com.amazonaws.appstream#UpdateStack` — "Updates the specified fields for the specified stack." Input `UpdateStackRequest` with `Name` (String, @length min 1, @required), `AttributesToDelete` (`StackAttributes`, a list of the `StackAttribute` enum whose values are STORAGE_CONNECTORS | STORAGE_CONNECTOR_HOMEFOLDERS | STORAGE_CONNECTOR_GOOGLE_DRIVE | STORAGE_CONNECTOR_ONE_DRIVE | REDIRECT_URL | FEEDBACK_URL | THEME_NAME | USER_SETTINGS | EMBED_HOST_DOMAINS | IAM_ROLE_ARN | ACCESS_ENDPOINTS | STREAMING_EXPERIENCE_SETTINGS | CONTENT_REDIRECTION | AGENT_ACCESS_CONFIG | STORAGE_CONNECTOR_AUTHENTICATION), and `AccessEndpoints` (`AccessEndpointList`, @length 1..4). Output `UpdateStackResult` carries `Stack`. Modeled errors: ConcurrentModificationException (400), IncompatibleImageException (400), InvalidAccountStatusException (400), InvalidParameterCombinationException (400), InvalidRoleException (400), LimitExceededException (400), OperationNotPermittedException (400), ResourceInUseException (400), ResourceNotFoundException (404). The model's own member documentation states the deletion semantics: `AgentAccessConfig` — "To remove agent access, use AttributesToDelete with the AGENT_ACCESS_CONFIG value"; `StorageConnectorAuthentication` — "To clear this attribute, add STORAGE_CONNECTOR_AUTHENTICATION to AttributesToDelete." Recorded behavior for the representation detail: naming an attribute in `AttributesToDelete` makes the corresponding key ABSENT from the returned `Stack` and from subsequent `DescribeStacks` output, not present-and-empty; and detaching `ACCESS_ENDPOINTS` from a stack that does not own the VPC endpoint leaves the EC2 interface endpoint alive (its `State` is neither `deleting` nor `deleted`), so the same `VpceId` can be re-attached by a later UpdateStack.

Emulator: every `UpdateStack` call is refused with HTTP 501, error code `NotImplemented`, message `operation UpdateStack is scaffolded but not implemented in this emulator`. In that message the only variable part is the operation name `UpdateStack`; the rest is the scaffold's fixed text. None of the nine modeled errors, and no success response, can ever be produced. The handler body is the generated stub that unconditionally returns `Scaffolded::not_implemented("UpdateStack")`.

Why: a client cannot mutate an AppStream stack at all. `NotImplemented` is not in `UpdateStack`'s modeled error set and 501 is not a status AWS returns here, so SDK error handling falls through to an unmodeled-error path rather than any `ClientError` the caller can branch on. Every downstream workflow is blocked: attaching/detaching streaming interface VPC endpoints, clearing redirect/feedback URLs, changing user settings, and all `AttributesToDelete`-based teardown. Because the operation never runs, the key-absence representation contract (absent vs empty list) and the "detach does not delete the endpoint" invariant are both unverifiable.

Fix: implement the operation handler at services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/update_stack.rs (replace the `Err(Scaffolded::not_implemented(..))` in `UpdateStack::handle` and delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker), backed by the stack store in services/service-com-amazonaws-appstream-photonadminproxyservice/src/state.rs. It must: look the stack up by `Name` and refuse an unknown one with the modeled ResourceNotFoundException (HTTP 404); apply each supplied member to the stored stack; for every value in `AttributesToDelete` REMOVE the corresponding stored field so it serializes as absent rather than as an empty list/empty struct; and keep `ACCESS_ENDPOINTS` removal purely a detach — it must not touch the referenced EC2 VPC endpoint's lifecycle state. The response serializer must then return the updated `Stack` with the deleted keys omitted.

Traces: appstream__ec2.asvpce-update-stack-attributes-to-delete-removes-access-endpoints — the check creates an EC2 interface endpoint for `com.amazonaws.<region>.appstream.streaming`, creates a stack with that endpoint as a STREAMING `AccessEndpoints` entry, and then calls `UpdateStack(Name=..., AttributesToDelete=["ACCESS_ENDPOINTS"])`, which is where the 501 is raised. Note: this cause is distinct from the open cloudformation `UpdateStack` issues (e.g. 20261027T044602Z-rds-cloudformation-updatestack-accepts-a-stack-in-delete-failed.md, 20261026T235900Z-cloudformation-updatestackset-stub.md) — those are a different service's identically named operation and do not cover appstream.

## Evidence
Expected (AWS): 200 with `UpdateStackResult.Stack`; after `AttributesToDelete=["ACCESS_ENDPOINTS"]` the returned stack and a following `DescribeStacks` both carry NO `AccessEndpoints` key (absent, not `[]`); the detached EC2 endpoint's `State` stays out of (`deleting`, `deleted`); re-sending `UpdateStack(Name=..., AccessEndpoints=[{EndpointType: STREAMING, VpceId: <same id>}])` returns the stack with that list restored. Refusals, when they apply, come from the modeled set (ResourceNotFoundException 404, InvalidRoleException 400, ...).

Actual (emulator): HTTP 501, error code `NotImplemented`, message `operation UpdateStack is scaffolded but not implemented in this emulator`. botocore surfaces it as `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateStack operation: operation UpdateStack is scaffolded but not implemented in this emulator`. The assertions on key absence, endpoint survival and re-attach are never reached.

Handler: services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/update_stack.rs lines 3-6 (`//! SCAFFOLDED, NOT IMPLEMENTED:` marker) and line 209 (`Err(crate::scaffolded::Scaffolded::not_implemented("UpdateStack"))`); the 501 mapping is services/service-com-amazonaws-appstream-photonadminproxyservice/src/scaffolded.rs lines 273-291.

## Steps to Reproduce
1. Create a VPC with a subnet in an availability zone offered by the `com.amazonaws.<region>.appstream.streaming` endpoint service (per `ec2:DescribeVpcEndpointServices`).
2. `ec2:CreateVpcEndpoint` with `VpcEndpointType=Interface`, `ServiceName=com.amazonaws.<region>.appstream.streaming` in that subnet; note the returned `VpcEndpointId`.
3. Ensure the `AmazonAppStreamServiceAccess` service role exists (path `/service-role/`, trust principal `appstream.amazonaws.com`).
4. `appstream:CreateStack(Name=<name>, AccessEndpoints=[{"EndpointType": "STREAMING", "VpceId": <endpoint id>}])` and confirm `appstream:DescribeStacks(Names=[<name>])` lists that `AccessEndpoints` value — both succeed today.
5. Call `appstream:UpdateStack(Name=<name>, AttributesToDelete=["ACCESS_ENDPOINTS"])`.
   -> AWS: 200, stack returned with no `AccessEndpoints` key.
   -> Emulator: HTTP 501 `NotImplemented` — `operation UpdateStack is scaffolded but not implemented in this emulator`.
6. Any other `appstream:UpdateStack` call (e.g. just `Name` plus `Description`) reproduces the same 501, confirming the whole operation is stubbed rather than only the `AttributesToDelete` path.
