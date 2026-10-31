Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.admin-set-user-password-weak-password-rejected cognito-idp.admin-set-weak-password-fails cognito-idp.configured-password-policy-governs-accepted-passwords cognito-idp.signup-weak-password-fails cognito-idp.weak-password-signup-rejected cognito-idp.op-contract-adminSetUserPassword cognito-idp.invalid-pool-name-fails cognito-idp.get-csv-header-malformed-pool-id-rejected cognito-idp.get-csv-header-invalid-pool-id-rejected cognito-idp.describe-terms-by-client-nonexistent-terms-precise-error cognito-idp.log-delivery-configuration-empty-vs-absent-fields-preserved cognito-idp.op-contract-delete-user-pool cognito-idp.invalid-domain-name-fails cognito-idp.group-description-exceeds-max-length-rejected cognito-idp.set-ui-customization-oversized-css-rejected cognito-idp.add-custom-attributes-duplicate-name-rejected cognito-idp.admin-delete-user-attributes-unknown-attribute-rejected cognito-idp.admin-update-unknown-attribute-fails cognito-idp.immutable-custom-attribute-update-rejected cognito-idp.op-contract-list-users cognito-idp.list-groups-invalid-next-token-rejected cognito-idp.list-groups-invalid-pagination-token-rejected cognito-idp.op-contract-list-user-pools cognito-idp.op-admin-get-device-nonexistent-rejected cognito-idp.set-user-mfa-preference-unverified-sms-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.admin-set-user-password-weak-password-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.admin-set-weak-password-fails|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.configured-password-policy-governs-accepted-passwords|run#15|AssertionError|assert too_short_message == _POLICY_PREFIX + "Password not long enough", (
Issue-Key: cognito-idp.signup-weak-password-fails|run#15|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.weak-password-signup-rejected|run#10|AssertionError|assert message == expected, (
Issue-Key: cognito-idp.op-contract-adminSetUserPassword|run#15|AssertionError|assert code.get("Message") == "Password does not conform to policy: Password not long enough", code.get("Message")
Issue-Key: cognito-idp.invalid-pool-name-fails|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.get-csv-header-malformed-pool-id-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.get-csv-header-invalid-pool-id-rejected|run#4|AssertionError|assert code == ("InvalidParameterException", (
Issue-Key: cognito-idp.describe-terms-by-client-nonexistent-terms-precise-error|run#13|AssertionError|assert malformed_message == expected_malformed, (
Issue-Key: cognito-idp.log-delivery-configuration-empty-vs-absent-fields-preserved|run#21|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.op-contract-delete-user-pool|run#14|AssertionError|assert bad_message == (
Issue-Key: cognito-idp.invalid-domain-name-fails|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.group-description-exceeds-max-length-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.set-ui-customization-oversized-css-rejected|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.add-custom-attributes-duplicate-name-rejected|run#10|AssertionError|assert (code, message) == ("InvalidParameterException", "Entity Type: USER, Message: AttributeName: tier, DataType: STRING, Error: Attribute name is not unique"
Issue-Key: cognito-idp.admin-delete-user-attributes-unknown-attribute-rejected|run#10|AssertionError|assert (code, message) == ("InvalidParameterException", "Invalid user attributes: Type for attribute {custom:never_defined_attr} could not be determined"), f"ex
Issue-Key: cognito-idp.admin-update-unknown-attribute-fails|run#12|AssertionError|assert msg == "user.custom:not_in_schema: Attribute does not exist in the schema.\n", f"expected the exact {code} message, got {msg!r}"
Issue-Key: cognito-idp.immutable-custom-attribute-update-rejected|run#13|AssertionError|assert code_message == code_expected, (
Issue-Key: cognito-idp.op-contract-list-users|run#15|AssertionError|assert err.get("Message") == want, f"expected message {want!r}, got {err.get('Message')!r}"
Issue-Key: cognito-idp.list-groups-invalid-next-token-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.list-groups-invalid-pagination-token-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.op-contract-list-user-pools|run#12|AssertionError|assert err.get("Message") == want, f"expected message {want!r}, got {err.get('Message')!r}"
Issue-Key: cognito-idp.op-admin-get-device-nonexistent-rejected|run#7|AssertionError|assert malformed_code.get("Message") == (
Issue-Key: cognito-idp.set-user-mfa-preference-unverified-sms-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# cognito-idp: input-validation refusals carry the wrong message — anchored regexes, wrong password-policy prose, wrong attribute/filter/pagination text (10 causes)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPool

## Description
Ten validation refusals answer with a message that differs from AWS's. The most systematic one is cause 2: the emulator prints the Smithy pattern WITH its ^...$ anchors, while AWS prints the pattern unanchored. These are net-new; 20261024T114943Z-cognito-idp-user-pool-and-app-client-configuration.md and 20261026T235252Z-cognito-idp-five-causes-declared-custom-attribute-constraints.md are open but carry other replay keys.

Cause 1 of 10 — password-policy refusal text.
AWS: InvalidPasswordException. The text is operation-dependent: AdminSetUserPassword answers 'Password does not conform to policy: Password not long enough'; SignUp/AdminCreateUser/ConfirmForgotPassword answer 'Password did not conform with policy: Password not long enough'. Fixed text plus a trailing reason clause ('Password not long enough', 'Password must have uppercase characters', ...) selected from the pool's PasswordPolicy.
Emulator: one string for all of them, 'Password did not conform with password policy: Password not long enough' — the word 'password' is duplicated and the AdminSetUserPassword form is never produced.
Why: clients that surface AWS's text to end users, or test for it, see a message AWS never sends.
Fix: the shared password-policy validation error construction, parameterised per operation.
Traces: cognito-idp.admin-set-user-password-weak-password-rejected, cognito-idp.admin-set-weak-password-fails, cognito-idp.configured-password-policy-governs-accepted-passwords, cognito-idp.signup-weak-password-fails, cognito-idp.weak-password-signup-rejected, cognito-idp.op-contract-adminSetUserPassword.

Cause 2 of 10 — constraint-violation messages print the anchored regex.
AWS: '1 validation error detected: Value \'<value>\' at \'<member>\' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>' where <pattern> is printed WITHOUT the ^ and $ anchors, e.g. [\w-]+_[0-9a-zA-Z]+ for UserPoolIdType, [\w+]+ for ClientIdType, [\w\s+=,.@-]+ for poolName, and the logGroupArn ARN pattern without anchors. (Smithy declares them anchored: UserPoolIdType pattern '^[\w-]+_[0-9a-zA-Z]+$', ClientIdType '^[\w+]+$' — AWS strips the anchors when rendering the message.)
Emulator: emits the pattern verbatim from the model, anchors included (e.g. '... pattern: ^[\w-]+_[0-9a-zA-Z]+$').
Why: every constraint-violation message in the service differs from AWS by two characters, so exact-match tooling fails service-wide.
Fix: the request-validation message renderer (strip leading ^ / trailing $ when formatting the pattern).
Traces: cognito-idp.invalid-pool-name-fails, cognito-idp.get-csv-header-malformed-pool-id-rejected, cognito-idp.get-csv-header-invalid-pool-id-rejected, cognito-idp.describe-terms-by-client-nonexistent-terms-precise-error, cognito-idp.log-delivery-configuration-empty-vs-absent-fields-preserved, cognito-idp.op-contract-delete-user-pool.

Cause 3 of 10 — CreateUserPoolDomain with an invalid character.
AWS: InvalidParameterException with prose, not a constraint report: 'The domain name contains an invalid character. Domain names can only contain lower-case letters, numbers, and hyphens. Please enter a different name that follows this format: ^[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?$' (here AWS does include the anchored pattern as part of its prose).
Emulator: the generic '1 validation error detected: Value \'Invalid_Domain_Name\' at \'domain\' failed to satisfy constraint: ...'.
Fix: CreateUserPoolDomain domain-name validation must raise the documented prose before generic constraint validation.
Traces: cognito-idp.invalid-domain-name-fails.

Cause 4 of 10 — CreateGroup Description over the 2048-char maximum.
AWS: '1 validation error detected: Value \'<value>\' at \'description\' failed to satisfy constraint: Member must have length less than or equal to 2048' (Smithy: DescriptionType @length min 0 max 2048).
Emulator: a different message for the same over-length input.
Fix: CreateGroup length validation message.
Traces: cognito-idp.group-description-exceeds-max-length-rejected.

Cause 5 of 10 — SetUICustomization with oversized CSS.
AWS: a '1 validation error detected: Value \'<css>\' at \'cSS\' failed to satisfy constraint: ...' message naming the cSS member.
Emulator: a different message.
Fix: SetUICustomization CSS-size validation message.
Traces: cognito-idp.set-ui-customization-oversized-css-rejected.

Cause 6 of 10 — schema/attribute refusals.
AWS: AddCustomAttributes redefining an existing custom attribute -> InvalidParameterException 'Entity Type: USER, Message: AttributeName: <name>, DataType: <TYPE>, Error: Attribute name is not unique' (fixed skeleton, variables are the requested attribute name and data type). AdminUpdateUserAttributes/AdminDeleteUserAttributes naming an attribute absent from the schema -> InvalidParameterException with AWS's own text. Updating an immutable attribute -> InvalidParameterException 'user.<attributeName>: Attribute cannot be updated.\n' (note AWS's trailing newline).
Emulator: 'Invalid attributes given, custom:tier already exists in the schema of this user pool' / 'Invalid attributes given, custom:never_defined_attr is not a valid attribute of this user pool' / "Attribute custom:sim2realfixed cannot be updated: this user pool's schema declares it immutable".
Fix: AddCustomAttributes and the Admin*UserAttributes schema-validation paths.
Traces: cognito-idp.add-custom-attributes-duplicate-name-rejected, cognito-idp.admin-delete-user-attributes-unknown-attribute-rejected, cognito-idp.admin-update-unknown-attribute-fails, cognito-idp.immutable-custom-attribute-update-rejected.

Cause 7 of 10 — ListUsers with an unparseable Filter.
AWS: InvalidParameterException 'Error while parsing filter.' (fixed text).
Emulator: 'Input fails to satisfy the constraints.'
Fix: ListUsers filter parser error path.
Traces: cognito-idp.op-contract-list-users.

Cause 8 of 10 — invalid pagination token.
AWS: InvalidParameterException. ListGroups answers 'Provided pagination token is invalid. Check the token and try your request again.'; ListUserPools answers 'Invalid pagination token given.' Both are fixed text.
Emulator: 'NextToken is not a pagination token this operation issued for this user pool' / 'NextToken is not a pagination token this operation issued'.
Fix: the pagination-token decode failure path of each list operation.
Traces: cognito-idp.list-groups-invalid-next-token-rejected, cognito-idp.list-groups-invalid-pagination-token-rejected, cognito-idp.op-contract-list-user-pools.

Cause 9 of 10 — AdminGetDevice with an unknown DeviceKey.
AWS: answers the not-found class for a well-formed-but-unknown device key.
Emulator: refuses it as a pattern violation: '1 validation error detected: Value \'us-west-2_ghost-device-key\' at \'deviceKey\' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w-]+_[0-9a-f-]+$' — i.e. the emulator applies a stricter device-key pattern than AWS and reports a constraint error where AWS reports a lookup failure.
Fix: AdminGetDevice device-key validation / lookup ordering.
Traces: cognito-idp.op-admin-get-device-nonexistent-rejected.

Cause 10 of 10 — SetUserMFAPreference preferring an un-enrolled SMS factor.
AWS: InvalidParameterException "Can't mark a disabled mfa as preferred" (fixed text).
Emulator: 'User does not have delivery config set to turn on SMS_MFA'.
Fix: SetUserMFAPreference precondition check message.
Traces: cognito-idp.set-user-mfa-preference-unverified-sms-rejected.

## Evidence
Verbatim from the replay:
- expected 'Password does not conform to policy: Password not long enough' / 'Password did not conform with policy: Password not long enough', got 'Password did not conform with password policy: Password not long enough'
- expected "... pattern: [\\w-]+_[0-9a-zA-Z]+", got "... pattern: ^[\\w-]+_[0-9a-zA-Z]+$" (same for [\\w+]+, [\\w\\s+=,.@-]+ and the logGroupArn ARN pattern)
- expected 'The domain name contains an invalid character. Domain names can only contain lower-case letters, numbers, and hyphens. Please enter a different name that follows this format: ^[a-z0-9](?:[a-z0-9\\-]{0,61}[a-z0-9])?$', got "1 validation error detected: Value 'Invalid_Domain_Name' at 'domain' failed to satisfy constraint: ..."
- expected 'Entity Type: USER, Message: AttributeName: tier, DataType: STRING, Error: Attribute name is not unique', got 'Invalid attributes given, custom:tier already exists in the schema of this user pool'
- expected 'user.custom:sim2realfixed: Attribute cannot be updated.\\n', got "Attribute custom:sim2realfixed cannot be updated: this user pool's schema declares it immutable"
- expected 'Error while parsing filter.', got 'Input fails to satisfy the constraints.'
- expected 'Provided pagination token is invalid. Check the token and try your request again.', got 'NextToken is not a pagination token this operation issued for this user pool'
- expected 'Invalid pagination token given.', got 'NextToken is not a pagination token this operation issued'
- AdminGetDevice: '1 validation error detected: Value ... at 'deviceKey' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w-]+_[0-9a-f-]+$'
- expected "Can't mark a disabled mfa as preferred", got 'User does not have delivery config set to turn on SMS_MFA'

## Steps to Reproduce
1. CreateUserPool(PoolName='bad/pool*name!') -> compare the printed regex against AWS's unanchored form.
2. SignUp / AdminSetUserPassword with a 4-character password against a pool with MinimumLength=8 -> compare InvalidPasswordException.Message.
3. CreateUserPool(Schema=[{Name:'tier',...}]) then AddCustomAttributes(CustomAttributes=[{Name:'tier',...}]) -> compare InvalidParameterException.Message.
4. ListGroups(NextToken='garbage'), ListUsers(Filter='((')  -> compare messages.
5. AdminGetDevice(DeviceKey='us-west-2_ghost-device-key') -> observe a constraint error instead of a lookup failure.
