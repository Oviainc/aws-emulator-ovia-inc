Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.entity-temporarily-unmodifiable-enforced iam.delete-permissions-boundary-service-linked-role-rejected iam.limit-exceeded-on-add-and-attach-operations iam.malformed-policy-document-rejected iam.deploy-iam-role-cloudformation-malformed-trust-policy-rejected iam.deploy-iam-managed-policy-via-cloudformation-attaches-and-enforces
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.entity-temporarily-unmodifiable-enforced|_assert_one_of#17|AssertionError|assert code in expected, f"{what}: expected one of {sorted(expected)}, got {code!r}"
Issue-Key: iam.delete-permissions-boundary-service-linked-role-rejected|run#9|AssertionError|assert control_message == expected_control, (
Issue-Key: iam.limit-exceeded-on-add-and-attach-operations|run#34|AssertionError|assert msg_profile == "Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1", (
Issue-Key: iam.malformed-policy-document-rejected|run#65|AssertionError|assert code_uarp_message == expected, f"expected message {expected!r}, got {code_uarp_message!r}"
Issue-Key: iam.deploy-iam-role-cloudformation-malformed-trust-policy-rejected|run#10|AssertionError|assert final_status in ("ROLLBACK_COMPLETE", "CREATE_FAILED"), (
Issue-Key: iam.deploy-iam-managed-policy-via-cloudformation-attaches-and-enforces|_error#35|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Commit: dbad359e500a4b0cc82cfb6a960dace762f1358f

# iam: 6 net-new behavioral root causes — EnableMFADevice is a 501 scaffold stub, three refusal messages are emulator prose, CreateRole accepts a trust statement with no Effect, and S3 DeleteObject is never IAM-authorized

> _Note: this issue was identified against the code base at commit `dbad359e500a4b0cc82cfb6a960dace762f1358f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableMFADevice

## Description
SIX distinct net-new root causes in the bootstrapped `iam` service. None of the six failing checks below is marked ALREADY FILED or OWNED in the replay report. A fixer must close all SIX causes before submitting.

Nearest open neighbours, named so they are not re-fixed or mistaken for coverage: 20261026T051836Z-iam-deleterolepermissionsboundary-is-an-unimplemented-501-scaffold-stub.md (DeleteRolePermissionsBoundary answered 501 at the time it was filed; in THIS checkout the operation answers and refuses, so Cause 2 below is a different defect on that same operation and that issue's replay key no longer reproduces), 20261027T051038Z-iam-7-net-new-behavioral-root-causes-delete.md (its Cause 3 is the same AddRoleToInstanceProfile quota-name constant but reached only through iam.instance-profile-single-role-constraint; Cause 3 below adds the second replay key it does not carry, and the two should be closed together), 20261027T000203Z-iam-4-net-new-root-causes-createpolicyversion-s.md (its Cause 2 covered the generic "Syntax errors in policy." text reached through CreatePolicyVersion in this same check; that leg now PASSES and the failure has moved to a DIFFERENT message on a different operation — see Cause 4), 20261026T041300Z-cloudformation-iam-sts-a-stack-whose-aws-iam.md (an AWS::IAM::ManagedPolicy with a statement lacking Resource reaching CREATE_COMPLETE — a different resource type and a different missing element than Cause 5), and 20261027T000203Z-iam-s3-s3-getobject-performs-no-iam-authorization-a-principal.md (the same missing-authorization defect on S3 GetObject, not on DeleteObject — see Cause 6).

Cause 1 of 6: EnableMFADevice is an unimplemented scaffold stub, so it answers an unmodeled 501 NotImplemented instead of refusing an unregistered serial number.
AWS: Smithy iam#EnableMFADevice takes UserName (required, existingUserNameType, @length 1-128, pattern ^[\w+=,.@-]+$), SerialNumber (required, serialNumberType, @length 9-256), AuthenticationCode1 and AuthenticationCode2 (both required, @length 6-6, pattern ^[\d]+$), and models NoSuchEntityException (awsQueryError NoSuchEntity, httpResponseCode 404), EntityTemporarilyUnmodifiableException (EntityTemporarilyUnmodifiable, 409), InvalidAuthenticationCodeException (InvalidAuthenticationCode, 403), EntityAlreadyExistsException, ConcurrentModificationException, LimitExceededException and ServiceFailureException. Enabling a serial number that was never registered for the user is refused NoSuchEntity 404 with the message "MFA Device with serial number <serialNumber> does not exist." — fixed text "MFA Device with serial number " + " does not exist.", the one variable part being the SerialNumber taken verbatim from the request. (Against a user that was just deleted and recreated, EntityTemporarilyUnmodifiable 409 is the alternative legitimate answer; the check accepts either.)
Emulator: the handler is the generated scaffold — services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/enable_mfa_device.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("EnableMFADevice"))`. Every call answers code NotImplemented HTTP 501 with "operation EnableMFADevice is scaffolded but not implemented in this emulator". Note the replay report's own stub detector did NOT flag this one, because this check catches the ClientError and re-raises it as an assertion, so no auto-filed stub issue exists for the operation.
Why: 501 NotImplemented is not an IAM error code and is not in the operation's modeled error set, so no SDK error shape matches it; a client cannot tell "this serial is not registered" from "this user is in its post-recreate cooldown" from "the service is broken", and MFA enrolment cannot be exercised at all.
Fix: implement the handler in services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/enable_mfa_device.rs against the service state's MFA-device store (resolve the user, then the serial; refuse NoSuchEntity with AWS's message when the serial is not registered to that user) and delete the SCAFFOLDED marker line.
Traces: iam.entity-temporarily-unmodifiable-enforced.

Cause 2 of 6: DeleteRolePermissionsBoundary's NoSuchEntity refusal is emulator prose that names the role, where AWS names the role's principal id.
AWS: Smithy iam#DeleteRolePermissionsBoundary takes RoleName (required, roleNameType) and models NoSuchEntityException (awsQueryError NoSuchEntity, httpResponseCode 404) and UnmodifiableEntityException (UnmodifiableEntity, 400). Recorded behavior for an ordinary role that has no permissions boundary set: NoSuchEntity 404 with the message "Principal <roleId> does not have scope" — fixed text "Principal " + " does not have scope" (no terminating period), the one variable part being the role's RoleId from state, i.e. the AROA-prefixed unique id CreateRole returned, NOT the role name. The error code the emulator returns is already correct; only the message diverges.
Emulator: the same call is refused NoSuchEntity with "No permissions boundary is set for the role with name <roleName>." — a different template that substitutes the request's RoleName for AWS's principal id and adds a terminating period.
Why: a client that matches IAM's boundary-absent sentence, or that extracts the principal id from it to correlate the refusal with the role it read from CreateRole/GetRole, sees text AWS never emits; keying on the role name instead of the unique id also breaks for a deleted-and-recreated role, where AWS's id changes and the name does not.
Fix: the NoSuchEntity message rendering in the DeleteRolePermissionsBoundary handler's boundary lookup, which must emit AWS's template filled with the resolved role's RoleId. (Supersedes the stale 501 diagnosis in 20261026T051836Z-iam-deleterolepermissionsboundary-is-an-unimplemented-501-scaffold-stub.md; the operation now answers. The service-linked-role UnmodifiableEntity leg of the same check is downstream of this assertion and was never reached, so it is not yet graded.)
Traces: iam.delete-permissions-boundary-service-linked-role-rejected.

Cause 3 of 6: AddRoleToInstanceProfile's LimitExceeded refusal names the wrong quota (second replay key for a cause already filed from one other check).
AWS: Smithy iam#AddRoleToInstanceProfile takes InstanceProfileName and RoleName (both required) and models LimitExceededException (awsQueryError LimitExceeded, httpResponseCode 409). An instance profile holds exactly one role, and adding a second is refused with exactly "Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1" — fixed text "Cannot exceed quota for InstanceSessionsPerInstanceProfile: " plus the quota value (1). The quota NAME is the contract-bearing part: it is what identifies which of the operation's several LimitExceeded causes fired.
Emulator: the right code and the right limit value, but a different quota name: "Cannot exceed quota for RolesPerInstanceProfile: 1". The surrounding state assertions pass — the rejected call does not add the second role, and freeing the slot lets a different role be added.
Why: the quota name is the only machine-readable discriminator in a LimitExceeded body; tooling that maps IAM quota names onto Service Quotas entries or onto remediation cannot resolve an invented name.
Fix: the quota-name constant used by the single-role check in the AddRoleToInstanceProfile handler. Same constant as Cause 3 of 20261027T051038Z-iam-7-net-new-behavioral-root-causes-delete.md; this cause supplies the additional failing check id that issue does not carry, so re-replay BOTH ids after the fix.
Traces: iam.limit-exceeded-on-add-and-attach-operations.

Cause 4 of 6: UpdateAssumeRolePolicy with unparseable JSON returns the generic policy-syntax message instead of IAM's distinct trust-policy message.
AWS: Smithy iam#UpdateAssumeRolePolicy takes RoleName (required) and PolicyDocument (required, policyDocumentType @length 1-131072) and models MalformedPolicyDocumentException (awsQueryError MalformedPolicyDocument, httpResponseCode 400). IAM does NOT reuse one syntax message across all policy-document operations: for a trust document that is not parseable JSON, UpdateAssumeRolePolicy is refused MalformedPolicyDocument 400 with exactly "This policy contains invalid Json" — the whole message is fixed text, with no variable part and no terminating period — whereas CreatePolicyVersion and Put{Group,Role,User}Policy answer the different fixed sentence "Syntax errors in policy." for the same input class.
Emulator: UpdateAssumeRolePolicy(PolicyDocument='{not valid json') is refused with the correct code MalformedPolicyDocument but the message "Syntax errors in policy." — the generic constant, applied to an operation AWS words differently. Everything else in the check now passes: the CreatePolicyVersion, PutGroupPolicy, PutRolePolicy and PutUserPolicy legs each return "Syntax errors in policy." as AWS does, nothing malformed persists, and the rejected UpdateAssumeRolePolicy correctly leaves the stored trust document untouched.
Why: the message is the only thing distinguishing a malformed TRUST policy from a malformed PERMISSION policy in IAM's responses (the code is the same for both); a client or IaC engine that branches on IAM's trust-policy wording to point an operator at the AssumeRolePolicyDocument cannot classify the refusal.
Fix: the MalformedPolicyDocument message for the trust-document parse failure in the UpdateAssumeRolePolicy handler (and the same branch in CreateRole) must emit IAM's trust-policy sentence rather than the shared policy-document validator's generic one. Distinct from Cause 2 of 20261027T000203Z-iam-4-net-new-root-causes-createpolicyversion-s.md, which concerned the generic sentence itself and whose leg of this check now passes.
Traces: iam.malformed-policy-document-rejected.

Cause 5 of 6: CreateRole accepts an AssumeRolePolicyDocument whose statement has no Effect, so a CloudFormation stack declaring that role reaches CREATE_COMPLETE instead of failing.
AWS: Smithy iam#CreateRole takes AssumeRolePolicyDocument (required, policyDocumentType) and models MalformedPolicyDocumentException (awsQueryError MalformedPolicyDocument, httpResponseCode 400). Effect is a required element of every policy statement, so a trust document whose only statement carries Principal and Action but no Effect is refused MalformedPolicyDocument 400 and the role is not created. Through CloudFormation that rejection propagates: the AWS::IAM::Role resource goes CREATE_FAILED with a ResourceStatusReason that names the missing field ("Missing required field Effect") and attributes the failure to the Iam service, the stack ends ROLLBACK_COMPLETE (or CREATE_FAILED), and GetRole on the declared RoleName answers NoSuchEntity 404 with "The role with name <roleName> cannot be found."
Emulator: the stack is created with that exact template and reaches StackStatus 'CREATE_COMPLETE'. No validation refuses the Effect-less statement, so the role is created from an invalid trust document and nothing is rolled back. The positive control (an otherwise-identical stack whose statement has Effect: Allow) also deploys, so the divergence is purely the absent validation, not CFN provisioning.
Why: an invalid trust policy is the canonical IaC typo, and AWS's contract is that it fails the deploy. Here it silently succeeds and leaves behind a role whose trust document no policy evaluator can interpret — every downstream AssumeRole decision against it is unspecified, and a template that real AWS refuses is reported as deployed, so CI gates built on a failing stack never fire.
Fix: the AssumeRolePolicyDocument validation in the CreateRole handler (the same policy-document validator path as Cause 4) must require Effect in every statement and return the modeled MalformedPolicyDocumentException, so the CloudFormation AWS::IAM::Role resource handler surfaces a CREATE_FAILED whose reason names the missing field and attributes it to Iam.
Traces: iam.deploy-iam-role-cloudformation-malformed-trust-policy-rejected.

Cause 6 of 6: S3 DeleteObject performs no IAM authorization, so an assumed session whose only grant is s3:GetObject can delete the object — and the emulator's own simulator contradicts its enforcement.
AWS: s3:DeleteObject by a principal that appears in no Allow statement is refused AccessDenied with HTTP 403 and the message "User: <assumed-role session ARN> is not authorized to perform: s3:DeleteObject on resource: \"<object ARN>\" because no identity-based policy allows the s3:DeleteObject action" — fixed text around three variable parts taken from the request and the caller's identity: the assumed-role session ARN (arn:aws:sts::<account>:assumed-role/<roleName>/<sessionName>), the quoted object ARN, and the action name repeated in the trailing clause. iam#SimulatePrincipalPolicy's EvalDecision for that action is implicitDeny, and the simulator's advisory verdict and the real signed call must agree.
Emulator: with a CloudFormation-deployed AWS::IAM::ManagedPolicy whose only statement is Allow s3:GetObject on arn:aws:s3:::<bucket>/* attached to the role via Roles, SimulatePrincipalPolicy correctly answers allowed for s3:GetObject and implicitDeny for s3:DeleteObject, and the assumed session's signed GetObject correctly succeeds — but the assumed session's signed DeleteObject SUCCEEDS with HTTP 204 and deletes the object. No refusal is produced at all, so the emulator enforces strictly less than it simulates.
Why: this is the over-permissive class: a least-privilege policy that grants read and withholds delete is accepted, readable, and correctly simulated, yet the withheld action still executes and destroys data. Any test or control that proves "this role cannot delete" passes against the emulator and is false on AWS, and the contradiction between SimulatePrincipalPolicy and the data plane means neither view can be trusted.
Fix: route the S3 object-mutation handlers through the same request-authorizer gate the read path is expected to use, evaluating the caller's identity-based policies (including managed policies attached to an assumed role) and refusing with S3's AccessDenied template above when no statement allows the action. Same missing gate as 20261027T000203Z-iam-s3-s3-getobject-performs-no-iam-authorization-a-principal.md reports for GetObject; DeleteObject is a separate handler and is not covered by that issue's replay key.
Traces: iam.deploy-iam-managed-policy-via-cloudformation-attaches-and-enforces.

## Evidence
Suite simparity-behavioral-ga-prod-v1, replayed against this checkout. Service token `iam` is bootstrapped. Overall run: 178/403 passed; the six checks below are the only `iam`-token failures in the report that are neither ALREADY FILED nor OWNED.

Cause 1 — iam.entity-temporarily-unmodifiable-enforced [mismatch], check line 103 via _assert_one_of at line 31:
  AssertionError: EnableMFADevice: expected one of ['EntityTemporarilyUnmodifiable', 'NoSuchEntity'], got 'NotImplemented'
  Request: EnableMFADevice(UserName=<a user deleted and immediately recreated>, SerialNumber=<a freshly generated, never-registered serial>, AuthenticationCode1='123456', AuthenticationCode2='654321').
  Expected: NoSuchEntity / HTTP 404 with "MFA Device with serial number <serialNumber> does not exist." (fixed text + the request's SerialNumber), or EntityTemporarilyUnmodifiable / HTTP 409 if the recreate cooldown is still in effect.
  Actual: NotImplemented / HTTP 501, 'operation EnableMFADevice is scaffolded but not implemented in this emulator'. Confirmed in source: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/enable_mfa_device.rs line 4 still carries '//! SCAFFOLDED, NOT IMPLEMENTED' and line 212 is `Err(crate::scaffolded::Scaffolded::not_implemented("EnableMFADevice"))`.
  Legs that passed before this one: UpdateUser applied the new path and GetUser read it back; UpdateLoginProfile and DeleteLoginProfile both answered NoSuchEntity with the exact 'Login Profile for User <user> cannot be found.' text. DeactivateMFADevice and ChangePassword are downstream of the failing assertion and were not reached.

Cause 2 — iam.delete-permissions-boundary-service-linked-role-rejected [mismatch], check line 53:
  AssertionError: expected message 'Principal AROAOWVOJOY3VBWOJCSHY does not have scope', got 'No permissions boundary is set for the role with name sim2real-behav-role-69a162fba17b.'
  The code assertion (NoSuchEntity, iam#DeleteRolePermissionsBoundary's modeled NoSuchEntityException, awsQueryError NoSuchEntity / HTTP 404) passed at line 48; only the message diverges, so the operation is NOT a 501 stub in this checkout.
  Request: DeleteRolePermissionsBoundary(RoleName=<an ordinary role created by this check, with no boundary ever set>).
  Expected message: fixed text 'Principal ' + ' does not have scope', single variable part = the role's RoleId from state (the AROA… value CreateRole returned for this run; AROAOWVOJOY3VBWOJCSHY and sim2real-behav-role-69a162fba17b are this run's generated values, not part of either template).

Cause 3 — iam.limit-exceeded-on-add-and-attach-operations [mismatch], check line 171:
  AssertionError: expected the exact LimitExceeded message AWS returns, got 'Cannot exceed quota for RolesPerInstanceProfile: 1'
  Sequence: CreateRole x2, CreateInstanceProfile, AddRoleToInstanceProfile(role1) succeeds, AddRoleToInstanceProfile(role2) is refused.
  The code assertion (LimitExceeded / HTTP 409) passed at line 167. Expected: 'Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1' — fixed text 'Cannot exceed quota for InstanceSessionsPerInstanceProfile: ' plus the quota value 1. Only the quota NAME diverges.

Cause 4 — iam.malformed-policy-document-rejected [mismatch], check line 184:
  AssertionError: expected message 'This policy contains invalid Json', got 'Syntax errors in policy.'
  Request: UpdateAssumeRolePolicy(RoleName=<an existing role>, PolicyDocument='{not valid json').
  The code assertion (MalformedPolicyDocument / HTTP 400) passed at line 181, and the subsequent state assertion confirms the stored trust document was unchanged.
  Legs that now PASS in this same check (so they are not part of this cause): CreatePolicyVersion, PutGroupPolicy, PutRolePolicy and PutUserPolicy each answered MalformedPolicyDocument with exactly 'Syntax errors in policy.' for the same '{not valid json' input, and nothing malformed persisted. This is the leg the prior issue 20261027T000203Z's Issue-Key named (code_cpv_message at run#14); the failure has moved to code_uarp_message at run#184.

Cause 5 — iam.deploy-iam-role-cloudformation-malformed-trust-policy-rejected [mismatch], check line 55:
  AssertionError: expected ROLLBACK_COMPLETE or CREATE_FAILED for a malformed trust policy, got 'CREATE_COMPLETE'
  Request: CreateStack(Capabilities=['CAPABILITY_NAMED_IAM']) with one resource, AWS::IAM::Role, whose AssumeRolePolicyDocument is {"Version":"2012-10-17","Statement":[{"Principal":{"Service":"lambda.amazonaws.com"},"Action":["sts:AssumeRole"]}]} — a statement with NO Effect.
  Expected: the stack reaches ROLLBACK_COMPLETE or CREATE_FAILED; the TestRole resource has a CREATE_FAILED event whose ResourceStatusReason contains 'Missing required field Effect' and attributes the failure to 'Iam'; GetRole(<declared role name>) answers NoSuchEntity with 'The role with name <roleName> cannot be found.'
  Actual: StackStatus 'CREATE_COMPLETE' within the 180 s terminal-state poll. The positive control (the identical template with Effect: Allow) deploys and GetRole returns the role, so CFN provisioning of AWS::IAM::Role works; the missing piece is CreateRole's refusal.

Cause 6 — iam.deploy-iam-managed-policy-via-cloudformation-attaches-and-enforces [mismatch], check line 148 via _error at line 231:
  AssertionError: expected the call to be refused, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 204, ...}}
  Request: DeleteObject(Bucket=<bucket>, Key='testobj') signed with credentials from AssumeRole on a role whose only attached policy (a CFN AWS::IAM::ManagedPolicy with Roles: [Ref TestRole]) allows exactly {"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::<bucket>/*"}.
  Controls that passed first: the stack reached CREATE_COMPLETE and its outputs resolved; GetPolicy returned the managed policy; SimulatePrincipalPolicy answered EvalDecision 'allowed' for s3:GetObject and 'implicitDeny' for s3:DeleteObject on the object ARN; the assumed session's real signed GetObject returned the written body.
  Expected: AccessDenied / HTTP 403 with 'User: arn:aws:sts::<account>:assumed-role/<roleName>/<sessionName> is not authorized to perform: s3:DeleteObject on resource: "<object ARN>" because no identity-based policy allows the s3:DeleteObject action' — fixed text with the session ARN, the quoted object ARN and the action as its variable parts.
  Actual: HTTP 204, the delete is performed. The emulator's simulator said implicitDeny for the same (principal, action, resource) triple, so simulation and enforcement disagree.

## Steps to Reproduce
Boot the emulator from this checkout and point an `iam` client at it, signing as a principal permitted to call IAM.

Cause 1:
1. CreateUser U; DeleteUser U; CreateUser U again.
2. EnableMFADevice(UserName=U, SerialNumber=<any 9+ char serial that was never registered>, AuthenticationCode1='123456', AuthenticationCode2='654321').
3. Observe NotImplemented / HTTP 501 instead of NoSuchEntity / 404 with 'MFA Device with serial number <serial> does not exist.' (or EntityTemporarilyUnmodifiable / 409).
   Replay: iam.entity-temporarily-unmodifiable-enforced

Cause 2:
1. CreateRole R with any valid trust policy and note the RoleId (AROA…) from the response. Do NOT set a permissions boundary.
2. DeleteRolePermissionsBoundary(RoleName=R).
3. Observe NoSuchEntity / 404 (correct) with 'No permissions boundary is set for the role with name R.' instead of 'Principal <RoleId> does not have scope'.
   Replay: iam.delete-permissions-boundary-service-linked-role-rejected

Cause 3:
1. CreateInstanceProfile P; CreateRole R1 and R2; AddRoleToInstanceProfile(P, R1) (succeeds).
2. AddRoleToInstanceProfile(P, R2).
3. Observe LimitExceeded / 409 (correct) with 'Cannot exceed quota for RolesPerInstanceProfile: 1' instead of 'Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1'.
   Replay: iam.limit-exceeded-on-add-and-attach-operations

Cause 4:
1. CreateRole R with a valid trust policy.
2. UpdateAssumeRolePolicy(RoleName=R, PolicyDocument='{not valid json').
3. Observe MalformedPolicyDocument / 400 (correct) with 'Syntax errors in policy.' instead of 'This policy contains invalid Json'. For contrast, PutRolePolicy(RoleName=R, PolicyName=<any>, PolicyDocument='{not valid json') correctly returns 'Syntax errors in policy.'
   Replay: iam.malformed-policy-document-rejected

Cause 5:
1. CreateStack with Capabilities=['CAPABILITY_NAMED_IAM'] and a single AWS::IAM::Role resource whose AssumeRolePolicyDocument is {"Version":"2012-10-17","Statement":[{"Principal":{"Service":"lambda.amazonaws.com"},"Action":["sts:AssumeRole"]}]} (no Effect) and an explicit RoleName.
2. Poll DescribeStacks until terminal — observe 'CREATE_COMPLETE' instead of ROLLBACK_COMPLETE/CREATE_FAILED; DescribeStackEvents has no CREATE_FAILED event for the role naming 'Missing required field Effect'; GetRole(<RoleName>) succeeds instead of answering NoSuchEntity.
3. Equivalently, call CreateRole directly with that AssumeRolePolicyDocument and observe it accepted instead of MalformedPolicyDocument / 400.
   Replay: iam.deploy-iam-role-cloudformation-malformed-trust-policy-rejected

Cause 6:
1. CreateBucket B and PutObject B/testobj.
2. CreateStack with Capabilities=['CAPABILITY_NAMED_IAM'] declaring AWS::IAM::Role (trusted by arn:aws:iam::<account>:root) and AWS::IAM::ManagedPolicy whose PolicyDocument is a single Allow on s3:GetObject for arn:aws:s3:::B/* and whose Roles is [{"Ref":"TestRole"}].
3. SimulatePrincipalPolicy(PolicySourceArn=<role ARN>, ActionNames=['s3:DeleteObject'], ResourceArns=['arn:aws:s3:::B/testobj']) — note EvalDecision 'implicitDeny'.
4. AssumeRole the role and, with those credentials, call GetObject (succeeds — correct) then DeleteObject on B/testobj.
5. Observe HTTP 204 and the object gone, instead of AccessDenied / 403 with 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: s3:DeleteObject on resource: "arn:aws:s3:::B/testobj" because no identity-based policy allows the s3:DeleteObject action'.
   Replay: iam.deploy-iam-managed-policy-via-cloudformation-attaches-and-enforces
