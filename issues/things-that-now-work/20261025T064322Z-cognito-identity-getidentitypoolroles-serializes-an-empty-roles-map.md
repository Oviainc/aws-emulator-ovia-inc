Issue-Source: trace-sweep-divergence
Issue-Trace: cogid-creds-empty-roles-omits-roles-key
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cogid-creds-empty-roles-omits-roles-key
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|GetIdentityPoolRoles|2xx->2xx|$.Roles
Issue-Commit: f29a5831c5957291c6cb768de44e48e5e558bd01

# cognito-identity: GetIdentityPoolRoles serializes an empty Roles map instead of omitting the unset optional member

> _Note: this issue was identified against the code base at commit `f29a5831c5957291c6cb768de44e48e5e558bd01`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIdentityPoolRoles

## Description
Cause 1 of 1: GetIdentityPoolRoles emits a `Roles` key holding an empty object for an identity pool that has no role map configured, where AWS omits the member entirely.

AWS: Smithy model services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice — GetIdentityPoolRoles output shape GetIdentityPoolRolesResponse has members IdentityPoolId, Roles (target RolesMap, map, @length min 0 max 2) and RoleMappings (target RoleMappingMap, @length min 0 max 10). `Roles` is NOT @required and carries no @default, so under awsJson1.1 an unset optional member is left out of the serialized body rather than written as an empty map. The recording confirms this: for a pool whose roles have never been set via SetIdentityPoolRoles, the response is 200 with no `Roles` key at all (recorded behavior for the unset case).

Emulator: the same step answers 200 but the body carries `$.Roles` as an empty map — replay reports `$.Roles: unexpected_present (expected None, got {})`. The handler/serializer unconditionally materializes the roles collection from state (an empty map when nothing was stored) instead of treating "no roles configured" as an absent optional member.

Why: a client cannot distinguish "this pool has no role mapping configured" from "roles were configured and are empty". Typed SDKs deserialize the member to Some(empty-map)/an empty dict rather than None/absent, so presence tests (`if 'Roles' in response`, `roles.is_none()`) take the wrong branch, and provisioning logic that bootstraps roles only when the key is missing silently skips the bootstrap. Round-tripping a GetIdentityPoolRoles response back into SetIdentityPoolRoles also changes meaning, since SetIdentityPoolRoles treats Roles as @required.

Fix: cognito-identity GetIdentityPoolRoles response serializer / handler — make the `Roles` (and symmetrically `RoleMappings`) output member optional and omit it when the pool's stored role configuration is absent, rather than projecting an empty map from state.

Traces: fixture cogid-creds-empty-roles-omits-roles-key, step task#6 (operation GetIdentityPoolRoles, expected 200 / actual 200, body key $.Roles).

## Evidence
Replay divergence (verdict=mismatch, service=cognito-identity, operation=GetIdentityPoolRoles):
  expected_status=200, actual_status=200
  detail: "$.Roles: unexpected_present (expected None, got {})"
  divergence_key: service=cognito-identity|GetIdentityPoolRoles|status=200->200|keys=$.Roles
  affected: cogid-creds-empty-roles-omits-roles-key#task#6 (affected_count=1)

Expected (recorded AWS): HTTP 200, body contains IdentityPoolId and NO `Roles` key for a pool with no roles configured.
Actual (emulator): HTTP 200, body contains `"Roles": {}`.

Model citation: GetIdentityPoolRoles.Roles -> RolesMap, map, @length min 0 max 2, not @required, no @default (so absent, not empty, when unset).

## Steps to Reproduce
1. Boot the emulator from the mainline checkout.
2. Replay fixture `cogid-creds-empty-roles-omits-roles-key` and run to step `task#6`: create an identity pool (unauthenticated identities allowed) and, without ever calling SetIdentityPoolRoles, call GetIdentityPoolRoles with that IdentityPoolId.
3. Inspect the 200 response body: the emulator includes `"Roles": {}`; AWS omits the `Roles` member entirely.
