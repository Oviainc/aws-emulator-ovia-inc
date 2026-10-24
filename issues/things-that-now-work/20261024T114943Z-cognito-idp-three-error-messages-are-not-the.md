Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.admin-add-nonexistent-user-to-group-fails cognito-idp.op-contract-adminCreateUser-weak-temp-password-rejected cognito-idp.feature-unavailable-in-tier-domain-mfa-logging
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.admin-add-nonexistent-user-to-group-fails|run#13|AssertionError|assert (code, message) == ("UserNotFoundException", "User does not exist."), (
Issue-Key: cognito-idp.op-contract-adminCreateUser-weak-temp-password-rejected|run#10|AssertionError|assert code.get("Message") == "Password did not conform with password policy: Password not long enough", code.get("Message")
Issue-Key: cognito-idp.feature-unavailable-in-tier-domain-mfa-logging|run#7|AssertionError|assert message1 == "The following feature is not available for the LITE pricing tier configured: Managed Login Version 2", (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# cognito-idp: three error messages are not the message AWS builds for the same condition (3 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminAddUserToGroup

## Description
Three message-construction causes. In each one the HTTP status and the error code already match AWS; what diverges is the message, and in each case the rule that produces AWS's message is known (it is a fixed string for the condition, not a value derived from the request), so the divergence is a defect and not an example difference.

Cause 1 of 3 — UserNotFoundException echoes the username back into the message.
AWS: Smithy UserNotFoundException (httpError 404, member 'message': 'The message returned when a user isn't found'). For an unknown user AWS builds the fixed, request-independent message 'User does not exist.' — it deliberately does NOT name the username, which is what keeps the error from confirming or denying the existence of a particular identity. Recorded behavior confirms the exact string.
Emulator: ('UserNotFoundException', 'User sim2real-behav-cip-nouser-2bfc904a4e3a does not exist.') — the requested username is interpolated into the message.
Why: the message is a user-existence oracle: an unauthenticated or low-privilege caller can distinguish 'this exact user does not exist' from the generic answer, which is the enumeration leak Cognito's fixed wording exists to prevent. Clients and log scrubbers that match the documented string also stop matching.
Fix: the message construction for UserNotFoundException in the shared user-lookup path (AdminAddUserToGroup and every admin operation that resolves a username).
Traces: cognito-idp.admin-add-nonexistent-user-to-group-fails.

Cause 2 of 3 — the InvalidPasswordException message for a too-short password is reworded.
AWS: recorded behavior only, but a fixed string for the condition: 'Password did not conform with password policy: Password not long enough'.
Emulator: 'Password did not conform with policy: Password must be at least 8 characters long' — a different prefix ('with policy' rather than 'with password policy') and a different, policy-value-interpolated reason clause.
Why: the prefix is the part callers and CLI users match on to detect a policy rejection, and interpolating the configured minimum discloses the pool's policy to a caller that has not been allowed to read it.
Fix: the message construction of the password-policy rejection in the AdminCreateUser handler's validation (shared with SignUp/AdminSetUserPassword).
Traces: cognito-idp.op-contract-adminCreateUser-weak-temp-password-rejected.

Cause 3 of 3 — the FeatureUnavailableInTierException message does not use AWS's tier-refusal template.
AWS: Smithy FeatureUnavailableInTierException (httpError 403) — 'thrown when a feature that you attempted to use or configure isn't included in your user pool's current feature plan'. Recorded behavior: the message follows the fixed template 'The following feature is not available for the <TIER> pricing tier configured: <Feature>', e.g. 'The following feature is not available for the LITE pricing tier configured: Managed Login Version 2'.
Emulator: 'Managed login branding version 2 is not included in the LITE feature plan.' — the same facts in prose the template never produces.
Why: the template is the machine-readable part of a tier refusal (callers parse out the feature name to tell the operator what to upgrade); free-form prose breaks that parse while the code still says the request was refused for tier reasons.
Fix: the message template for FeatureUnavailableInTierException in the tier-gating path shared by the domain / MFA / log-delivery handlers.
Traces: cognito-idp.feature-unavailable-in-tier-domain-mfa-logging.

## Evidence
Cause 1: AssertionError: expected UserNotFoundException for unknown user, got ('UserNotFoundException', 'User sim2real-behav-cip-nouser-2bfc904a4e3a does not exist.'); the check asserts (code, message) == ('UserNotFoundException', 'User does not exist.'). Status 404 and the code are already correct.
Cause 2: AssertionError: Password did not conform with policy: Password must be at least 8 characters long; the check asserts the message == 'Password did not conform with password policy: Password not long enough'.
Cause 3: AssertionError: expected the exact tier refusal message, got 'Managed login branding version 2 is not included in the LITE feature plan.'; the check asserts the message == 'The following feature is not available for the LITE pricing tier configured: Managed Login Version 2'.
None of the three checks carried an 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
Cause 1: CreateUserPool + CreateGroup, then AdminAddUserToGroup with a username that was never created -> expect ('UserNotFoundException', 'User does not exist.'), observe the username interpolated into the message.
Cause 2: CreateUserPool with PasswordPolicy.MinimumLength=8, then AdminCreateUser with TemporaryPassword='Ab1!' -> compare the InvalidPasswordException message with 'Password did not conform with password policy: Password not long enough'.
Cause 3: CreateUserPool with UserPoolTier='LITE', then CreateUserPoolDomain with ManagedLoginVersion=2 -> compare the FeatureUnavailableInTierException message with the 'The following feature is not available for the LITE pricing tier configured: Managed Login Version 2' template.
