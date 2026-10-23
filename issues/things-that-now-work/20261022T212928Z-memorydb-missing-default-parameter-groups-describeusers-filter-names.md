Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: memorydb.parameter-group-families-follow-engine-versions memorydb.parameter-update-and-reset-roundtrip memorydb.describe-users-pages-cover-every-user memorydb.acl-membership-reflected-on-users
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|DescribeParameters|ParameterGroupNotFoundFault
Issue-Key: memorydb|DescribeUsers|InvalidParameterCombinationException
Issue-Key: memorydb.acl-membership-reflected-on-users|run#17|KeyError|assert upd["PendingChanges"] == {"UserNamesToAdd": [uiam]}, f"pending changes {upd.get('PendingChanges')!r}"
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# memorydb: missing default parameter groups, DescribeUsers filter names rejected, and UpdateACL omits PendingChanges (3 causes)

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeParameters, DescribeUsers, UpdateACL

## Description
Three independent root causes in the memorydb emulator, found by replaying the behavioral suite (memorydb scored 4/20). Each must be closed before this issue is done.

=== Cause 1 of 3: the AWS-owned default parameter group does not exist for any family ===
AWS: MemoryDB owns one immutable default parameter group per parameter group family, named `default.` + the family name with underscores replaced by hyphens (family `memorydb_valkey7` -> group `default.memorydb-valkey7`; family `memorydb_redis6` -> group `default.memorydb-redis6`). Those groups exist in every account without being created, are returned by DescribeParameterGroups, and DescribeParameters on them returns that family's full parameter set (including `maxmemory-policy`, `timeout`, `activedefrag`), which is also the starting value set of any freshly created custom group in the family and the target of ResetParameterGroup. Smithy: `DescribeParameters` takes a required `ParameterGroupName` and raises `ParameterGroupNotFoundFault`; the model does not enumerate the default groups themselves, so this is recorded behavior only — both recordings read `default.<family>` without ever creating it and expect a parameter list back.
Emulator: DescribeParameters on `default.memorydb-valkey7` and on `default.memorydb-redis6` fails with HTTP 400 `ParameterGroupNotFoundFault` ("Parameter group default.memorydb-valkey7 not found." / "... default.memorydb-redis6 not found."). No default group is seeded for any family named by DescribeEngineVersions, even though DescribeEngineVersions does name those families.
Why: a client cannot read the family defaults, cannot diff a custom group against its family default, and cannot verify a reset. Every parameter-group workflow that starts from the AWS-provided baseline (the documented way to build a custom group) fails against the emulator while it works on AWS. It also makes the "default group cannot be modified" rule unreachable.
Fix: seed the service's initial state with one default parameter group per supported parameter group family (name `default.<family-with-hyphens>`, `IsGlobal`/default semantics, full family parameter set with AWS default values), and have the parameter-group lookup used by DescribeParameterGroups / DescribeParameters / ResetParameterGroup / UpdateParameterGroup resolve those names. The family list must agree with what DescribeEngineVersions reports for both `valkey` and `redis`.
Traces: memorydb.parameter-group-families-follow-engine-versions (fails reading default.memorydb-redis6), memorydb.parameter-update-and-reset-roundtrip (fails reading default.memorydb-valkey7).

=== Cause 2 of 3: DescribeUsers rejects AWS's filter names and uses the wrong error code for an unknown one ===
AWS: the `Filters` member of DescribeUsers is keyed by hyphenated, lower-case property names — `user-name` and `acl-name` — and a filter on `user-name` returns only the matching user. An unrecognised filter name is refused with HTTP 400 and error code `InvalidParameterValueException` (recorded message, as an example: "Invalid filter name UserName."). Smithy: `DescribeUsers.Filters` -> list of `Filter`, whose `Name` targets `FilterName` with only a `\S` pattern and the prose "The property being filtered. For example, UserName." — the model does not enumerate the accepted values, and its doc example is not the wire vocabulary, so the accepted names and the error code are recorded behavior only.
Emulator: `DescribeUsers(Filters=[{Name: "user-name", Values: [...]}])` — a request AWS accepts — is rejected with HTTP 400 `InvalidParameterCombinationException`: "Unknown filter name user-name. Valid filter names are ACLName and UserName." So it accepts the model's doc spelling (`UserName`/`ACLName`) and refuses the real one, and it classifies an unknown filter name as `InvalidParameterCombinationException` instead of `InvalidParameterValueException`.
Why: filtered DescribeUsers calls written against AWS fail outright on the emulator, and a client that branches on the error code (retry/validation paths keyed on InvalidParameterValueException) takes the wrong branch for a bad filter name.
Fix: in the DescribeUsers handler's filter validation (services/service-com-amazonaws-memorydb-amazonmemorydb/src/ops/describe_users.rs), accept the hyphenated names `user-name` and `acl-name`, apply them against the user's name and its ACL membership, and raise `InvalidParameterValueException` for any other name. The same vocabulary applies to DescribeACLs-style filters where modelled.
Traces: memorydb.describe-users-pages-cover-every-user.

=== Cause 3 of 3: UpdateACL's returned ACL omits PendingChanges ===
AWS: UpdateACL returns the updated ACL with `PendingChanges` populated while the membership change is still settling — adding a user yields `PendingChanges = {"UserNamesToAdd": [<user>]}` — and the member disappears from `PendingChanges` once the ACL is back to `active`. Smithy: `UpdateACLResponse.ACL` -> `ACL`, whose member `PendingChanges` targets `ACLPendingChanges` {`UserNamesToAdd`, `UserNamesToRemove`} and is documented "A list of updates being applied to the ACL."
Emulator: the ACL in the UpdateACL response has no `PendingChanges` key at all (the replay raises `KeyError: 'PendingChanges'` on `update_acl(...)["ACL"]["PendingChanges"]`), so the in-flight change is invisible.
Why: a client that polls `PendingChanges` to learn which members are still being applied — the documented way to track an ACL update — sees nothing pending and concludes the update is complete (or crashes on the absent member), while AWS reports the pending add/remove.
Fix: in the UpdateACL handler and the ACL serializer (services/service-com-amazonaws-memorydb-amazonmemorydb/src/ops/update_acl.rs plus the shared ACL representation/state), record the requested UserNamesToAdd/UserNamesToRemove as pending on the ACL, emit them in `ACL.PendingChanges` while the ACL is in `modifying`, and clear the member once it settles to `active`. NOTE: the open issue 20261022T170732Z-memorydb-updateacl-stub.md ("Implement memorydb UpdateACL: unimplemented scaffold stub") concerns UpdateACL being a scaffold stub and carries only memorydb.acl-membership-updates-rejected; the emulator now answers UpdateACL, so this response-shape gap is a separate, net-new cause and is not covered by it.
Traces: memorydb.acl-membership-reflected-on-users.

## Evidence
Suite: simparity-behavioral-aws-v1, memorydb 4/20 (20%), 16 failed.

Cause 1 — memorydb.parameter-group-families-follow-engine-versions:
  botocore.errorfactory.ParameterGroupNotFoundFault: An error occurred (ParameterGroupNotFoundFault) when calling the DescribeParameters operation: Parameter group default.memorydb-redis6 not found.
  (the check derives `default.` + family.replace("_","-") from the families DescribeEngineVersions itself reported)
Cause 1 — memorydb.parameter-update-and-reset-roundtrip:
  botocore.errorfactory.ParameterGroupNotFoundFault: An error occurred (ParameterGroupNotFoundFault) when calling the DescribeParameters operation: Parameter group default.memorydb-valkey7 not found.
  Expected: DescribeParameters(default.memorydb-valkey7) returns the family baseline; a new custom group in memorydb_valkey7 starts equal to it.

Cause 2 — memorydb.describe-users-pages-cover-every-user:
  botocore.errorfactory.InvalidParameterCombinationException: An error occurred (InvalidParameterCombinationException) when calling the DescribeUsers operation: Unknown filter name user-name. Valid filter names are ACLName and UserName.
  Expected (AWS): DescribeUsers(Filters=[{"Name":"user-name","Values":[<name>]}]) returns exactly that one user; an unknown filter name (e.g. "UserName") is refused with HTTP 400 InvalidParameterValueException.
  Actual: the valid name is refused, and the refusal uses InvalidParameterCombinationException.

Cause 3 — memorydb.acl-membership-reflected-on-users:
  File "check_memorydb_acl_membership_reflected_on_users.py", line 98, in run
    assert upd["PendingChanges"] == {"UserNamesToAdd": [uiam]}
  KeyError: 'PendingChanges'
  Expected (AWS): UpdateACL(ACLName=acl, UserNamesToAdd=[u]) -> ACL.PendingChanges == {"UserNamesToAdd": [u]}.
  Actual: the response ACL carries no PendingChanges member.

## Steps to Reproduce
Boot the memorydb emulator and run with a botocore memorydb client.

Cause 1:
  1. DescribeEngineVersions(Engine="valkey") and (Engine="redis"); collect ParameterGroupFamily values.
  2. For each family F, call DescribeParameters(ParameterGroupName="default." + F.replace("_","-")).
  3. Observe HTTP 400 ParameterGroupNotFoundFault instead of the family's parameter list.
  4. Also CreateParameterGroup(Name=X, Family="memorydb_valkey7") then compare DescribeParameters(X) with DescribeParameters("default.memorydb-valkey7") — the second call fails.

Cause 2:
  1. CreateUser(UserName=u, AccessString="on ~* +@all", AuthenticationMode={"Type":"password","Passwords":["<16-128 chars>"]}).
  2. DescribeUsers(Filters=[{"Name":"user-name","Values":[u]}]) -> expect only u; observe HTTP 400 InvalidParameterCombinationException "Unknown filter name user-name".
  3. DescribeUsers(Filters=[{"Name":"UserName","Values":[u]}]) -> expect HTTP 400 InvalidParameterValueException; observe it accepted / mis-coded.

Cause 3:
  1. CreateUser(u1, password mode) and CreateUser(u2, {"Type":"iam"}); CreateACL(ACLName=a, UserNames=[u1]); wait until the ACL is active.
  2. UpdateACL(ACLName=a, UserNamesToAdd=[u2]).
  3. Inspect the response's ACL: PendingChanges is absent, where AWS returns {"UserNamesToAdd": [u2]}.

Replay: memorydb.parameter-group-families-follow-engine-versions, memorydb.parameter-update-and-reset-roundtrip, memorydb.describe-users-pages-cover-every-user, memorydb.acl-membership-reflected-on-users.
