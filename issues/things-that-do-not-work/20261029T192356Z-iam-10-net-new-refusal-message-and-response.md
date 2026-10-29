Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.attach-list-user-policies iam.attach-same-managed-policy-twice-is-idempotent iam.detach-role-policy iam.op-contract-detachUserPolicy iam.op-contract-detachRolePolicy iam.op-contract-detachGroupPolicy iam.delete-policy iam.op-contract-getPolicyVersion iam.op-contract-list-policy-versions iam.delete-permissions-boundary-nonexistent-404 iam.op-contract-deleteRolePermissionsBoundary iam.user-permissions-boundary-lifecycle iam.entity-name-length-and-charset-rejected iam.op-contract-list-groups iam.op-contract-list-instance-profiles iam.op-contract-list-policies iam.op-contract-list-roles iam.op-contract-list-users iam.op-contract-listUsers-path-prefix iam.op-contract-getAccountProperties iam.op-contract-put-account-properties iam.remove-role-from-instance-profile iam.op-contract-remove-role-from-instance-profile iam.op-contract-add-role-to-instance-profile iam.op-contract-deleteUserPermissionsBoundary
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.attach-list-user-policies|run#17|AssertionError|assert message == f"Policy {arn} was not found.", (
Issue-Key: iam.attach-same-managed-policy-twice-is-idempotent|run#20|AssertionError|assert msg == f"Policy {policy_arn} was not found.", f"second detach from the {kind}: message {msg!r}"
Issue-Key: iam|DetachRolePolicy|NoSuchEntity
Issue-Key: iam.op-contract-detachUserPolicy|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iam.op-contract-detachRolePolicy|run#15|AssertionError|assert code_msg == f"Policy {_DENY_ALL_POLICY_ARN} was not found.", f"unexpected error message {code_msg!r}"
Issue-Key: iam.op-contract-detachGroupPolicy|run#15|AssertionError|assert code_msg == f"Policy {_DENY_ALL_POLICY_ARN} was not found.", f"unexpected error message {code_msg!r}"
Issue-Key: iam.delete-policy|run#13|AssertionError|assert version_message == expected_version, (
Issue-Key: iam.op-contract-getPolicyVersion|run#17|AssertionError|assert message_after_delete == expected_after_delete, (
Issue-Key: iam.op-contract-list-policy-versions|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.delete-permissions-boundary-nonexistent-404|run#13|AssertionError|assert message_role == expected_role, f"expected message {expected_role!r}, got {message_role!r}"
Issue-Key: iam.op-contract-deleteRolePermissionsBoundary|run#15|AssertionError|assert code_msg == f"Principal {fetched_after['RoleId']} does not have scope", f"unexpected error message {code_msg!r}"
Issue-Key: iam.user-permissions-boundary-lifecycle|run#15|AssertionError|assert got["PermissionsBoundary"]["PermissionsBoundaryType"] == "Policy", (
Issue-Key: iam.entity-name-length-and-charset-rejected|_refused_exactly#28|AssertionError|assert got_message == message, f"{what}: expected message {message!r}, got {got_message!r}"
Issue-Key: iam.op-contract-list-groups|run#13|AssertionError|assert message == _PATH_PREFIX_MESSAGE, f"expected message {_PATH_PREFIX_MESSAGE!r}, got {message!r}"
Issue-Key: iam.op-contract-list-instance-profiles|run#13|AssertionError|assert message == _PATH_PREFIX_MESSAGE, f"expected message {_PATH_PREFIX_MESSAGE!r}, got {message!r}"
Issue-Key: iam.op-contract-list-policies|run#15|AssertionError|assert message == _PATH_PREFIX_MESSAGE, f"expected message {_PATH_PREFIX_MESSAGE!r}, got {message!r}"
Issue-Key: iam.op-contract-list-roles|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.op-contract-list-users|run#21|AssertionError|assert code == "ValidationError", f"unexpected error code {code}"
Issue-Key: iam|ListUsers|ValidationException
Issue-Key: iam.op-contract-getAccountProperties|run#12|AssertionError|assert code == "ValidationError", f"expected ValidationError, got {code!r}"
Issue-Key: iam.op-contract-put-account-properties|run#13|AssertionError|assert code == "ValidationError", f"expected ValidationError, got {code}"
Issue-Key: iam|RemoveRoleFromInstanceProfile|NoSuchEntity
Issue-Key: iam.op-contract-remove-role-from-instance-profile|run#21|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.op-contract-add-role-to-instance-profile|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.op-contract-deleteUserPermissionsBoundary|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# iam: 10 net-new refusal-message and response-shape divergences — detach/policy-not-found wording, boundary-scope wording, generic Smithy validation text, ValidationException for a bad Marker, InvalidInput for account properties, wrong quota name

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetachUserPolicy

## Description
Ten distinct net-new root causes, all in IAM's refusal wording / refusal codes / response shape. Each must be closed before this issue is submitted. Values shown below are templates: the fixed text is the requirement, the names/ARNs/ids in it come from the request and the emulator's state.

Cause 1 of 10: Detach{User,Role,Group}Policy on a policy that is not attached returns a re-worded NoSuchEntity message.
AWS: the message is exactly 'Policy <PolicyArn> was not found.' — fixed text 'Policy ' + the request's PolicyArn + ' was not found.' Smithy: DetachUserPolicy models NoSuchEntityException (code NoSuchEntity, HTTP 404).
Emulator: 'Policy <PolicyArn> was not found attached to user <UserName>.' (and the role/group variants 'was not found attached to role <RoleName>.' / 'was not found attached to group <GroupName>.') — an added clause AWS does not emit.
Why: a client matching on AWS's message (or logging it) sees text AWS never produces.
Fix: the message construction in the DetachUserPolicy / DetachRolePolicy / DetachGroupPolicy handlers.
Traces: iam.attach-list-user-policies, iam.attach-same-managed-policy-twice-is-idempotent, iam.detach-role-policy, iam.op-contract-detachUserPolicy, iam.op-contract-detachRolePolicy, iam.op-contract-detachGroupPolicy

Cause 2 of 10: operations naming a deleted or nonexistent managed-policy ARN report the wrong NoSuchEntity message.
AWS: for CreatePolicyVersion, GetPolicyVersion and ListPolicyVersions the message is 'Policy <PolicyArn> does not exist or is not attachable.' (GetPolicyVersion also names the version: 'Policy <PolicyArn> version <VersionId> does not exist or is not attachable.'). Smithy: all three model NoSuchEntityException (NoSuchEntity, 404).
Emulator: all three answer 'Policy <PolicyArn> was not found.'
Why: the two messages mean different things to a client (absent vs not attachable), and neither matches AWS for these operations.
Fix: the NoSuchEntity message construction in the create_policy_version / get_policy_version / list_policy_versions handlers.
Traces: iam.delete-policy, iam.op-contract-getPolicyVersion, iam.op-contract-list-policy-versions

Cause 3 of 10: Delete{Role,User}PermissionsBoundary when no boundary is set reports a re-worded refusal.
AWS: the message is 'Principal <RoleId> does not have scope' — fixed text 'Principal ' + the entity's unique id (AROA.../AIDA...) + ' does not have scope'.
Emulator: 'No permissions boundary is set for the role with name <RoleName>.' — different wording and it names the role by NAME, not by its unique id.
Why: a client keying off the principal id in AWS's message gets no id at all.
Fix: the refusal-message construction in the delete_role_permissions_boundary / delete_user_permissions_boundary handlers.
Traces: iam.delete-permissions-boundary-nonexistent-404, iam.op-contract-deleteRolePermissionsBoundary

Cause 4 of 10: PermissionsBoundary.PermissionsBoundaryType is reported as 'PermissionsBoundaryPolicy'.
AWS: GetRole/GetUser report PermissionsBoundaryType 'Policy' alongside PermissionsBoundaryArn. Smithy: the member targets PermissionsBoundaryAttachmentType, whose single enum entry is named PermissionsBoundaryPolicy; the recorded AWS wire value is 'Policy', so the recording governs the wire value.
Emulator: the member carries 'PermissionsBoundaryPolicy'.
Why: a client comparing the type against AWS's 'Policy' never matches.
Fix: the PermissionsBoundary serializer in IAM's role/user read path.
Traces: iam.user-permissions-boundary-lifecycle

Cause 5 of 10: name and path-prefix validation refusals use the generic Smithy constraint text instead of IAM's own message.
AWS: an invalid userName character class is refused with 'The specified value for userName is invalid. It must contain only alphanumeric characters and/or the following: +=,.@_-'; an invalid pathPrefix is refused with 'The specified value for pathPrefix is invalid. It must begin with the / character and contain only alphanumeric characters and/or / characters.' Fixed text as quoted; the member name is the offending input member.
Emulator: emits the framework's constraint report instead, e.g. "1 validation error detected: Value '<value>' at 'userName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w+=,.@-]+$" and the pathPrefix equivalents with patterns ^\\u002F[\\u0021-\\u007F]*$ / ^((/[A-Za-z0-9\\.,\\+@=_-]+)*)/$.
Why: the emulator leaks an internal regex and never produces AWS's human-facing sentence, so message-matching clients and operators both diverge.
Fix: IAM's input-validation layer must map modelled pattern/length violations for these members onto IAM's own message text (GetUser, ListGroups, ListInstanceProfiles, ListPolicies, ListRoles).
Traces: iam.entity-name-length-and-charset-rejected, iam.op-contract-list-groups, iam.op-contract-list-instance-profiles, iam.op-contract-list-policies, iam.op-contract-list-roles

Cause 6 of 10: an invalid pagination Marker is refused with the wrong error code.
AWS: IAM's Query protocol refuses an unissued Marker with code ValidationError (HTTP 400).
Emulator: answers code ValidationException with message 'Invalid Marker: the value is not a marker this service issued'.
Why: a client catching ValidationError (every AWS SDK's IAM error surface) does not catch ValidationException, so the refusal is unhandled.
Fix: the pagination-marker validation path for ListUsers (and the other list operations sharing it) must raise IAM's ValidationError.
Traces: iam.op-contract-list-users, iam.op-contract-listUsers-path-prefix

Cause 7 of 10: GetAccountProperties / PutAccountProperties refuse with InvalidInput instead of ValidationError.
AWS: an invalid property name/value is refused with code ValidationError. Smithy: the model exposes InvalidInputException (code InvalidInput, HTTP 400) for many IAM operations, but the recording for these two shows ValidationError.
Emulator: answers InvalidInput.
Why: a client catching ValidationError does not catch InvalidInput.
Fix: the refusal classification in the get_account_properties / put_account_properties handlers.
Traces: iam.op-contract-getAccountProperties, iam.op-contract-put-account-properties

Cause 8 of 10: RemoveRoleFromInstanceProfile for a role that is not in the profile reports a re-worded refusal.
AWS: 'Role <RoleName> in Instance Profile <InstanceProfileName> cannot be found.' — fixed text as quoted, with the request's role and instance-profile names. Smithy: RemoveRoleFromInstanceProfile models NoSuchEntityException (NoSuchEntity, 404).
Emulator: 'The role <RoleName> is not associated with Instance Profile <InstanceProfileName>.'
Why: message-matching clients diverge.
Fix: the NoSuchEntity message construction in the remove_role_from_instance_profile handler.
Traces: iam.remove-role-from-instance-profile, iam.op-contract-remove-role-from-instance-profile

Cause 9 of 10: AddRoleToInstanceProfile's quota refusal names the wrong quota.
AWS: adding a second role to an instance profile is refused with LimitExceeded and message 'Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1'. Smithy: AddRoleToInstanceProfile models LimitExceededException (LimitExceeded, HTTP 409).
Emulator: 'Cannot exceed quota for RolesPerInstanceProfile: 1' — the quota name is wrong.
Why: an operator (or a quota-aware client) is pointed at a quota that does not exist by that name.
Fix: the quota-name constant in the add_role_to_instance_profile handler's LimitExceeded message.
Traces: iam.op-contract-add-role-to-instance-profile

Cause 10 of 10: DeleteUserPermissionsBoundary for an absent user reports the wrong not-found wording.
AWS: 'User <UserName> was not found.' — fixed text 'User ' + the request's UserName + ' was not found.'
Emulator: 'The user with name <UserName> cannot be found.' (IAM's GetUser wording, reused where this operation uses a different sentence).
Why: message-matching clients diverge.
Fix: the NoSuchEntity message construction in the delete_user_permissions_boundary handler.
Traces: iam.op-contract-deleteUserPermissionsBoundary

## Evidence
Cause 1 — expected `'Policy arn:aws:iam::aws:policy/AWSDenyAll was not found.'`, got `'Policy arn:aws:iam::aws:policy/AWSDenyAll was not found attached to user <user>.'` (role/group variants identical in form).
Cause 2 — expected `'Policy <arn> does not exist or is not attachable.'` / `'Policy <arn> version v1 does not exist or is not attachable.'`, got `'Policy <arn> was not found.'`.
Cause 3 — expected `'Principal AROABGXEQES5NRCCJJWAN does not have scope'`, got `'No permissions boundary is set for the role with name <role>.'`.
Cause 4 — expected PermissionsBoundaryType `'Policy'`, got `'PermissionsBoundaryPolicy'`.
Cause 5 — expected `'The specified value for userName is invalid. It must contain only alphanumeric characters and/or the following: +=,.@_-'`, got `"1 validation error detected: Value '<v>' at 'userName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w+=,.@-]+$"`; expected `'The specified value for pathPrefix is invalid. It must begin with the / character and contain only alphanumeric characters and/or / characters.'`, got the same generic form at 'pathPrefix'.
Cause 6 — expected code `ValidationError`, got `ValidationException` (`Invalid Marker: the value is not a marker this service issued`).
Cause 7 — expected code `ValidationError`, got `InvalidInput`.
Cause 8 — expected `'Role <role> in Instance Profile <profile> cannot be found.'`, got `'The role <role> is not associated with Instance Profile <profile>.'`.
Cause 9 — expected `'Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1'`, got `'Cannot exceed quota for RolesPerInstanceProfile: 1'`.
Cause 10 — expected `'User <user> was not found.'`, got `'The user with name <user> cannot be found.'`.

## Steps to Reproduce
Against the emulator's IAM endpoint: (1) attach then detach a managed policy twice and read the second DetachUserPolicy/DetachRolePolicy/DetachGroupPolicy message; (2) delete a managed policy, then call CreatePolicyVersion / GetPolicyVersion / ListPolicyVersions on its ARN; (3) call DeleteRolePermissionsBoundary / DeleteUserPermissionsBoundary on an entity with no boundary; (4) put a role/user permissions boundary and read the entity back, inspecting PermissionsBoundaryType; (5) call GetUser with a '/' in UserName and ListGroups/ListInstanceProfiles/ListPolicies/ListRoles with PathPrefix='badpath'; (6) call ListUsers with a Marker the service never issued; (7) call GetAccountProperties/PutAccountProperties with an invalid property; (8) call RemoveRoleFromInstanceProfile for a role not in the profile; (9) add a second role to an instance profile; (10) call DeleteUserPermissionsBoundary for a nonexistent user.
