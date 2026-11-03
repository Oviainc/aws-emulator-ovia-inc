Issue-Source: trace-sweep-divergence
Issue-Trace: iam-err-delete-policy-blocked-by-versions
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-err-delete-policy-blocked-by-versions iam-err-delete-role-blocked-by-dependencies iam-err-delete-user-blocked-by-policies iam-role-lifecycle-inline-managed-policy-and-boundary iam-credential-conflict-and-malformed-errors iam-signing-certificate-error-paths iam-signing-certificate-malformed iam-signing-certificate-upload-and-status-update-errors iam-server-certificate-error-paths iam-server-certificate-lifecycle iam-cred-signing-cert-duplicate-across-users iam-managed-policy-lifecycle iam-permissions-boundary-and-policy-simulation
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|DeletePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|DeleteRole|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|DeleteUser|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|DeleteRole|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|UploadSigningCertificate|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|UploadServerCertificate|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|UploadServerCertificate|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListPoliciesGrantingServiceAccess|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|PutRolePermissionsBoundary|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|PutRolePermissionsBoundary|4xx->4xx|$.ErrorResponse.RequestId
Issue-Related: 20261029T184359Z-iam-error-responses-omit-the-responsemetadata-requestid-element.md
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# IAM conflict and certificate refusal messages use emulator prose

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePolicy

## Description
Eight refusal paths answer with the right status and Code but a message the emulator words itself. Each message below is entirely fixed text unless noted; nothing from the request is interpolated.

Cause 1 of 8 - DeletePolicy blocked by non-default versions. AWS: 409 DeleteConflict with "This policy has more than one version. Before you delete a policy, you must delete the policy's versions. The default version is deleted with the policy." Emulator: 'Cannot delete a policy with non-default versions. Delete the non-default versions first.' Fix: the DeletePolicy dependency-check message.
Traces: iam-err-delete-policy-blocked-by-versions#task#3.

Cause 2 of 8 - DeleteRole blocked by an instance profile. AWS: 409 with 'Cannot delete entity, must remove roles from instance profile first.' Emulator: it reports the policy dependency instead - 'Cannot delete entity, must delete policies first.' - so the caller is told to remove the wrong dependency. Fix: the DeleteRole handler must check and report the instance-profile dependency distinctly.
Traces: iam-err-delete-role-blocked-by-dependencies#task#5.

Cause 3 of 8 - DeleteUser/DeleteRole blocked by attached managed policies. AWS: 'Cannot delete entity, must detach all policies first.' Emulator: 'Cannot delete entity, must delete policies first.' - 'detach all' vs 'delete', which names the wrong remedy for a managed policy. Fix: distinguish attached (detach) from inline (delete) in the delete-conflict messages.
Traces: iam-err-delete-user-blocked-by-policies#task#5; iam-role-lifecycle-inline-managed-policy-and-boundary#task#16.

Cause 4 of 8 - UploadSigningCertificate with an unparseable certificate. AWS: 400 with 'Certificate <certificateBody> is malformed.' - the submitted CertificateBody value is substituted. Emulator: 'Invalid CertificateBody: the value must be one PEM-encoded X.509 certificate'. Fix: the signing-certificate parse-failure message.
Traces: iam-credential-conflict-and-malformed-errors#task#6.

Cause 5 of 8 - UploadServerCertificate with an unparseable certificate. AWS: 400 with the fixed text 'Unable to parse certificate. Please ensure the certificate is in PEM format.' Emulator: the same 'Invalid CertificateBody: ...' prose as cause 4 - the two operations word this differently in AWS and must not share one message. Fix: the server-certificate parse-failure message.
Traces: iam-credential-conflict-and-malformed-errors#task#7; iam-server-certificate-lifecycle#task#12.

Cause 6 of 8 - UploadSigningCertificate with a duplicate certificate. AWS: 409 with the fixed text 'Duplicate certificate uploaded.' Emulator: 'The Certificate with id <certificateId> already exists.' - it echoes a generated id AWS does not mention. Fix: the duplicate-certificate message.
Traces: iam-cred-signing-cert-duplicate-across-users#task#6.

Cause 7 of 8 - ListPoliciesGrantingServiceAccess with a policy ARN. AWS: 400 with 'Invalid Entity Arn: <arn> is not supported.' - the submitted Arn is substituted. Emulator: 'ARN <arn> is not valid: Arn must be the ARN of an IAM user, group or role'. Fix: the entity-ARN rejection message on this operation.
Traces: iam-managed-policy-lifecycle#task#15.

Cause 8 of 8 - PutRolePermissionsBoundary with an AWS-reserved policy. AWS: 400 with the fixed text 'Cannot attach AWS reserved policy to an IAM role.' Emulator: 'Policy <policyArn> is not attachable.' - which reports the wrong rule (attachability rather than reservation). Fix: the reserved-policy guard's message in the boundary handler.
Traces: iam-permissions-boundary-and-policy-simulation#task#16.

Note: open issue 20261027T051038Z-iam-7-net-new-behavioral-root-causes-delete describes delete-conflict/quota/policy-syntax refusal prose behaviorally; it carries no replay key for any of these fixture steps.

## Evidence
Cause 1: expected "This policy has more than one version. Before you delete a policy, you must delete the policy's versions. The default version is deleted with the policy."; actual 'Cannot delete a policy with non-default versions. Delete the non-default versions first.' (both HTTP 409).
Cause 2: expected 'Cannot delete entity, must remove roles from instance profile first.'; actual 'Cannot delete entity, must delete policies first.'
Cause 3: expected 'Cannot delete entity, must detach all policies first.'; actual 'Cannot delete entity, must delete policies first.'
Cause 4: expected 'Certificate not-a-valid-x509-certificate is malformed.'; actual 'Invalid CertificateBody: the value must be one PEM-encoded X.509 certificate'.
Cause 5: expected 'Unable to parse certificate. Please ensure the certificate is in PEM format.'; actual 'Invalid CertificateBody: the value must be one PEM-encoded X.509 certificate'.
Cause 6: expected 'Duplicate certificate uploaded.'; actual 'The Certificate with id <certificateId> already exists.'
Cause 7: expected 'Invalid Entity Arn: <arn> is not supported.'; actual 'ARN <arn> is not valid: Arn must be the ARN of an IAM user, group or role'.
Cause 8: expected 'Cannot attach AWS reserved policy to an IAM role.'; actual 'Policy <policyArn> is not attachable.'
In all eight the HTTP status and Error/Code match the recording; only Message differs.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-err-delete-policy-blocked-by-versions#task#3 and diff only Error/Message against the recording.
3. Replay each binding in trace_ids and do the same: the status and Code match, the Message does not.
