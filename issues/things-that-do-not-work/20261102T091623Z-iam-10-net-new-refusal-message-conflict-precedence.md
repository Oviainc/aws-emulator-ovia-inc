Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.deny-notaction-complement-enforced-on-a-real-signed-call iam.explicit-deny-blocks-a-real-signed-call-and-is-scoped iam.group-policy-explicit-deny-overrides-the-users-own-allow iam.get-account-authorization-details iam.get-account-properties iam.iam-eval-deep-iam-1-denial-reason-messages-are-distinct iam.list-attached-role-policies-denied-without-permission iam.sep-policy-composition-principal-tag-condition-simulated-with-real-tags-denied-on-mismatch iam.sep-policy-composition-stringnotequals-missing-sourcevpce-key-deny-applies-denied-by-simulator-and-real-call iam.sep-policy-composition-group-managed-deny-overrides-user-allows-denied-by-simulator-and-real-call iam.non-attachable-policy-boundary-rejected iam.permissions-boundary-non-attachable-policy-rejected iam.put-role-permissions-boundary-non-attachable-rejected iam.put-role-permissions-boundary-with-service-role-policy-rejected iam.put-user-permissions-boundary-non-attachable-rejected iam.asgssh-role-in-instance-profile-delete-rejected iam.instance-profile-add-nonexistent-role-rejected iam.instance-profile-re-adding-its-own-role-rejected iam.get-aws-managed-policy-metadata-and-default-version iam.server-certificate-body-chain-and-key-match iam.service-specific-credential-quota-race iam.sep-policy-composition-securetransport-condition-denied-when-policy-requires-plaintext
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.deny-notaction-complement-enforced-on-a-real-signed-call|run#18|AssertionError|assert denied_msg == _expected_denial(denied_msg, (
Issue-Key: iam.explicit-deny-blocks-a-real-signed-call-and-is-scoped|run#21|AssertionError|assert denied_msg == _expected_denial(denied_msg, (
Issue-Key: iam.group-policy-explicit-deny-overrides-the-users-own-allow|run#26|AssertionError|assert denied_msg == _expected_denial(denied_msg, (
Issue-Key: iam.get-account-authorization-details|run#15|AssertionError|assert message == _no_policy_denial(unpriv_arn, "GetAccountAuthorizationDetails", message), (
Issue-Key: iam.get-account-properties|run#9|AssertionError|assert message == _no_policy_denial(unpriv_arn, "GetAccountProperties", message), (
Issue-Key: iam.iam-eval-deep-iam-1-denial-reason-messages-are-distinct|run#32|AssertionError|assert message == expected_message, (
Issue-Key: iam.list-attached-role-policies-denied-without-permission|run#19|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.sep-policy-composition-principal-tag-condition-simulated-with-real-tags-denied-on-mismatch|run#24|AssertionError|assert denied_message == expected_message, (
Issue-Key: iam.sep-policy-composition-stringnotequals-missing-sourcevpce-key-deny-applies-denied-by-simulator-and-real-call|run#23|AssertionError|assert denied_msg == expected_msg, f"expected message {expected_msg!r}, got {denied_msg!r}"
Issue-Key: iam.sep-policy-composition-group-managed-deny-overrides-user-allows-denied-by-simulator-and-real-call|run#31|AssertionError|assert denied_message == expected_message, (
Issue-Key: iam.non-attachable-policy-boundary-rejected|run#7|AssertionError|assert message_role == "Cannot attach AWS reserved policy to an IAM role.", message_role
Issue-Key: iam.permissions-boundary-non-attachable-policy-rejected|run#9|AssertionError|assert message == "Cannot attach AWS reserved policy to an IAM role.", message
Issue-Key: iam.put-role-permissions-boundary-non-attachable-rejected|run#11|AssertionError|assert message == "Cannot attach AWS reserved policy to an IAM role.", message
Issue-Key: iam.put-role-permissions-boundary-with-service-role-policy-rejected|run#13|AssertionError|assert message == "Cannot attach AWS reserved policy to an IAM role.", message
Issue-Key: iam.put-user-permissions-boundary-non-attachable-rejected|run#11|AssertionError|assert message == "Cannot attach AWS reserved policy to an IAM user.", message
Issue-Key: iam.asgssh-role-in-instance-profile-delete-rejected|run#7|AssertionError|assert (code, message) == ("DeleteConflict", _IN_PROFILE), (
Issue-Key: iam.instance-profile-add-nonexistent-role-rejected|run#8|AssertionError|assert got == want, f"adding a role that does not exist: expected {want!r}, got {got!r}"
Issue-Key: iam.instance-profile-re-adding-its-own-role-rejected|run#9|AssertionError|assert got == ("LimitExceeded", _ONE_ROLE), (
Issue-Key: iam.get-aws-managed-policy-metadata-and-default-version|run#3|AssertionError|assert re.fullmatch(r"ANPA[A-Z0-9]{17}", policy["PolicyId"]), (
Issue-Key: iam.server-certificate-body-chain-and-key-match|run#16|AssertionError|assert got["CertificateBody"] == body_pem.rstrip(), (
Issue-Key: iam.service-specific-credential-quota-race|run#16|AssertionError|assert all(message == expected for message in messages), (
Issue-Key: iam.sep-policy-composition-securetransport-condition-denied-when-policy-requires-plaintext|_settle#24|Timeout|time.sleep(interval_s)
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# iam: 10 net-new refusal-message, conflict-precedence and condition-evaluation divergences

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetUser

## Description
This issue carries 10 root causes in IAM. None is covered by the open IAM issues (20261025T000319Z-iam-10-net-new-behavioral-root-causes-four.md, 20261029T192356Z-iam-10-net-new-refusal-message-and-response.md, 20261029T192356Z-iam-10-net-new-validation-and-accept-reject.md, 20261029T192356Z-iam-9-net-new-authorization-and-policy-evaluation.md, 20261025T102129Z-iam-10-net-new-behavioral-root-causes-simulateprincipalpolicy.md): those carry other checks' replay keys. All ten must be closed before submitting.

Cause 1 of 10: every IAM AccessDenied message is missing the authorization-details pointer AWS appends.
AWS: recorded behavior only (the model types Message as a plain string). An IAM AccessDenied message is a head followed by a fixed pointer sentence. Head, implicit deny: 'User: <caller-arn> is not authorized to perform: <service:Action> on resource: <resource> because no identity-based policy allows the <service:Action> action'. Head, explicit deny: 'User: <caller-arn> is not authorized to perform: <service:Action> on resource: <resource> with an explicit deny in an identity-based policy'. Pointer, appended to both: '. Go to https://<region>.console.aws.amazon.com/iam/home?region=<region>#/authorization-details/<authorization-id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <authorization-id>'. Everything there is fixed text except <caller-arn>, <service:Action> and <resource> (from the request), <region> (the request's region) and <authorization-id> (minted per request and repeated verbatim in both places).
Emulator: emits the head only; the pointer sentence is entirely absent.
Why: a client or operator that pins AWS's refusal text, or that follows the pointer to GetRequestAuthorizationDetails, gets a message AWS never returns and loses the only handle on the denial's details.
Fix: the IAM denial serializer that composes the AccessDenied message, for both the implicit- and explicit-deny heads.
Traces: iam.deny-notaction-complement-enforced-on-a-real-signed-call, iam.explicit-deny-blocks-a-real-signed-call-and-is-scoped, iam.group-policy-explicit-deny-overrides-the-users-own-allow, iam.get-account-authorization-details, iam.get-account-properties, iam.iam-eval-deep-iam-1-denial-reason-messages-are-distinct, iam.list-attached-role-policies-denied-without-permission, iam.sep-policy-composition-principal-tag-condition-simulated-with-real-tags-denied-on-mismatch, iam.sep-policy-composition-stringnotequals-missing-sourcevpce-key-deny-applies-denied-by-simulator-and-real-call.

Cause 2 of 10: an explicit deny that comes from a MANAGED policy does not name that policy.
AWS: recorded behavior only. When the deny statement lives in a managed policy, the head ends '... with an explicit deny in an identity-based policy: <arn of the deny policy>' — fixed text 'with an explicit deny in an identity-based policy: ' plus the policy ARN taken from state — and then the pointer of cause 1. An inline-policy deny carries no ARN.
Emulator: stops at '... with an explicit deny in an identity-based policy', with no ARN, so a managed deny is indistinguishable from an inline one.
Why: an operator cannot tell which attached policy refused the call.
Fix: the denial serializer must carry the matched deny statement's source policy ARN into the message.
Traces: iam.sep-policy-composition-group-managed-deny-overrides-user-allows-denied-by-simulator-and-real-call.

Cause 3 of 10: refusing an AWS reserved (service-role) policy uses the wrong message.
AWS: attaching a policy under the /aws-service-role/ path as a permissions boundary or to an entity is refused PolicyNotAttachable (Smithy: PutRolePermissionsBoundary declares PolicyNotAttachableException, awsQueryError code PolicyNotAttachable, HTTP 400) with the message 'Cannot attach AWS reserved policy to an IAM role.' — or '... to an IAM user.' for a user target. The whole message is fixed text; only the entity word varies, and no ARN appears in it.
Emulator: 'Policy <policy-arn> is not attachable.'
Why: a client that matches AWS's wording cannot recognize the refusal, and the message leaks a value AWS does not put there.
Fix: the validation shared by PutRolePermissionsBoundary, PutUserPermissionsBoundary and the attach handlers.
Traces: iam.non-attachable-policy-boundary-rejected, iam.permissions-boundary-non-attachable-policy-rejected, iam.put-role-permissions-boundary-non-attachable-rejected, iam.put-role-permissions-boundary-with-service-role-policy-rejected, iam.put-user-permissions-boundary-non-attachable-rejected.

Cause 4 of 10: DeleteRole reports the wrong conflict first.
AWS: DeleteRole on a role that is BOTH a member of an instance profile and carrying attached policies answers DeleteConflict (Smithy: DeleteRole declares DeleteConflictException, awsQueryError code DeleteConflict, HTTP 409) with 'Cannot delete entity, must remove roles from instance profile first.' — instance-profile membership is checked before attached policies. The message is entirely fixed text.
Emulator: answers DeleteConflict with 'Cannot delete entity, must detach all policies first.'
Why: a cleanup routine that reads the message to decide its next step detaches policies and retries forever instead of removing the role from its instance profile.
Fix: the DeleteRole handler's conflict-precedence ordering.
Traces: iam.asgssh-role-in-instance-profile-delete-rejected.

Cause 5 of 10: AddRoleToInstanceProfile's NoSuchEntity message for a missing role.
AWS: 'The role with name <roleName> cannot be found.' (Smithy: AddRoleToInstanceProfile declares NoSuchEntityException, code NoSuchEntity, HTTP 404). Fixed text around the requested role name.
Emulator: 'Role <roleName> cannot be found.'
Why: message-matching clients misclassify the refusal.
Fix: the AddRoleToInstanceProfile handler's not-found message.
Traces: iam.instance-profile-add-nonexistent-role-rejected.

Cause 6 of 10: re-adding the role an instance profile already holds is answered with the wrong error class.
AWS: the profile quota is one role, so the second AddRoleToInstanceProfile is refused LimitExceeded (Smithy: AddRoleToInstanceProfile declares LimitExceededException, code LimitExceeded, HTTP 409) with 'Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1' — fixed text plus the quota value; it is not an already-exists conflict.
Emulator: EntityAlreadyExists with 'Role <role> is already associated with Instance Profile <profile>.'
Why: the two codes imply different retry semantics (a quota refusal is permanent for that profile, a conflict suggests idempotent success), so a caller retries wrongly.
Fix: the AddRoleToInstanceProfile handler's duplicate/quota branch.
Traces: iam.instance-profile-re-adding-its-own-role-rejected.

Cause 7 of 10: an AWS-managed policy's PolicyId is not an IAM unique id.
AWS: GetPolicy on an AWS-managed policy returns Policy.PolicyId as an ANPA-prefixed unique id — 'ANPA' followed by 17 uppercase alphanumerics (matching ^ANPA[A-Z0-9]{17}$). Smithy: GetPolicy returns the Policy shape whose PolicyId is an opaque idType.
Emulator: returns a name-derived string, e.g. 'ANPA_READONLYACCESS' for arn:aws:iam::aws:policy/ReadOnlyAccess, which is not of that form.
Why: clients that validate or index by the unique-id format reject the value.
Fix: the AWS-managed policy catalog's id minting behind GetPolicy.
Traces: iam.get-aws-managed-policy-metadata-and-default-version.

Cause 8 of 10: GetServerCertificate does not return the uploaded PEM with trailing whitespace stripped.
AWS: recorded behavior only. UploadServerCertificate stores the certificate body with trailing whitespace removed, so GetServerCertificate's CertificateBody equals the uploaded PEM stripped of trailing whitespace (no trailing newline).
Emulator: echoes the body verbatim, keeping the trailing newline the upload carried, so a byte comparison against the stripped upload fails.
Why: a client that compares the stored certificate with the file it uploaded sees a spurious difference.
Fix: UploadServerCertificate's normalization of the stored body (and the chain), read back by GetServerCertificate.
Traces: iam.server-certificate-body-chain-and-key-match.

Cause 9 of 10: the service-specific-credential quota is named wrongly in the refusal.
AWS: the credential beyond the per-user-per-service limit is refused with 'Cannot exceed quota for ContextSpecificCredentialsPerUserPerContext: 2' — fixed text plus the quota value, and every loser of a concurrent race gets that same message.
Emulator: 'Cannot exceed quota for ServiceSpecificCredentialsPerUserPerService: 2'.
Why: operators and quota tooling keyed on the quota name cannot find it.
Fix: the CreateServiceSpecificCredential quota-refusal message.
Traces: iam.service-specific-credential-quota-race.

Cause 10 of 10: aws:SecureTransport is never evaluated on a real signed call.
AWS: an Allow whose Condition is Bool aws:SecureTransport=false does not authorize a request made over TLS, so once it is the only Allow left the signed call is refused AccessDenied.
Emulator: that refusal never arrives — the check's bounded poll for it never converges and the check exhausts the 60 s budget, so the condition key is not consulted at all.
Why: policies that gate access on transport security are silently ineffective; a caller is admitted where AWS refuses it.
Fix: the condition-key evaluation in the IAM authorizer must populate and test aws:SecureTransport.
Traces: iam.sep-policy-composition-securetransport-condition-denied-when-policy-requires-plaintext.

## Evidence
Cause 1: "expected message 'User: ... is not authorized to perform: iam:ListAttachedRolePolicies on resource: role ... because no identity-based policy allows the iam:ListAttachedRolePolicies action. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/<absent> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <absent>', got 'User: ... because no identity-based policy allows the iam:ListAttachedRolePolicies action'".
Cause 2: expected '... with an explicit deny in an identity-based policy: arn:aws:iam::000000000000:policy/sim2real-sep-deny-deny-704f151bd1b3. Go to ...', got '... with an explicit deny in an identity-based policy'.
Cause 3: 'AssertionError: Policy arn:aws:iam::aws:policy/aws-service-role/AWSBackupServiceLinkedRolePolicyForBackup is not attachable.' where 'Cannot attach AWS reserved policy to an IAM role.' was required.
Cause 4: expected ('DeleteConflict', 'Cannot delete entity, must remove roles from instance profile first.'), got ('DeleteConflict', 'Cannot delete entity, must detach all policies first.').
Cause 5: expected ('NoSuchEntity', 'The role with name <role> cannot be found.'), got ('NoSuchEntity', 'Role <role> cannot be found.').
Cause 6: expected ('LimitExceeded', 'Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1'), got ('EntityAlreadyExists', 'Role <role> is already associated with Instance Profile <profile>.').
Cause 7: "GetPolicy('arn:aws:iam::aws:policy/ReadOnlyAccess') PolicyId 'ANPA_READONLYACCESS'" against ^ANPA[A-Z0-9]{17}$.
Cause 8: 'GetServerCertificate body differs from the upload (stripped of trailing whitespace)' — the returned body ends '-----END CERTIFICATE-----\n'.
Cause 9: "expected every loser's message 'Cannot exceed quota for ContextSpecificCredentialsPerUserPerContext: 2', got ['Cannot exceed quota for ServiceSpecificCredentialsPerUserPerService: 2', ...]".
Cause 10: 'Error: Check timed out after 60.0s' while polling for 'real TLS ListUserTags once only a plaintext Allow remains'.

## Steps to Reproduce
1. Cause 1/2: create a user with an access key, attach an inline or managed Deny for one action, call that action signed as the user and read the AccessDenied Message; repeat with a zero-permission user for the implicit-deny head.
2. Cause 3: call PutRolePermissionsBoundary / PutUserPermissionsBoundary with any arn:aws:iam::aws:policy/aws-service-role/... policy.
3. Cause 4: create a role, attach a policy, add it to an instance profile, then DeleteRole.
4. Cause 5/6: AddRoleToInstanceProfile with a nonexistent role, then twice with the same existing role.
5. Cause 7: GetPolicy('arn:aws:iam::aws:policy/ReadOnlyAccess').
6. Cause 8: UploadServerCertificate with a PEM ending in a newline, then GetServerCertificate.
7. Cause 9: create three service-specific credentials for one user and service.
8. Cause 10: leave only an Allow conditioned on Bool aws:SecureTransport=false and make the signed call over TLS.
9. Or replay the check ids in trace_ids.
