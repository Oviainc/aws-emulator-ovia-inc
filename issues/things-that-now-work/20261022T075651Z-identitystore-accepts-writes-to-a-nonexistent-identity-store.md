Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: identitystore.create-group-nonexistent-identity-store-rejected identitystore.create-user-defaults-enabled identitystore.create-user-describe-reflects-fields identitystore.create-user-lifecycle identitystore.group-audit-fields-created-updated-by identitystore.user-audit-fields-created-updated-by identitystore.update-user-immutable-userid-attribute-rejected
Issue-Shape-Id: com.amazonaws.identitystore#AWSIdentityStore
Issue-Key: identitystore.create-group-nonexistent-identity-store-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: identitystore.create-user-defaults-enabled|run#14|KeyError|assert described["UserStatus"] == "ENABLED", (
Issue-Key: identitystore.create-user-describe-reflects-fields|run#13|KeyError|assert desc["UserStatus"] == "ENABLED", f"expected UserStatus='ENABLED', got {desc.get('UserStatus')!r}"
Issue-Key: identitystore.create-user-lifecycle|run#12|KeyError|assert described["UserStatus"] == "ENABLED", f"expected UserStatus ENABLED, got {described.get('UserStatus')!r}"
Issue-Key: identitystore.group-audit-fields-created-updated-by|run#6|AssertionError|assert before.get("CreatedBy"), f"expected CreatedBy populated on create, got {before.get('CreatedBy')!r}"
Issue-Key: identitystore.user-audit-fields-created-updated-by|run#6|AssertionError|assert before.get("CreatedBy"), f"expected CreatedBy populated on a fresh user, got {before.get('CreatedBy')!r}"
Issue-Key: identitystore.update-user-immutable-userid-attribute-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException rejecting a userId mutation, got {code!r}"
Issue-Commit: 4cbced68d8f61275ceb5b427ff8478e9aa7bafe5

# identitystore accepts writes to a nonexistent identity store and drops UserStatus / CreatedBy / UpdatedBy from describe responses (4 causes)

> _Note: this issue was identified against the code base at commit `4cbced68d8f61275ceb5b427ff8478e9aa7bafe5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
Four state/response-fidelity defects in the identitystore emulator, all in service `identitystore`. 4 distinct causes.

Cause 1 of 4: CreateGroup (and CreateUser on the same path) accepts a well-formed but nonexistent IdentityStoreId and creates the resource.
AWS: an IdentityStoreId that satisfies the pattern but names no existing identity store is rejected with 404 ResourceNotFoundException; nothing is created, and the display name does not resolve in any store afterwards (the recorded follow-up GetGroupId against the real store answers ResourceNotFoundException `GROUP not found.`). Smithy: CreateGroup.IdentityStoreId required, @length 1..93, @pattern `^(arn:aws[a-z-]*:identitystore::\d{12}:identitystore/)?(d-[0-9a-f]{10}|...)$`; CreateGroup errors include ResourceNotFoundException @httpError 404.
Emulator: returns HTTP 200 with a freshly minted GroupId and echoes `IdentityStoreId: d-9999999999` — it only pattern-checks the id and implicitly creates/accepts an arbitrary store namespace.
Why: the emulator has no identity-store existence gate, so a client with a typo'd or stale store id silently succeeds and writes groups/users into a namespace that does not exist on AWS; every later read against the correct store then disagrees, and code that relies on the 404 to detect a mis-targeted store never fires.
Fix: the CreateGroup handler's validation plus the store-existence lookup in identitystore state (same gate needed on CreateUser and the other write paths).
Traces: identitystore.create-group-nonexistent-identity-store-rejected.

Cause 2 of 4: DescribeUser omits the modeled UserStatus member.
AWS: a freshly created user reports UserStatus `ENABLED` on DescribeUser. Smithy: DescribeUserResponse.UserStatus targets the UserStatus enum (modeled output member).
Emulator: the DescribeUser response has no UserStatus key at all (the replayed checks raise KeyError: 'UserStatus' against the parsed response).
Why: callers that gate provisioning on a user being ENABLED — the normal Identity Center flow — crash or treat every user as status-unknown; the member is modeled, so SDKs expose it as expected-present for a created user.
Fix: the DescribeUser handler/serializer and the user record in identitystore state (default a created user to ENABLED).
Traces: identitystore.create-user-defaults-enabled, identitystore.create-user-describe-reflects-fields, identitystore.create-user-lifecycle.

Cause 3 of 4: DescribeGroup and DescribeUser never populate the CreatedBy / UpdatedBy audit members.
AWS: a group or user created through the API reports CreatedBy (and UpdatedBy after a mutation) populated on describe. Smithy: DescribeGroupResponse and DescribeUserResponse both model CreatedAt, UpdatedAt, CreatedBy (StringType) and UpdatedBy (StringType).
Emulator: CreatedBy comes back absent/None on a freshly created group and on a freshly created user.
Why: audit/provenance consumers (who created or last changed a principal) read nothing where AWS returns a value, so reconciliation and audit tooling sees every identity as unattributed.
Fix: the group/user records in identitystore state must record the calling principal on create and update, and the DescribeGroup / DescribeUser serializers must emit CreatedBy / UpdatedBy. (The value itself is caller/account-specific; only its presence and population on create/update are being reported.)
Traces: identitystore.group-audit-fields-created-updated-by, identitystore.user-audit-fields-created-updated-by.

Cause 4 of 4: UpdateUser does not reject a mutation of the immutable `userId` attribute.
AWS: UpdateUser with Operations=[{AttributePath: 'userId', ...}] is refused with 400 ValidationException and the message `Updates for AttributePath: userId is not supported` (built from the requested attribute path), and the user is left unchanged. Smithy: UpdateUser errors include ValidationException @httpError 400; UpdateUser.Operations members carry AttributePath.
Emulator: answers `NotImplemented` (`operation UpdateUser is scaffolded but not implemented in this emulator`), so the immutability rule is absent. Related: an open issue covers the bare UpdateUser scaffold stub (`Implement identitystore UpdateUser`); this cause is the behavioral rule that must hold once the handler exists and is NOT satisfied by merely returning success, so it is recorded separately and its check must be re-replayed.
Why: a client probing whether an identifier can be re-pointed gets a 501-class answer instead of the modeled validation rejection; once implemented naively, such a request would mutate an immutable identifier and silently break every reference to the user.
Fix: the UpdateUser handler's attribute-path validation (reject non-updatable paths, userId among them) before applying operations.
Traces: identitystore.update-user-immutable-userid-attribute-rejected.

## Evidence
Cause 1 — CreateGroup(IdentityStoreId='d-9999999999', DisplayName=<unique>) expected to be rejected; actual: HTTPStatusCode 200 and body {'GroupId': 'f6de4597-b3cb-47e6-a006-5f53620361f3', 'IdentityStoreId': 'd-9999999999'} (check assertion: "expected the call to be rejected, but it SUCCEEDED and returned ..."). AWS answers 404 ResourceNotFoundException and creates nothing.

Cause 2 — after CreateUser(UserName=..., DisplayName=..., Name={GivenName,FamilyName}), DescribeUser response contains no 'UserStatus' key: the replay raises `KeyError: 'UserStatus'` at `assert described["UserStatus"] == "ENABLED"` in three independent checks. Expected: UserStatus == 'ENABLED'.

Cause 3 — "AssertionError: expected CreatedBy populated on create, got None" (DescribeGroup) and "AssertionError: expected CreatedBy populated on a fresh user, got None" (DescribeUser).

Cause 4 — UpdateUser(UserId=<existing>, Operations=[{'AttributePath': 'userId', 'AttributeValue': <other id>}]): expected ('ValidationException', 'Updates for AttributePath: userId is not supported'); actual error code 'NotImplemented' with message 'operation UpdateUser is scaffolded but not implemented in this emulator' ("expected ValidationException rejecting a userId mutation, got 'NotImplemented'").

## Steps to Reproduce
1. Resolve an identity store id via sso-admin ListInstances and point an identitystore client at the emulator.
2. Cause 1: identitystore CreateGroup(IdentityStoreId='d-9999999999', DisplayName='<unique>') -> returns 200 with a GroupId instead of 404 ResourceNotFoundException.
3. Cause 2: CreateUser(IdentityStoreId=<store>, UserName='<unique>', DisplayName='<unique>', Name={'GivenName':'Ada','FamilyName':'Lovelace'}) then DescribeUser(IdentityStoreId=<store>, UserId=<new id>) -> response has no UserStatus member; AWS reports 'ENABLED'.
4. Cause 3: CreateGroup(...) then DescribeGroup(...), and CreateUser(...) then DescribeUser(...) -> CreatedBy (and UpdatedBy) are absent/None.
5. Cause 4: CreateUser(...), then UpdateUser(IdentityStoreId=<store>, UserId=<new id>, Operations=[{'AttributePath':'userId','AttributeValue':'<another uuid>'}]) -> NotImplemented instead of 400 ValidationException 'Updates for AttributePath: userId is not supported'.
