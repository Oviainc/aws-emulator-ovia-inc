Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.create-describe-delete-domain cognito-idp.invalid-user-pool-configuration-device-ops cognito-idp.get-user-attribute-verification-code-rejects-unverifiable-attribute
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|DescribeUserPoolDomain|ResourceNotFoundException
Issue-Key: cognito-idp.invalid-user-pool-configuration-device-ops|run#12|AssertionError|assert message1 == expected_message1, (
Issue-Key: cognito-idp.get-user-attribute-verification-code-rejects-unverifiable-attribute|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: f29a5831c5957291c6cb768de44e48e5e558bd01

# cognito-idp: three causes — a just-created user pool domain is not findable by DescribeUserPoolDomain, and two condition-specific refusal messages are rewritten

> _Note: this issue was identified against the code base at commit `f29a5831c5957291c6cb768de44e48e5e558bd01`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUserPoolDomain

## Description
Three independent cognito-idp root causes found by the behavioral replay. Each has its own failing check id; all three must be closed before this issue is submitted.

=== Cause 1 of 3: DescribeUserPoolDomain cannot find a prefix domain that CreateUserPoolDomain just accepted (HIGH) ===
AWS: Smithy: CreateUserPoolDomain(Domain required, DomainType len 1..63 pattern ^[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?$; UserPoolId required) returns 200. DescribeUserPoolDomain takes `Domain` ALONE and returns `DomainDescription` (DomainDescriptionType) whose `UserPoolId` is the pool the domain is attached to; the model's doc for DomainDescriptionType.UserPoolId is "The ID of the user pool that the domain is attached to." `ResourceNotFoundException` (HTTP 404) is modeled as "thrown when the Amazon Cognito service can't find the requested resource" — i.e. it is for a domain string that no pool holds. A prefix domain created in this Region is describable in this Region (the model's own DescribeUserPoolDomain doc only excludes a prefix domain queried in a SECONDARY Region). So the sequence CreateUserPoolDomain(Domain=d, UserPoolId=p) -> DescribeUserPoolDomain(Domain=d) must answer 200 with DomainDescription.UserPoolId == p.
Emulator: CreateUserPoolDomain returns 200 for a lowercase prefix domain on a pool created in the same run, and the IMMEDIATELY following DescribeUserPoolDomain(Domain=<that same domain>) is refused with 404 ResourceNotFoundException, message e.g. "User pool domain <domain> does not exist." The domain is therefore write-only: nothing a client can call observes it, so the whole create -> describe -> delete domain cycle is unusable and the downstream legs of the check (delete, then the already-deleted-domain refusal) are never reached.
Why: a client that configures a hosted-UI / managed-login prefix domain and then reads it back — which is how every provisioning flow confirms the domain is attached and learns its Status/CloudFrontDistribution/ManagedLoginVersion — gets a 404 for a resource the service told it it had created. Idempotent provisioners that use DescribeUserPoolDomain as an existence probe will loop or re-create forever, and CloudFormation-style converge logic reads this as "the domain was never made".
Fix: the domain lookup that joins the two sides of this pair — the store write CreateUserPoolDomain performs into the pool's prefix/custom domain slot, and the by-domain-name lookup DescribeUserPoolDomain performs against the whole store. One of the two does not see the other: the create's own post-write read (`pool.domain_named`) succeeds, so the record IS in the pool, and the by-name scan the describe uses does not reach it. Belongs in the service's state module (the by-domain-name lookup) and/or create_user_pool_domain's write, not in the describe serializer — the describe handler is otherwise correct (it reads the stored record and renders it).
Related already-open/closed work, named for the fixer and NOT a duplicate of this: 20261023T185902Z-cognito-identity-provider-createuserpooldomain-stub.md (CreateUserPoolDomain stub, fixed — the create now answers 200 but what it stores is not reachable by name), 20261024T114943Z-cognito-idp-deleteuserpooldomain-stub.md (fixed) and 20261024T114943Z-cognito-idp-updateuserpooldomain-stub.md (open, a different operation). No filed issue names DescribeUserPoolDomain.
Traces: cognito-idp.create-describe-delete-domain. Code read while diagnosing: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/describe_user_pool_domain.rs:158-172 (the by-name lookup and the 404 it raises) and .../src/ops/create_user_pool_domain.rs:264-335 (the uniqueness probe, the write into the pool's domain slot, and the post-write read that succeeds).

=== Cause 2 of 3: the device-tracking-not-enabled refusal carries a different message than the one AWS builds for that condition (MEDIUM) ===
AWS: Smithy: AdminListDevices / AdminGetDevice / AdminUpdateDeviceStatus / AdminForgetDevice all model `InvalidUserPoolConfigurationException` (HTTP 400). The model states the code and status but not the text; recorded behavior only for the text: on a pool that never set DeviceConfiguration, real AWS answers 400 InvalidUserPoolConfigurationException with the fixed, condition-specific message "Device tracking not currently enabled for this pool." — the same constant for all four operations. The rule is: this condition (pool with no DeviceConfiguration) produces that one constant message; it embeds no request value, id, host or timestamp.
Emulator: the status (400) and the error code (InvalidUserPoolConfigurationException) are CORRECT. The message is rewritten to "Device tracking is not configured for this user pool."
Why: a client that classifies this specific misconfiguration by message — and the condition is distinguished from the four other 400s these operations can raise only by code+message — does not recognise the refusal. The code is right, so impact is limited to message-based classification and to test fixtures that pin the AWS text.
Fix: the message constructed for InvalidUserPoolConfigurationException in the Admin*Device operation handlers' validation (shared across admin_list_devices, admin_get_device, admin_update_device_status, admin_forget_device), not the error channel or serializer.
Traces: cognito-idp.invalid-user-pool-configuration-device-ops (failed on the FIRST leg, AdminListDevices; the same constant is asserted for the other three operations and for the downstream ResourceNotFoundException "Device does not exist." legs, which were not reached and are not claimed here).

=== Cause 3 of 3: the unverifiable-attribute refusal of GetUserAttributeVerificationCode carries a different message than the one AWS builds for that condition (LOW) ===
AWS: Smithy: GetUserAttributeVerificationCode(AccessToken required, AttributeName required AttributeNameType len 1..32) models `InvalidParameterException` (HTTP 400). The model states the code and status but not the text; recorded behavior only for the text: asking for a verification code for an attribute that is not email or phone_number is refused 400 InvalidParameterException with the fixed, condition-specific message "Invalid attribute name. Only phone_number and email can be verified." The rule is: this condition (AttributeName outside {email, phone_number}) produces that one constant message, which names the allowed set and embeds no request value.
Emulator: the status (400) and the error code (InvalidParameterException) are CORRECT. The message is rewritten and echoes the requested attribute instead: "Invalid AttributeName: family_name is not an attribute a verification code can be sent for".
Why: the AWS message is a constant that states the permitted set; the emulator's is request-derived and states no set. A client or test that matches on the AWS text, or that parses the allowed attributes out of it, does not recognise the refusal. The code is right, so impact is limited to message-based classification.
Fix: the message constructed for InvalidParameterException in GetUserAttributeVerificationCode's input validation (the verifiable-attribute gate), not the error channel or serializer.
Traces: cognito-idp.get-user-attribute-verification-code-rejects-unverifiable-attribute (the positive-control leg, GetUserAttributeVerificationCode for "email" returning CodeDeliveryDetails, was not reached and is not claimed here).

## Evidence
Cause 1 — cognito-idp.create-describe-delete-domain:
  CreateUserPoolDomain(Domain='sim2real-behav-cip-domain-<id>', UserPoolId=<pool created in the same run>) -> 200 (no error raised)
  DescribeUserPoolDomain(Domain='sim2real-behav-cip-domain-<id>') immediately after
    expected: 200 with DomainDescription.UserPoolId == <that pool id>
    actual:   404 ResourceNotFoundException — botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeUserPoolDomain operation: User pool domain sim2real-behav-cip-domain-ecdb11beaa6e does not exist.
  (the domain name is an example; what diverges is that any just-created prefix domain is not findable by name)

Cause 2 — cognito-idp.invalid-user-pool-configuration-device-ops:
  AdminListDevices on a pool created WITHOUT DeviceConfiguration
    expected: 400 InvalidUserPoolConfigurationException, message 'Device tracking not currently enabled for this pool.'
    actual:   400 InvalidUserPoolConfigurationException, message 'Device tracking is not configured for this user pool.'
    AssertionError: expected message 'Device tracking not currently enabled for this pool.', got 'Device tracking is not configured for this user pool.'
  status and error code MATCH; only the condition-specific constant message differs.

Cause 3 — cognito-idp.get-user-attribute-verification-code-rejects-unverifiable-attribute:
  GetUserAttributeVerificationCode(AccessToken=<user access token>, AttributeName='family_name')
    expected: 400 InvalidParameterException, message 'Invalid attribute name. Only phone_number and email can be verified.'
    actual:   400 InvalidParameterException, message 'Invalid AttributeName: family_name is not an attribute a verification code can be sent for'
    AssertionError: expected message 'Invalid attribute name. Only phone_number and email can be verified.', got 'Invalid AttributeName: family_name is not an attribute a verification code can be sent for'
  status and error code MATCH; only the condition-specific constant message differs (the emulator's is request-derived, AWS's is a constant naming the allowed set).

## Steps to Reproduce
Replay the three checks named in trace_ids against the emulator.

Cause 1 (cognito-idp.create-describe-delete-domain):
1. CreateUserPool(PoolName=<unique>) -> note UserPool.Id
2. CreateUserPoolDomain(Domain=<unique lowercase prefix, e.g. 'cip-domain-<hex>'>, UserPoolId=<that id>) -> succeeds 200
3. DescribeUserPoolDomain(Domain=<same domain>) -> emulator answers 404 ResourceNotFoundException; AWS answers 200 with DomainDescription.UserPoolId == the pool id from step 1.
   (Once fixed, the rest of the check also runs: DeleteUserPoolDomain, then DescribeUserPoolDomain with no UserPoolId bound, then a second DeleteUserPoolDomain on the dead domain.)

Cause 2 (cognito-idp.invalid-user-pool-configuration-device-ops):
1. CreateUserPool(PoolName=<unique>) with NO DeviceConfiguration
2. AdminCreateUser + AdminSetUserPassword(Permanent=True) for a user in that pool
3. AdminListDevices(UserPoolId=<pool>, Username=<user>) -> 400 InvalidUserPoolConfigurationException with message 'Device tracking is not configured for this user pool.' instead of 'Device tracking not currently enabled for this pool.'
4. The same divergence is asserted for AdminGetDevice, AdminUpdateDeviceStatus and AdminForgetDevice on the same pool.

Cause 3 (cognito-idp.get-user-attribute-verification-code-rejects-unverifiable-attribute):
1. CreateUserPool + CreateUserPoolClient(ExplicitAuthFlows=['ALLOW_ADMIN_USER_PASSWORD_AUTH','ALLOW_REFRESH_TOKEN_AUTH'], GenerateSecret=False)
2. AdminCreateUser with UserAttributes email + family_name, AdminSetUserPassword(Permanent=True), AdminInitiateAuth ADMIN_USER_PASSWORD_AUTH -> AccessToken
3. GetUserAttributeVerificationCode(AccessToken=<token>, AttributeName='family_name') -> 400 InvalidParameterException with message 'Invalid AttributeName: family_name is not an attribute a verification code can be sent for' instead of 'Invalid attribute name. Only phone_number and email can be verified.'
