Issue-Source: trace-sweep-divergence
Issue-Trace: iam-change-password-invalid-caller
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-change-password-invalid-caller iam-change-password-non-user-caller-error iam-change-password-wrong-context iam-login-profile-and-password-not-found-errors iam-login-profile-not-found-and-change-password-errors iam-read-only-inspection-of-operator-identity iam-user-identity-and-credential-ops-not-found iam-access-key-status-toggle-self iam-account-properties-invalid-key-error iam-managed-policy-versioning-and-tags iam-pg-list-users-marker-walk iam-instance-profile-role-association-and-tags iam-saml-provider-lifecycle iam-service-specific-credential-unsupported-service
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ChangePassword|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|ChangePassword|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|ChangePassword|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ChangePassword|4xx->4xx|status
Issue-Key: iam|UpdateAccessKey|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|UpdateAccessKey|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|UpdateAccessKey|4xx->4xx|status
Issue-Key: iam|PutAccountProperties|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|PutAccountProperties|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|GetPolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|GetPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|ListUsers|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|ListUsers|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|AddRoleToInstanceProfile|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|AddRoleToInstanceProfile|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|AddRoleToInstanceProfile|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeleteSAMLProvider|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|DeleteSAMLProvider|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|DeleteSAMLProvider|4xx->4xx|status
Issue-Key: iam|CreateServiceSpecificCredential|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|CreateServiceSpecificCredential|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Related: 20261029T184359Z-iam-error-responses-omit-the-responsemetadata-requestid-element.md
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# IAM error codes diverge from the modeled awsQuery error codes

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangePassword

## Description
Eight refusal paths answer with the wrong Error/Code (and in four of them the wrong HTTP status), so a client branching on the code takes the wrong branch.

Cause 1 of 8 - ChangePassword when the caller has no login profile. AWS: 404 Code NoSuchEntity, Message 'Login Profile for User <userName> cannot be found.' (fixed text with the caller's user name). Smithy: ChangePassword models both NoSuchEntityException (404, NoSuchEntity) and InvalidUserTypeException (400, InvalidUserType); the missing-login-profile case is NoSuchEntity. Emulator: 400 Code InvalidUserType 'The request was rejected because the type of user for the transaction was incorrect.'. Why: a client that creates the login profile on NoSuchEntity instead treats the call as a permanent caller-type error. Fix: the ChangePassword handler's order of guard clauses - look the login profile up before classifying the caller type.
Traces: iam-change-password-invalid-caller#task#2.

Cause 2 of 8 - UpdateAccessKey with no UserName. AWS: UserName is optional (Smithy: UpdateAccessKeyRequest.UserName not required) and defaults to the calling user; an unknown AccessKeyId is then 404 Code NoSuchEntity 'The Access Key with id <accessKeyId> cannot be found.'. Emulator: 400 Code ValidationError 'Must specify userName when calling with non-User credentials'. Fix: default UserName to the caller in the UpdateAccessKey handler, then raise NoSuchEntity.
Traces: iam-access-key-status-toggle-self#task#1.

Cause 3 of 8 - PutAccountProperties with a malformed property key. AWS: 400 Code ValidationError with the awsQuery constraint-violation message "1 validation error detected: Value at 'properties' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 50, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^[A-Za-z][A-Za-z0-9/_-]*$]" - fixed text; the member path 'properties' is the input member name. Emulator: 400 Code InvalidInput with emulator prose naming the offending key. Fix: raise the request-validation error (ValidationError) rather than the handler's InvalidInput for input-constraint violations.
Traces: iam-account-properties-invalid-key-error#task#1.

Cause 4 of 8 - GetPolicy with an unparseable ARN. AWS: 400 Code ValidationError, Message 'Invalid ARN:  Could not be parsed!'. Emulator: 400 Code InvalidInput 'ARN <value> is not valid.'. Fix: same - classify ARN parse failures as ValidationError.
Traces: iam-managed-policy-versioning-and-tags#task#14.

Cause 5 of 8 - ListUsers with a marker the service did not issue. AWS: 400 Code ValidationError, Message 'Invalid Marker.'. Emulator: 400 Code ValidationException 'Invalid Marker: the value is not a marker this service issued' - ValidationException is not an IAM awsQuery code at all. Fix: the pagination-marker validator must raise the IAM ValidationError code.
Traces: iam-pg-list-users-marker-walk#task#8.

Cause 6 of 8 - AddRoleToInstanceProfile when the profile already holds a role. AWS: the quota of one role per instance profile is the binding rule, so this is 409 Code LimitExceeded, Message 'Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1' (fixed text with the quota value). Emulator: 409 Code EntityAlreadyExists with prose naming the role and profile. Why: a client that retries on EntityAlreadyExists but surfaces LimitExceeded to the operator does the opposite of what it should. Fix: the AddRoleToInstanceProfile handler must raise the quota error.
Traces: iam-instance-profile-role-association-and-tags#task#10.

Cause 7 of 8 - DeleteSAMLProvider with a malformed ARN. AWS: 400 Code ValidationError, Message 'Invalid resource type in ARN' (Smithy: DeleteSAMLProvider models InvalidInputException 400 and NoSuchEntityException 404; the recorded wire answer for a bad resource type is the 400 validation path). Emulator: 404 Code NoSuchEntity 'Manifest not found for arn <arn>'. Why: a malformed request is reported as a missing resource, so a client retries a call that can never succeed. Fix: validate the ARN shape before the lookup in the DeleteSAMLProvider handler.
Traces: iam-saml-provider-lifecycle#task#9.

Cause 8 of 8 - CreateServiceSpecificCredential for an unsupported service. AWS (recorded): 404 Code NoSuchEntity, Message 'No such service <serviceName> is supported for Service Specific Credentials' - fixed text with the request's ServiceName substituted. Smithy also defines ServiceNotSupportedException (404, awsQueryError code NotSupportedService); the recorded wire answer for an unknown service name is NoSuchEntity. Emulator: 404 Code NotSupportedService with emulator prose. Fix: the CreateServiceSpecificCredential handler must classify an unrecognised service name as NoSuchEntity with the recorded message.
Traces: iam-service-specific-credential-unsupported-service#task#1.

## Evidence
Cause 1: expected HTTP 404 / Code NoSuchEntity / 'Login Profile for User emulator cannot be found.'; actual HTTP 400 / InvalidUserType / 'The request was rejected because the type of user for the transaction was incorrect.'
Cause 2: expected HTTP 404 / NoSuchEntity / 'The Access Key with id BOGUSNONEXISTENTKEYID01 cannot be found.'; actual HTTP 400 / ValidationError / 'Must specify userName when calling with non-User credentials'.
Cause 3: expected ValidationError / "1 validation error detected: Value at 'properties' failed to satisfy constraint: Map keys must satisfy constraint: [...]"; actual InvalidInput / 'Property key must be at most 50 characters of letters, digits, underscores, dashes and the namespace separator /, beginning with a letter: ...'.
Cause 4: expected ValidationError / 'Invalid ARN:  Could not be parsed!'; actual InvalidInput / 'ARN sim2real-probe-not-an-arn is not valid.'
Cause 5: expected ValidationError / 'Invalid Marker.'; actual ValidationException / 'Invalid Marker: the value is not a marker this service issued'.
Cause 6: expected LimitExceeded / 'Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1'; actual EntityAlreadyExists / 'Role <roleName> is already associated with Instance Profile <profileName>.'
Cause 7: expected HTTP 400 / ValidationError / 'Invalid resource type in ARN'; actual HTTP 404 / NoSuchEntity / 'Manifest not found for arn arn:aws:iam:::bad-arn'.
Cause 8: expected NoSuchEntity / 'No such service <serviceName> is supported for Service Specific Credentials'; actual NotSupportedService / 'The service <serviceName> does not support service-specific credentials.'

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-change-password-invalid-caller#task#2 - ChangePassword as a caller with no login profile returns 400 InvalidUserType instead of 404 NoSuchEntity.
3. Replay each binding in trace_ids and compare Error/Code and the HTTP status against the recording: iam-access-key-status-toggle-self#task#1, iam-account-properties-invalid-key-error#task#1, iam-managed-policy-versioning-and-tags#task#14, iam-pg-list-users-marker-walk#task#8, iam-instance-profile-role-association-and-tags#task#10, iam-saml-provider-lifecycle#task#9, iam-service-specific-credential-unsupported-service#task#1.
