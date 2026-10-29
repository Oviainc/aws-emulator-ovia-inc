Issue-Source: trace-sweep-divergence
Issue-Trace: iam-pg-login-profile-password-policy-violation
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-pg-login-profile-password-policy-violation iam-delete-open-id-connect-provider-contract iam-identity-providers-saml-and-oidc-batch6 iam-delegation-request-guard-clauses-and-create-attempt iam-delegation-request-mutations-errors iam-err-service-linked-role-unmodifiable
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|CreateLoginProfile|4xx->2xx|$.ErrorResponse
Issue-Key: iam|CreateLoginProfile|4xx->2xx|status
Issue-Key: iam|GetLoginProfile|4xx->2xx|$.ErrorResponse
Issue-Key: iam|GetLoginProfile|4xx->2xx|status
Issue-Key: iam|CreateLoginProfile|2xx->4xx|$.CreateLoginProfileResponse
Issue-Key: iam|CreateLoginProfile|2xx->4xx|status
Issue-Key: iam|DeleteOpenIDConnectProvider|4xx->2xx|$.ErrorResponse
Issue-Key: iam|DeleteOpenIDConnectProvider|4xx->2xx|status
Issue-Key: iam|CreateDelegationRequest|4xx->2xx|$.ErrorResponse
Issue-Key: iam|CreateDelegationRequest|4xx->2xx|status
Issue-Key: iam|UpdateRole|4xx->2xx|$.ErrorResponse
Issue-Key: iam|UpdateRole|4xx->2xx|status
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# IAM guard clauses missing on login profile OIDC delegation and role

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoginProfile

## Description
Four write paths accept a request AWS refuses, each because a guard clause is not evaluated.

Cause 1 of 4 - CreateLoginProfile ignores the account password policy. AWS: the password is checked against the account password policy and a violating password is refused HTTP 400 with Code PasswordPolicyViolation and a message naming the rule broken, e.g. 'Password should have a minimum length of <MinimumPasswordLength>' - fixed text with the policy's configured minimum substituted (Smithy: PasswordPolicyViolationException, httpError 400, awsQueryError code PasswordPolicyViolation, is modelled on the password-setting operations). Emulator: HTTP 200, the login profile is created with the short password; the follow-on GetLoginProfile then answers 200 where AWS answers 404 NoSuchEntity 'Login Profile for User <userName> cannot be found.', and the later legitimate CreateLoginProfile is refused 409 because the profile the first call should not have created already exists. Why: a client testing its password policy sees it silently unenforced, and every subsequent step in the flow inverts. Fix: evaluate the stored account password policy in the CreateLoginProfile handler (and the UpdateLoginProfile/ChangePassword equivalents).
Traces: iam-pg-login-profile-password-policy-violation#task#4 (200 instead of 400), #task#5 (GetLoginProfile 200 instead of 404 - cascade), #task#6 (CreateLoginProfile 409 instead of 200 - cascade).

Cause 2 of 4 - DeleteOpenIDConnectProvider is idempotent where AWS is not. AWS: deleting a provider ARN that does not exist is refused HTTP 404 with Code NoSuchEntity and Message 'OpenId connect Provider <providerArn> cannot be found.' - fixed text with the request's OpenIDConnectProviderArn substituted. Emulator: HTTP 200 for the second delete. Why: a client that uses the NoSuchEntity 404 to confirm the provider is gone, or to detect a concurrent delete, can never observe it. Fix: the DeleteOpenIDConnectProvider handler must look the ARN up and raise NoSuchEntity when absent.
Traces: iam-delete-open-id-connect-provider-contract#task#4.

Cause 3 of 4 - CreateDelegationRequest does not validate the template ARN. AWS: a template ARN carrying an account number is refused HTTP 400 with Code ValidationError and the fixed message 'Invalid ARN: account numbers are not acceptable for template ARNs!'. Emulator: HTTP 200, the delegation request is created. Why: a malformed delegation is created in the emulator that AWS would reject, so the caller's error handling is never exercised. Fix: ARN validation in the CreateDelegationRequest handler.
Traces: iam-delegation-request-guard-clauses-and-create-attempt#task#7.

Cause 4 of 4 - UpdateRole mutates a service-linked role. AWS: a service-linked role is immutable; changing its MaxSessionDuration is refused HTTP 400 with Code UnmodifiableEntity and Message '<roleName> is an AWS service linked role, max session duration cannot be changed.' - fixed text with the role's name substituted. Emulator: HTTP 200 and the duration is changed. Why: a client can mutate a role AWS protects, and never sees the UnmodifiableEntity signal its error handling is built around. Fix: an is-service-linked guard in the UpdateRole handler.
Traces: iam-err-service-linked-role-unmodifiable#task#8.

## Evidence
Cause 1: expected (recorded AWS) HTTP 400, ErrorResponse/Error{Type Sender, Code PasswordPolicyViolation, Message 'Password should have a minimum length of 24'} where 24 is the policy's MinimumPasswordLength; actual HTTP 200 with a CreateLoginProfileResponse. Cascades: GetLoginProfile expected HTTP 404 Code NoSuchEntity Message 'Login Profile for User <userName> cannot be found.', actual HTTP 200; CreateLoginProfile expected HTTP 200 CreateLoginProfileResult/LoginProfile, actual HTTP 409.
Cause 2: expected HTTP 404 Code NoSuchEntity Message 'OpenId connect Provider <providerArn> cannot be found.'; actual HTTP 200 with no ErrorResponse.
Cause 3: expected HTTP 400 Code ValidationError Message 'Invalid ARN: account numbers are not acceptable for template ARNs!'; actual HTTP 200.
Cause 4: expected HTTP 400 Code UnmodifiableEntity Message '<roleName> is an AWS service linked role, max session duration cannot be changed.'; actual HTTP 200.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-pg-login-profile-password-policy-violation: task#1-2 set a 24-character minimum, task#4 CreateLoginProfile with a shorter password -> 200 instead of 400 PasswordPolicyViolation; task#5 and task#6 then invert.
3. Replay iam-delete-open-id-connect-provider-contract#task#4 (second DeleteOpenIDConnectProvider) -> 200 instead of 404 NoSuchEntity.
4. Replay iam-delegation-request-guard-clauses-and-create-attempt#task#7 (template ARN with an account number) -> 200 instead of 400 ValidationError.
5. Replay iam-err-service-linked-role-unmodifiable#task#8 (UpdateRole MaxSessionDuration on a service-linked role) -> 200 instead of 400 UnmodifiableEntity.
