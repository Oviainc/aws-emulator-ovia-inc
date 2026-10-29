Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: identitystore.concurrent-duplicate-username-creation-single-winner identitystore.delete-group-membership-repeat-delete-idempotent identitystore.create-group-nonexistent-identity-store-rejected identitystore.error-structured-fields-resource-type-and-reason identitystore.update-user-phone-numbers-full-replace
Issue-Shape-Id: com.amazonaws.identitystore#AWSIdentityStore
Issue-Key: identitystore|DeleteUser|ResourceNotFoundException
Issue-Key: identitystore|DeleteGroupMembership|ResourceNotFoundException
Issue-Key: identitystore.create-group-nonexistent-identity-store-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: identitystore.error-structured-fields-resource-type-and-reason|run#29|AssertionError|assert not wrong, f"not-found refusals carry the wrong typed members: {wrong!r}"
Issue-Key: identitystore.update-user-phone-numbers-full-replace|run#15|AssertionError|assert phones[0].get("Primary") is not False, (
Issue-Commit: 6e02dc63462b16cdda1be20178ebf3ce3788d85e

# identitystore: non-idempotent deletes, creates accepted into a nonexistent identity store, and over-populated not-found/PhoneNumber members

> _Note: this issue was identified against the code base at commit `6e02dc63462b16cdda1be20178ebf3ce3788d85e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteUser

## Description
Four independent root causes in the identitystore emulator, found by replaying the behavioral suite (simparity-behavioral-aws-v1) against mainline. All four are in the same service and must ALL be closed before submitting.

Cause 1 of 4: the delete family is not idempotent — a repeat delete of an already-removed resource is refused with ResourceNotFoundException instead of succeeding.
AWS: Smithy marks both DeleteUser and DeleteGroupMembership `idempotent: true` (query="operation" on DeleteUser / DeleteGroupMembership of com-amazonaws-identitystore-awsidentitystore.json: `"idempotent": true`, output shapes DeleteUserResponse / DeleteGroupMembershipResponse are empty). Recorded behavior confirms the semantics the trait implies: deleting an id the store no longer holds returns HTTP 200 with an empty body and changes nothing; the resource stays absent (a later Describe/GetUserId still refuses with ResourceNotFoundException). ResourceNotFoundException (404) is modeled on these operations, but it is reserved for an identity store the account does not have, not for an already-deleted resource id.
Emulator: the second delete is refused. DeleteUser answers HTTP 404 ResourceNotFoundException with message `User not found`; DeleteGroupMembership answers HTTP 404 ResourceNotFoundException with message `Group membership not found for given pool and membershipId.`. In both messages the whole text is fixed per resource class (it carries nothing from the request). The handlers make the store removal itself the not-found signal: delete_user.rs ends with `store.remove_user(&user_key).ok_or_else(not_found)?`, and delete_group_membership.rs with `removed.ok_or_else(|| ...ResourceNotFoundException(crate::state::not_found(ResourceKind::Membership, ...)))?`, so a removal that removed nothing is reported as a missing resource.
Why: idempotent delete is the contract every cleanup path, teardown fixture, retry-after-timeout and Terraform/CDK-style destroy relies on. Against AWS a duplicate or retried delete is a harmless no-op; against the emulator it raises, so a retried delete (a client that times out and retries, two concurrent reconcilers, or a cleanup handler that runs twice) turns a successful teardown into a hard failure, and callers are forced to swallow ResourceNotFoundException to emulate behavior AWS already gives them.
Fix: the DeleteUser and DeleteGroupMembership handlers' removal paths — a removal that finds no record must return the empty success output, keeping the 404 only for a store identifier the emulator does not serve. The same reasoning applies to the other `idempotent: true` deletes in the family (DeleteGroup), which share this shape.
Traces: identitystore.delete-group-membership-repeat-delete-idempotent (errored: ResourceNotFoundException raised on the second DeleteGroupMembership), identitystore.concurrent-duplicate-username-creation-single-winner (failed on the second DeleteUser of the race winner: "expected DeleteUser on an already-deleted user id to be a no-op success, got ResourceNotFoundException: User not found").

Cause 2 of 4: CreateGroup silently CREATES a group in a well-formed but nonexistent IdentityStoreId instead of refusing it.
AWS: Smithy CreateGroup models ResourceNotFoundException (httpError 404, "Indicates that a requested resource is not found"), whose ResourceType enum includes IDENTITY_STORE, and IdentityStoreId is `@required` with pattern `^(arn:aws[a-z-]*:identitystore::\d{12}:identitystore/)?(d-[0-9a-f]{10}|<uuid>)$`. An identity store is NOT caller-creatable through this API — it exists only as the identity store of an IAM Identity Center instance. Recorded behavior: CreateGroup against a well-formed id that is not the account's store is refused with ResourceNotFoundException and the message `IdentityStore not present for IdentityStoreId: <IdentityStoreId>`; the fixed text is `IdentityStore not present for IdentityStoreId: ` and the trailing value is the IdentityStoreId taken verbatim from the request. Nothing is created anywhere.
Emulator: the call SUCCEEDS with HTTP 200 and returns a freshly minted GroupId plus `IdentityStoreId` echoing the bogus id. create_group.rs resolves the store with `state.register_store_mut(&input.identity_store_id)`, which by design treats any well-formed id of this account as "the store the client has" and REGISTERS a new store record on first use; only another account's ARN, or exceeding the recorded store count, is refused. So an arbitrary `d-` id conjures a store.
Why: this is overly permissive in the way that hides real bugs. A client that typos or mis-propagates an IdentityStoreId (a stale value from another account/region, an unset config) gets a 200 and a GroupId that points into a store nothing else can see; writes silently diverge into a phantom store and the group never shows up in the real one. Tests that assert cross-store isolation, and code that branches on ResourceNotFoundException to detect "Identity Center is not enabled here", both pass against the emulator and fail against AWS.
Fix: the identity-store resolution seam used by the write handlers — `crate::state::register_store_mut` / `resolve_store` in state.rs and the CreateGroup handler that calls it: an IdentityStoreId that is not an identity store this emulator actually holds (i.e. not the store of a provisioned Identity Center instance) must be refused with ResourceNotFoundException, ResourceType IDENTITY_STORE, message `IdentityStore not present for IdentityStoreId: <requested id>`, creating nothing. The same resolution must not auto-register on CreateUser/CreateGroupMembership either.
Traces: identitystore.create-group-nonexistent-identity-store-rejected ("expected the call to be rejected, but it SUCCEEDED and returned {'GroupId': ..., 'IdentityStoreId': 'd-9999999999'}", HTTP 200).

Cause 3 of 4: not-found refusals for a Describe BY resource id populate ResourceId, which AWS leaves absent.
AWS: Smithy ResourceNotFoundException carries ResourceType (enum USER | GROUP | IDENTITY_STORE), ResourceId, Reason and Message, and documents Reason as present only for the Customer-Managed-KMS case ("For non-KMS permission errors, this field is not included"). Recorded behavior for a Describe of a deleted resource: HTTP 404 ResourceNotFoundException with ResourceType set to the resource class, Reason absent, ResourceId ABSENT, and the class's fixed message — `User not found` for DescribeUser, `Group not found` for DescribeGroup, `Group membership not found for given pool and membershipId.` for DescribeGroupMembership (each wholly fixed text, carrying nothing from the request). AWS does not echo the probed identifier back in the typed member.
Emulator: the code, message, ResourceType and Reason all match, but ResourceId is populated with the identifier from the request on all three operations. `crate::state::not_found(kind, resource_id)` in state.rs (~line 1000) sets `resource_id: Some(resource_id.to_string())`, and every handler calls it with the request's id (e.g. delete_user.rs / update_user.rs `crate::state::not_found(ResourceKind::User, &input.user_id)`). Note `not_found_by_alternate_identifier` (same file, ~line 1063) already leaves it `None` — the by-id path is the inconsistent one.
Why: a client that keys on the typed member sees a different shape than AWS gives it. Code that treats a present ResourceId as "the service identified a concrete resource" (as opposed to a store-level or identifier-level failure) branches differently, and any assertion or snapshot over the error body diverges. It also leaks the probed identifier into a field AWS deliberately leaves empty.
Fix: the error-construction seam in state.rs — `not_found` must not set ResourceId for the by-id not-found class (leave it None, as the alternate-identifier constructor already does), so DescribeUser/DescribeGroup/DescribeGroupMembership refusals carry only ResourceType and Message.
Traces: identitystore.error-structured-fields-resource-type-and-reason ("not-found refusals carry the wrong typed members": for DescribeUser, DescribeGroup and DescribeGroupMembership, expected ResourceId None, got the requested id).

Cause 4 of 4: UpdateUser on a multi-valued attribute reports Primary=false for a replaced element that supplied no Primary, where AWS reports the member as absent.
AWS: Smithy PhoneNumber has members Value, Type and Primary (target BooleanType, `@default: false`), and User$PhoneNumbers / Emails / Addresses / Roles are modeled `length min 1 max 1`. UpdateUser with AttributePath `phoneNumbers` replaces the whole list element (AttributePath cannot name one list element). Recorded behavior: an element that explicitly carries `Primary: false` at CreateUser is echoed back by DescribeUser as `Primary: false`, but after an UpdateUser whose new element supplies only Value, the subsequent DescribeUser reports the element WITHOUT a Primary member at all — the replaced element's unspecified sub-attributes are absent, not materialized from the model default. The same holds for Type, which the emulator does drop correctly.
Emulator: DescribeUser reports `{'Value': '832-555-0100', 'Primary': False}` — Type correctly dropped, but Primary materialized as false. The decoder in ops/update_user.rs builds the entry with `primary: flag(members.get("Primary").copied(), path)?`, and `flag` maps a missing/null member to `Ok(false)` ("defaulting to the model's own @default: false"), so the stored element always has a concrete boolean that the serializer then emits. Because the pre-update element also had `Primary: false`, the result is indistinguishable from a merge of the old element — which is exactly what the check reports.
Why: a client cannot tell "Primary was not set" from "Primary was explicitly set to false". A SCIM-style sync or a reconciler that diffs the described user against its desired state sees a spurious `Primary: false` it never sent, so it either reports perpetual drift or concludes the replace did not take effect; and the behavior reads as a field-level merge, contradicting UpdateUser's documented whole-element replacement for list-valued attributes.
Fix: the multi-valued attribute decode/representation seam — `flag` / `phone` / `email` / `role` / `address` / `photo` in ops/update_user.rs plus the corresponding state representation, so an unspecified Primary (and any other unspecified sub-attribute) is stored and serialized as ABSENT rather than coerced to the model default. The same representation should be used on the CreateUser path so create and update agree.
Traces: identitystore.update-user-phone-numbers-full-replace ("the old element's Primary=False survived an update that supplied no Primary ... {'Value': '832-555-0100', 'Primary': False}").

Open identitystore issues checked: 20261029T171822Z-identitystore-createdby-updatedby-report-a-synthesized-iam-access.md (CreatedBy/UpdatedBy principal synthesis) and 20261029T171822Z-identitystore-describegroup-misclassifies-an-invalid-identifier-as-resourcenotfound.md (DescribeGroup 400-vs-404 classification and the missing RequestId member). Neither covers any of the four causes above: cause 3 concerns ResourceId being over-populated on a legitimately-404 Describe, not the 400/404 classification of an invalid identifier, and the RequestId gap is not asserted by these behavioral checks.

## Evidence
Behavioral suite run (simparity-behavioral-aws-v1) against the emulator built from mainline: 78/83 passed; 4 mismatched, 1 errored, all 5 in service identitystore. Per-service: identitystore 78/83 (94%), 5 failed.

Cause 1 — non-idempotent delete:
  [error] identitystore.delete-group-membership-repeat-delete-idempotent
    botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteGroupMembership operation: Group membership not found for given pool and membershipId.
    (raised on the SECOND DeleteGroupMembership for the same MembershipId, after DescribeGroupMembership had already confirmed the first delete propagated.)
  [mismatch] identitystore.concurrent-duplicate-username-creation-single-winner
    AssertionError: expected DeleteUser on an already-deleted user id to be a no-op success, got ResourceNotFoundException: User not found
    (the race part of the check passed: exactly one winner, 5 losers all ConflictException.)
  Code: services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/delete_user.rs:209 `store.remove_user(&user_key).ok_or_else(not_found)?`; .../src/ops/delete_group_membership.rs:174-179 `removed.ok_or_else(|| ...not_found(ResourceKind::Membership, &input.membership_id))?`. Smithy marks both operations `idempotent: true`.

Cause 2 — create into a nonexistent identity store:
  [mismatch] identitystore.create-group-nonexistent-identity-store-rejected
    AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'GroupId': '<minted uuid>', 'IdentityStoreId': 'd-9999999999', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
    Expected: ResourceNotFoundException / "IdentityStore not present for IdentityStoreId: d-9999999999" (fixed text `IdentityStore not present for IdentityStoreId: ` + the requested IdentityStoreId; `d-9999999999` and the minted GroupId are this run's values, not requirements).
  Code: services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/create_group.rs:191-200 resolves the store via `state.register_store_mut(&input.identity_store_id)`, documented as registering the store record "on first use" for any well-formed id of this account.

Cause 3 — ResourceId over-populated on not-found:
  [mismatch] identitystore.error-structured-fields-resource-type-and-reason
    AssertionError: not-found refusals carry the wrong typed members:
      DescribeUser:            expected {code: ResourceNotFoundException, message: 'User not found', ResourceType: 'USER', ResourceId: None, Reason: None}
                               got      {... ResourceId: '<the requested UserId>' ...}
      DescribeGroup:           expected {... message: 'Group not found', ResourceType: 'GROUP', ResourceId: None ...}
                               got      {... ResourceId: '<the requested GroupId>' ...}
      DescribeGroupMembership: expected {... message: 'Group membership not found for given pool and membershipId.', ResourceType: 'GROUP_MEMBERSHIP', ResourceId: None ...}
                               got      {... ResourceId: '<the requested MembershipId>' ...}
    Code, message, ResourceType and Reason all matched; ResourceId is the sole diverging member. The concrete uuids in the replay output are per-run values.
  Code: services/service-com-amazonaws-identitystore-awsidentitystore/src/state.rs:1000-1008 `pub fn not_found(kind, resource_id) -> ResourceNotFoundException { ... resource_id: Some(resource_id.to_string()) ... }`; contrast state.rs:1063-1071 `not_found_by_alternate_identifier`, which sets `resource_id: None`.

Cause 4 — Primary materialized on a replaced list element:
  [mismatch] identitystore.update-user-phone-numbers-full-replace
    AssertionError: the old element's Primary=False survived an update that supplied no Primary -- the target merged fields into the existing element instead of replacing it wholesale: {'Value': '832-555-0100', 'Primary': False}
    Sequence: CreateUser with PhoneNumbers=[{Value, Type: 'work', Primary: false}] (DescribeUser round-trips all three, asserted and passing) -> UpdateUser Operations=[{AttributePath: 'phoneNumbers', AttributeValue: [{Value: <new number>}]}] -> DescribeUser. Type was correctly dropped; Primary came back as false where AWS reports no Primary member. The phone numbers in the check are its own values, not requirements.
  Code: services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/update_user.rs:397-406 (`"phonenumbers" => user.phone_numbers = entries(..., phone)?`), :640-649 (`fn phone` -> `primary: flag(members.get("Primary").copied(), path)?`), :444-454 (`fn flag`: `None | Some(Document::Null) => Ok(false)`).

## Steps to Reproduce
1. Clone mainline and boot the emulator; identitystore is bootstrapped (service-com-amazonaws-identitystore-awsidentitystore).
2. Obtain the account's IdentityStoreId via sso-admin ListInstances (creating an instance first if none exists).

Cause 1 (non-idempotent delete):
3. CreateUser, then DeleteUser it, then DeleteUser the SAME UserId again -> expected HTTP 200 empty success; observed HTTP 404 ResourceNotFoundException "User not found".
4. CreateGroup + CreateUser + CreateGroupMembership, DeleteGroupMembership, poll DescribeGroupMembership until it 404s, then DeleteGroupMembership the same MembershipId again -> expected HTTP 200 empty success; observed HTTP 404 ResourceNotFoundException "Group membership not found for given pool and membershipId.".

Cause 2 (create into a nonexistent store):
5. Call CreateGroup with IdentityStoreId set to a well-formed but nonexistent store id (any `d-[0-9a-f]{10}` that is not the account's store, e.g. d-9999999999) and any DisplayName -> expected HTTP 404 ResourceNotFoundException "IdentityStore not present for IdentityStoreId: <that id>"; observed HTTP 200 with a minted GroupId.
6. Confirm nothing was created in the real store: GetGroupId by displayName in the REAL IdentityStoreId -> ResourceNotFoundException "GROUP not found." (this part already holds).

Cause 3 (ResourceId on not-found):
7. Create a user, a group and a membership; delete all three; then call DescribeUser, DescribeGroup and DescribeGroupMembership for the deleted ids and inspect the raw error body -> expected ResourceType set, Reason and ResourceId absent; observed ResourceId echoing the requested identifier on all three.

Cause 4 (Primary materialized):
8. CreateUser with PhoneNumbers=[{Value: "555-010-0001", Type: "work", Primary: false}]; DescribeUser (round-trips).
9. UpdateUser with Operations=[{AttributePath: "phoneNumbers", AttributeValue: [{Value: "832-555-0100"}]}]; DescribeUser -> expected the single element to carry only Value (no Type, no Primary); observed {Value, Primary: false}.
