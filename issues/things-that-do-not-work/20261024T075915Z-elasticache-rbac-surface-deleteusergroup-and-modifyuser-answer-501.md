Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.user-group-not-found-semantics elasticache.modify-user-conflicting-auth-params-rejected elasticache.modify-user-invalid-access-string-rejected
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.user-group-not-found-semantics|_assert_error#16|AssertionError|assert _fault_base(code) == _fault_base(expected), f"expected {expected}, got {code}"
Issue-Key: elasticache.modify-user-conflicting-auth-params-rejected|run#9|AssertionError|assert _fault_base(code) == "InvalidParameterCombination", (
Issue-Key: elasticache.modify-user-invalid-access-string-rejected|run#10|AssertionError|assert code == "InvalidParameterValue", f"expected InvalidParameterValue, got {code}"
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# ElastiCache RBAC surface: DeleteUserGroup and ModifyUser answer 501 NotImplemented instead of their modeled responses and errors (2 causes)

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteUserGroup

## Description
Two operations of the ElastiCache RBAC surface are still scaffold stubs: every request to them is answered with HTTP 501 `NotImplemented`, so neither the success response nor any of the modeled refusals can ever be observed. Both causes are in the same service (`elasticache`) and both are reached over the ordinary Query surface with no supporting EC2/VPC resources, so neither is blocked by another service's gap.

Cause 1 of 2 — DeleteUserGroup is a 501 stub.
AWS: Smithy (com-amazonaws-elasticache-amazonelasticachev9) binds `DeleteUserGroup` with input `DeleteUserGroupMessage{UserGroupId (required)}` and output shape `UserGroup` (UserGroupId/Status/Engine/UserIds/MinimumEngineVersion/PendingChanges/ReplicationGroups/ServerlessCaches/ARN) at HTTP 200. Its modeled errors are `UserGroupNotFoundFault` (httpError 404, awsQueryError code `UserGroupNotFound`), `InvalidUserGroupStateFault` (400, `InvalidUserGroupState`), `InvalidParameterValueException` (400, `InvalidParameterValue`) and `ServiceLinkedRoleNotFoundFault` (400). So a delete of a user-group id that does not exist must be refused with HTTP 404 and error code `UserGroupNotFound` (recorded message example: "User group <id> not found."), and a delete of an existing active group must answer 200 with that group projected into `UserGroup` and move it out of the describable set.
Emulator: the request is answered with HTTP 501, error code `NotImplemented`. The handler's `handle` body is the generated stub that returns `Scaffolded::not_implemented("DeleteUserGroup")`, and the file still carries the `SCAFFOLDED, NOT IMPLEMENTED:` marker line.
Why: a client cannot distinguish "this user group does not exist" from "this API does not exist", and a 501 is not retry/branch-classifiable the way the modeled 404 is — not-found handling (`except UserGroupNotFoundFault`) never fires. More seriously, no user group can ever be removed, so RBAC teardown leaks groups and any create-delete-recreate flow fails on `UserGroupAlreadyExists` forever.
Fix: the operation's handler — `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_user_group.rs` — must resolve `UserGroupId` against the user-group map in `crate::state` (the same map `CreateUserGroup`/`DescribeUserGroups` already use), refuse an unknown id with the modeled `UserGroupNotFoundFault`, refuse a group that is not in a deletable state with `InvalidUserGroupStateFault`, and otherwise remove the group and return its `UserGroup` projection; then drop the SCAFFOLDED marker line.
Traces: elasticache.user-group-not-found-semantics (the check's earlier DescribeUserGroups/ModifyUserGroup not-found legs got past; it fails at the `delete_user_group(UserGroupId=<bogus>)` leg with "expected UserGroupNotFoundFault, got NotImplemented").

Cause 2 of 2 — ModifyUser is a 501 stub, so no ModifyUser rejection or mutation is reachable.
AWS: Smithy binds `ModifyUser` with input `ModifyUserMessage{UserId (required), AccessString, AppendAccessString, Passwords, NoPasswordRequired, AuthenticationMode, Engine}` and output shape `User` at HTTP 200, with modeled errors `InvalidParameterCombinationException` (httpError 400, awsQueryError code `InvalidParameterCombination`), `InvalidParameterValueException` (400, `InvalidParameterValue`), `InvalidUserStateFault` (400, `InvalidUserState`) and `UserNotFoundFault` (404, `UserNotFound`). So: a request that carries both `NoPasswordRequired=true` and an `AuthenticationMode` of type `password` is a two-incompatible-parameters request and must be refused 400 `InvalidParameterCombination` (recorded message example: "No password required flag is true but provided authentication type is password"); a syntactically malformed Redis ACL `AccessString` must be refused 400 `InvalidParameterValue` (recorded message example: "The access-string is invalid."); a `UserId` that no longer exists must be refused 404 `UserNotFound`; and an accepted change must answer 200 with the updated `User` (AccessString normalized, Authentication Type/PasswordCount reflecting the new state) and leave the user `active`. A refused ModifyUser must not partially apply — the user's AccessString and Authentication must be byte-identical afterwards.
Emulator: every ModifyUser request is answered HTTP 501 `NotImplemented`; `handle` is the generated stub returning `Scaffolded::not_implemented("ModifyUser")`.
Why: a user's access string and authentication mode can never be changed after CreateUser, and callers that branch on the modeled classifications (`InvalidParameterCombination` for a contradictory auth request, `InvalidParameterValue` for a bad ACL, `UserNotFound` for a deleted user) instead see one undifferentiated 501 — a non-retryable server-fault class for what are client errors.
Fix: the operation's handler — `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_user.rs` — must resolve `UserId` in the `crate::state` user map, enforce the auth-directive combination rule and the access-string grammar before any write (no partial application), return the modeled `User` on success, and drop the SCAFFOLDED marker line. NOTE: the open issue `20261023T155010Z-elasticache-modifyuser-stub.md` names this same operation and file; this cause is recorded here because these two replayed checks were not attributed to it, and the two checks listed below are the ones that must be re-replayed to confirm the fix.
Traces: elasticache.modify-user-conflicting-auth-params-rejected ("expected InvalidParameterCombination, got NotImplemented"), elasticache.modify-user-invalid-access-string-rejected ("expected InvalidParameterValue, got NotImplemented").

## Evidence
Cause 1 — elasticache.user-group-not-found-semantics:
  call: elasticache DeleteUserGroup(UserGroupId=<id that was never created>)
  expected: HTTP 404, Error.Code `UserGroupNotFound` (modeled `UserGroupNotFoundFault`, awsQueryError httpResponseCode 404); message example "User group <id> not found."
  actual:   HTTP 501, Error.Code `NotImplemented`
  replay assertion: "AssertionError: expected UserGroupNotFoundFault, got NotImplemented" (check line 129, _assert_error on delete_user_group)
  source: services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_user_group.rs line 4 ("SCAFFOLDED, NOT IMPLEMENTED" marker) and line 172 (`Err(crate::scaffolded::Scaffolded::not_implemented("DeleteUserGroup"))`).

Cause 2 — elasticache.modify-user-conflicting-auth-params-rejected:
  setup (succeeds): CreateUser(UserId=<id>, Engine=redis, AccessString="on ~* +@all", Passwords=[...]) -> Authentication.Type=password, PasswordCount=1, status active
  call: ModifyUser(UserId=<id>, NoPasswordRequired=true, AuthenticationMode={Type: password, Passwords: [...]})
  expected: HTTP 400, Error.Code `InvalidParameterCombination`; message example "No password required flag is true but provided authentication type is password"
  actual:   HTTP 501, Error.Code `NotImplemented`
  replay assertion: "AssertionError: expected InvalidParameterCombination, got NotImplemented"

Cause 2 — elasticache.modify-user-invalid-access-string-rejected:
  setup (succeeds): CreateUser(UserId=<id>, Engine=redis, AccessString="off -@all", NoPasswordRequired=true) -> active
  call: ModifyUser(UserId=<id>, AccessString="on ~* &*&*&* not-a-real-acl-token %%%garbage%%%")
  expected: HTTP 400, Error.Code `InvalidParameterValue`; message example "The access-string is invalid."
  actual:   HTTP 501, Error.Code `NotImplemented`
  replay assertion: "AssertionError: expected InvalidParameterValue, got NotImplemented"
  (the same check's later legs — the accepted AccessString change, and ModifyUser on a deleted user answering `UserNotFound` 404 — are unreachable behind the 501)
  source: services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_user.rs (generated stub handler returning `Scaffolded::not_implemented("ModifyUser")`).

## Steps to Reproduce
Cause 1 (DeleteUserGroup):
1. Point an elasticache client at the emulator.
2. aws elasticache delete-user-group --user-group-id does-not-exist-<random>
   -> observe HTTP 501 / Error.Code NotImplemented; AWS answers HTTP 404 / UserGroupNotFound.
3. aws elasticache create-user-group --user-group-id <g> --engine redis --user-ids default ; wait for status active ; aws elasticache delete-user-group --user-group-id <g>
   -> observe HTTP 501; AWS answers 200 with the UserGroup body (Status deleting) and the group stops being describable.
Replay: elasticache.user-group-not-found-semantics

Cause 2 (ModifyUser):
1. aws elasticache create-user --user-id <u> --user-name <u> --engine redis --access-string "on ~* +@all" --passwords <pw> ; wait for status active.
2. aws elasticache modify-user --user-id <u> --no-password-required --authentication-mode Type=password,Passwords=<pw2>
   -> observe HTTP 501 / NotImplemented; AWS answers HTTP 400 / InvalidParameterCombination and leaves Authentication unchanged.
3. aws elasticache modify-user --user-id <u> --access-string "on ~* &*&*&* not-a-real-acl-token %%%garbage%%%"
   -> observe HTTP 501 / NotImplemented; AWS answers HTTP 400 / InvalidParameterValue and leaves AccessString unchanged.
4. aws elasticache modify-user --user-id <u> --access-string "on ~* -@all +@read"
   -> observe HTTP 501 / NotImplemented; AWS answers 200 with the updated User.
Replay: elasticache.modify-user-conflicting-auth-params-rejected, elasticache.modify-user-invalid-access-string-rejected
