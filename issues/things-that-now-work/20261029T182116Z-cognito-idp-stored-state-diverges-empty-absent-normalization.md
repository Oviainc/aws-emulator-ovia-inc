Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.empty-vs-absent-custom-attribute-distinction cognito-idp.group-description-empty-string-vs-absent-distinguished cognito-idp.refarch-cog-omitted-account-recovery-defaults-to-email-first cognito-idp.device-lifecycle-self cognito-idp.self-device-lifecycle cognito-idp.self-service-device-lifecycle cognito-idp.alias-exists-on-attribute-update cognito-idp.op-contract-adminUpdateUserAttributes cognito-idp.identity-federation-linking-lifecycle cognito-idp.identity-provider-linking-lifecycle cognito-idp.provider-linking-alias cognito-idp.op-contract-admin-remove-user-from-group cognito-idp.admin-reset-user-password-unconfirmed-user-fails cognito-idp.confirm-device-duplicate-key-rejected cognito-idp.confirm-device-key-conflict-rejected cognito-idp.username-attributes-email-signin-canonical-username cognito-idp.username-case-sensitivity-configuration-governs-lookup
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.empty-vs-absent-custom-attribute-distinction|run#16|AssertionError|assert "custom:note" not in attrs_with_empty, (
Issue-Key: cognito-idp.group-description-empty-string-vs-absent-distinguished|run#16|AssertionError|assert "Description" not in got_with_empty, f"expected Description omitted, got {got_with_empty!r}"
Issue-Key: cognito-idp.refarch-cog-omitted-account-recovery-defaults-to-email-first|run#4|AssertionError|assert "AccountRecoverySetting" in pool, (
Issue-Key: cognito-idp.device-lifecycle-self|_poll_until#21|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: cognito-idp.self-device-lifecycle|_poll_until#17|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: cognito-idp.self-service-device-lifecycle|_poll_until_device_unlisted#18|AssertionError|raise AssertionError(
Issue-Key: cognito-identity-provider|AdminUpdateUserAttributes|AliasExistsException
Issue-Key: cognito-identity-provider|AdminGetUser|UserNotFoundException
Issue-Key: cognito-identity-provider|AdminRemoveUserFromGroup|ResourceNotFoundException
Issue-Key: cognito-idp.admin-reset-user-password-unconfirmed-user-fails|_error#8|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cognito-idp.confirm-device-duplicate-key-rejected|run#11|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: cognito-idp.confirm-device-key-conflict-rejected|run#15|AssertionError|assert code_cross == "InvalidParameterException", (
Issue-Key: cognito-idp.username-attributes-email-signin-canonical-username|run#12|AssertionError|assert canonical != email, (
Issue-Key: cognito-idp.username-case-sensitivity-configuration-governs-lookup|run#13|AssertionError|assert insensitive_created == lowered, (
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# cognito-idp: stored state diverges — empty/absent normalization, device lifecycle, alias and provider linking, group removal, unconfirmed-user reset, username canonicalization (10 causes)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminGetUser

## Description
Ten root causes where the emulator's stored state or its visible representation differs from AWS. Causes 5, 6 and 10 make legitimate happy paths fail outright, so the bundle is HIGH.

Cause 1 of 10 — an explicit empty-string custom attribute is preserved.
AWS: an attribute written with an empty value is normalized away entirely; AdminGetUser does not list it.
Emulator: it is returned as a present, empty-valued attribute.
Fix: attribute-write normalization in the user state.
Traces: cognito-idp.empty-vs-absent-custom-attribute-distinction.

Cause 2 of 10 — Group Description='' is echoed.
AWS: an empty Description is absent from the GetGroup/CreateGroup Group object (Smithy DescriptionType @length min 0, so '' is accepted on input but not materialised on output).
Emulator: the Group carries Description: ''.
Fix: group serialization must omit an empty Description.
Traces: cognito-idp.group-description-empty-string-vs-absent-distinguished.

Cause 3 of 10 — an omitted AccountRecoverySetting is materialised with the service default.
AWS: DescribeUserPool leaves AccountRecoverySetting ABSENT when CreateUserPool did not supply one.
Emulator: the key is materialised with the default recovery mechanism.
Why: callers that diff a described pool against their template see a field they never set.
Fix: do not default AccountRecoverySetting into the stored/described pool.
Traces: cognito-idp.refarch-cog-omitted-account-recovery-defaults-to-email-first.

Cause 4 of 10 — device status changes are never reflected in ListDevices.
AWS: after UpdateDeviceStatus(DeviceRememberedStatus='not_remembered') (and after AdminForgetDevice/ForgetDevice), the device leaves the user's ListDevices result.
Emulator: the device is still listed 15-30s later; the three device-lifecycle checks all time out on that condition.
Fix: UpdateDeviceStatus/ForgetDevice must write through to the device state ListDevices reads.
Traces: cognito-idp.device-lifecycle-self, cognito-idp.self-device-lifecycle, cognito-idp.self-service-device-lifecycle.

Cause 5 of 10 — (Admin)UpdateUserAttributes wrongly raises AliasExistsException.
AWS: neither UpdateUserAttributes nor AdminUpdateUserAttributes rejects setting a user's email to a value another user already holds as a verified alias; the write is accepted and Cognito DEMOTES the previous holder's copy out of verified status. AliasExistsException (modeled on AdminUpdateUserAttributes) is reserved for ConfirmSignUp.
Emulator: AliasExistsException 'An account with the given email already exists.' — the write is refused.
Why: a supported attribute update fails; the alias-exclusivity semantics are inverted.
Fix: the alias handling in the (Admin)UpdateUserAttributes path.
Traces: cognito-idp.alias-exists-on-attribute-update, cognito-idp.op-contract-adminUpdateUserAttributes.

Cause 6 of 10 — a provider-linked user is not resolvable afterwards.
AWS: after AdminLinkProviderForUser, the destination user is readable with AdminGetUser (and the external identity stays claimed).
Emulator: AdminGetUser raises UserNotFoundException 'User does not exist.'
Why: federation linking leaves no usable user, so the whole linking lifecycle is unreachable.
Fix: AdminLinkProviderForUser must register the link against the destination user in the user state.
Traces: cognito-idp.identity-federation-linking-lifecycle, cognito-idp.identity-provider-linking-lifecycle, cognito-idp.provider-linking-alias.

Cause 7 of 10 — AdminRemoveUserFromGroup is not a no-op for an unknown group.
AWS: AdminRemoveUserFromGroup does not validate that GroupName exists — there is no membership to remove either way, so it returns 200 (Smithy output: empty). ResourceNotFoundException is modeled but not used for this input.
Emulator: ResourceNotFoundException 'Group not found.'
Fix: drop the group-existence check in the AdminRemoveUserFromGroup handler.
Traces: cognito-idp.op-contract-admin-remove-user-from-group.

Cause 8 of 10 — AdminResetUserPassword succeeds for an UNCONFIRMED user.
AWS: resetting the password of a user who has never confirmed is refused.
Emulator: returns 200 with an empty body.
Fix: the user-status precondition in AdminResetUserPassword.
Traces: cognito-idp.admin-reset-user-password-unconfirmed-user-fails.

Cause 9 of 10 — ConfirmDevice error classification.
AWS: InvalidParameterException for a DeviceKey that is already confirmed and for one that belongs to another user/pool.
Emulator: DeviceKeyExistsException for the duplicate and ResourceNotFoundException for the foreign key.
Fix: the ConfirmDevice key-validation path.
Traces: cognito-idp.confirm-device-duplicate-key-rejected, cognito-idp.confirm-device-key-conflict-rejected.

Cause 10 of 10 — username canonicalization.
AWS: in a pool with UsernameAttributes=['email'] the canonical Username is an opaque id, never the email address itself; in a pool with CaseSensitive=false the username is lower-cased on the way in, so AdminCreateUser echoes the lower-cased form.
Emulator: the Username IS the email address, and a mixed-case username is echoed unchanged.
Why: every lookup keyed on AWS's canonical username (and the case-insensitive alias lookups) behaves differently.
Fix: username canonicalization in SignUp/AdminCreateUser and the user index.
Traces: cognito-idp.username-attributes-email-signin-canonical-username, cognito-idp.username-case-sensitivity-configuration-governs-lookup.

## Evidence
Verbatim from the replay:
- 'an explicit empty-string custom attribute was preserved as a present, empty-valued attribute by AdminGetUser; real Cognito normalizes it away entirely'
- "expected Description omitted, got {'GroupName': ..., 'Description': '', ...}"
- 'an omitted AccountRecoverySetting is MATERIALISED with the service default, not left absent'
- 'device us-west-2_a41de41f-... still in ListDevices 15.0s after being marked not_remembered' / 'condition not met within 30.0s'
- uncaught AliasExistsException on AdminUpdateUserAttributes: 'An account with the given email already exists.'
- uncaught UserNotFoundException on AdminGetUser: 'User does not exist.' (after AdminLinkProviderForUser)
- uncaught ResourceNotFoundException on AdminRemoveUserFromGroup: 'Group not found.'
- AdminResetUserPassword on an UNCONFIRMED user: 'expected a ClientError, but the call SUCCEEDED and returned {HTTPStatusCode: 200}'
- 'expected InvalidParameterException, got DeviceKeyExistsException' / 'expected ConfirmDevice with a foreign DeviceKey to be rejected as InvalidParameterException, got ResourceNotFoundException'
- 'in a UsernameAttributes=[email] pool the canonical Username must NOT be the email address itself, got sim2real-behav-cipuseref033d3051d7@example.com' / 'a case-insensitive pool must lower-case the username on the way in'

## Steps to Reproduce
1. AdminUpdateUserAttributes with a custom attribute set to '' then AdminGetUser -> the attribute must be absent.
2. CreateGroup(Description='') then GetGroup -> Description must be absent.
3. CreateUserPool without AccountRecoverySetting then DescribeUserPool -> the key must be absent.
4. Confirm a device, UpdateDeviceStatus(not_remembered), then ListDevices -> the device must be gone.
5. In an AliasAttributes=['email'] pool, give user A a verified email and AdminUpdateUserAttributes user B to the same email -> must succeed.
6. AdminLinkProviderForUser then AdminGetUser on the destination user -> must succeed.
7. AdminRemoveUserFromGroup with a GroupName that does not exist -> must return 200.
8. AdminCreateUser in a UsernameAttributes=['email'] pool -> Username must not be the email.
