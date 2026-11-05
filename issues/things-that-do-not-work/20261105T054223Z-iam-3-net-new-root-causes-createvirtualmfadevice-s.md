Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.create-virtual-mfa-device-duplicate-name-different-path-rejected iam.duplicate-create-returns-entityalreadyexists iam.enable-mfa-device-wrong-code-rejected iam.policy-invalid-effect
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.create-virtual-mfa-device-duplicate-name-different-path-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.duplicate-create-returns-entityalreadyexists|run#72|AssertionError|assert msg_mfa == "MFA device already exists.", f"mfa: unexpected message {msg_mfa!r}"
Issue-Key: iam.enable-mfa-device-wrong-code-rejected|run#11|AssertionError|assert code == "InvalidAuthenticationCode", f"expected InvalidAuthenticationCode, got {code!r}"
Issue-Key: iam.policy-invalid-effect|run#12|AssertionError|assert message == "Syntax errors in policy.", (
Issue-Commit: 089026a1c9416fdeaf52bd064325368a385b884c

# iam: 3 net-new root causes — CreateVirtualMFADevice's duplicate refusal message is emulator prose, EnableMFADevice answers 501 instead of InvalidAuthenticationCode for a wrong TOTP code, and CreatePolicy's MalformedPolicyDocument message is emulator prose

> _Note: this issue was identified against the code base at commit `089026a1c9416fdeaf52bd064325368a385b884c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVirtualMFADevice

## Description
THREE distinct net-new root causes in the bootstrapped `iam` service, grounded in four failing checks. None of the four is marked ALREADY FILED or OWNED in the replay report. A fixer must close all THREE causes before submitting.

Nearest open/resolved neighbours, named so they are not re-fixed and are not mistaken for coverage: 20261027T214234Z-iam-6-net-new-behavioral-root-causes-enablemfadevice.md (its Cause 1 is the same EnableMFADevice scaffold stub but graded ONLY on the unregistered-serial NoSuchEntity leg; Cause 2 below adds the wrong-authentication-code leg's replay key and its distinct error code/message, and the two should be closed together — its Cause 4 concerns the UpdateAssumeRolePolicy trust-document message, a different operation and a different template than Cause 3 below), the auto-filed scaffold-stub issue the report raises for `EnableMFADevice` (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/enable_mfa_device.rs), and the resolved 20261024T065715Z-iam-createvirtualmfadevice-stub.md (CreateVirtualMFADevice now answers and creates devices — the positive legs of both checks below pass — so Cause 1 is a message defect on a working operation, NOT a reopening of that stub).

Cause 1 of 3: CreateVirtualMFADevice's EntityAlreadyExists refusal for a duplicate name+path returns emulator prose instead of IAM's sentence.
AWS: Smithy iam#CreateVirtualMFADevice takes VirtualMFADeviceName (required, @length min 1, pattern ^[\w+=,.@-]+$), optional Path (pathType, @length 1-512) and optional Tags, and models EntityAlreadyExistsException (awsQueryError EntityAlreadyExists, httpResponseCode 409). Virtual-MFA-device uniqueness is ARN-scoped, i.e. per Path+name: creating a device whose Path AND VirtualMFADeviceName both match an existing device is refused EntityAlreadyExists / HTTP 409 with the message "MFA device already exists." — the whole message is FIXED text, with no variable part (the name and path are deliberately not interpolated). The same name under a DIFFERENT Path is accepted and yields a distinct SerialNumber/ARN ending in <path><name>.
Emulator: the error code is already right (EntityAlreadyExists), but the message is "MFADevice entity at the same path and name already exists." — a different, emulator-authored sentence that names the entity type and describes the collision.
Why: the message is the only human-readable discriminator IAM returns for this code, and CreateVirtualMFADevice's EntityAlreadyExists has no other variable content to key on; enrolment tooling and operators that match IAM's exact sentence (or surface it verbatim) see text AWS never emits, and the divergence masks whether the emulator's uniqueness rule is ARN-scoped at all — the path-scoped acceptance leg of the check is downstream of this assertion and was never reached, so it is still ungraded.
Fix: the EntityAlreadyExists message constant in the duplicate-detection branch of the CreateVirtualMFADevice handler (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_virtual_mfa_device.rs) must emit IAM's fixed sentence.
Traces: iam.create-virtual-mfa-device-duplicate-name-different-path-rejected, iam.duplicate-create-returns-entityalreadyexists (both fail on exactly this message; in the second, the user/group/role/policy/instance-profile/OIDC-provider duplicate legs that run before it all pass).

Cause 2 of 3: EnableMFADevice is an unimplemented scaffold stub, so a wrong authentication code is answered 501 NotImplemented instead of InvalidAuthenticationCode, and MFA enrolment cannot be completed at all.
AWS: Smithy iam#EnableMFADevice takes UserName (required, @length 1-128), SerialNumber (required, @length 9-256), AuthenticationCode1 and AuthenticationCode2 (both required, @length 6-6, pattern ^[\d]+$) and models InvalidAuthenticationCodeException (awsQueryError InvalidAuthenticationCode, httpResponseCode 403) alongside NoSuchEntityException (404), EntityTemporarilyUnmodifiableException (409), EntityAlreadyExistsException, ConcurrentModificationException, LimitExceededException and ServiceFailureException. Codes that do not match the device's TOTP seed are refused InvalidAuthenticationCode / HTTP 403 with the message "Authentication code for device is not valid." — the whole message is FIXED text with no variable part — and the refusal is side-effect free: the device stays in ListVirtualMFADevices(AssignmentStatus='Unassigned') and is absent from AssignmentStatus='Assigned'. The identical call made with two consecutive real TOTP codes computed from the Base32StringSeed CreateVirtualMFADevice returned succeeds (empty output, HTTP 200) and moves the device to Assigned.
Emulator: every EnableMFADevice call answers code NotImplemented / HTTP 501 with "operation EnableMFADevice is scaffolded but not implemented in this emulator" (the handler at services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/enable_mfa_device.rs is the generated scaffold). The replay report's stub detector did not flag THIS check because the check catches the ClientError and re-raises it as an assertion, so this wrong-code leg carries no auto-filed coverage.
Why: NotImplemented is not an IAM error code and is not in the operation's modeled error set, so no SDK error shape matches it; a client cannot distinguish a wrong TOTP code (retryable, 403) from an unregistered serial (404) from a broken service, and because the positive leg fails too, no virtual MFA device can ever be assigned — every downstream aws:MultiFactorAuthPresent condition and every MFA-gated flow is unreachable.
Fix: implement the handler in services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/enable_mfa_device.rs against the service state's virtual-MFA-device store — resolve the user and the serial, verify both codes as RFC-6238 TOTP (SHA-1, 30 s step, 6 digits) against the device's stored Base32 seed for the current and the adjacent window, refuse InvalidAuthenticationCode with AWS's fixed message and change no state when they do not match, and on success mark the device Assigned to that user so ListVirtualMFADevices reports it under AssignmentStatus='Assigned' — and remove the SCAFFOLDED marker.
Traces: iam.enable-mfa-device-wrong-code-rejected.

Cause 3 of 3: CreatePolicy's MalformedPolicyDocument refusal for a statement whose Effect is neither Allow nor Deny returns emulator prose instead of IAM's sentence.
AWS: Smithy iam#CreatePolicy takes PolicyName (required) and PolicyDocument (required, policyDocumentType @length 1-131072) and models MalformedPolicyDocumentException (awsQueryError MalformedPolicyDocument, httpResponseCode 400). Effect accepts only "Allow" or "Deny"; any other value is refused MalformedPolicyDocument / HTTP 400 with the message "Syntax errors in policy." — the whole message is FIXED text with no variable part — and nothing is persisted, so GetPolicy on the would-be ARN answers NoSuchEntity / HTTP 404 with "Policy <policyArn> was not found." (fixed text "Policy " + " was not found.", the one variable part being the PolicyArn from the request).
Emulator: the code is already right (MalformedPolicyDocument) but the message is "The policy failed legacy parsing" — an emulator-authored sentence naming an internal parsing stage, with no terminating period.
Why: MalformedPolicyDocument is returned for many distinct document defects and the message is the only thing that classifies them; IaC engines and policy linters that surface or match IAM's wording report an error AWS never emits, and a message naming a "legacy parsing" stage points an operator at emulator internals rather than at their policy. The follow-on assertion that nothing persisted (GetPolicy -> NoSuchEntity with "Policy <arn> was not found.") is downstream of this one and is still ungraded.
Fix: the MalformedPolicyDocument message rendering for the invalid-Effect branch of the shared policy-document validator used by the CreatePolicy handler (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_policy.rs) must emit IAM's fixed sentence instead of the parser's internal wording. Distinct from Cause 4 of 20261027T214234Z, which is about UpdateAssumeRolePolicy needing IAM's different trust-document sentence.
Traces: iam.policy-invalid-effect.

## Evidence
Suite simparity-behavioral-aws-v1, replayed against this checkout: 419/793 passed; service heading `iam` 308/456. The `iam` service is stood up (only iot is pending bootstrap). The four checks below are iam-headed failures that the report marks neither ALREADY FILED nor OWNED.

Cause 1 — [mismatch] iam.create-virtual-mfa-device-duplicate-name-different-path-rejected, check line 34:
  AssertionError: expected message 'MFA device already exists.', got 'MFADevice entity at the same path and name already exists.'
  Sequence: CreateVirtualMFADevice(VirtualMFADeviceName=N, Path='/sim2real-path-a/', Tags=[...]) succeeds and returns a SerialNumber; CreateVirtualMFADevice(VirtualMFADeviceName=N, Path='/sim2real-path-a/') is then refused.
  The code assertion (EntityAlreadyExists, iam#EntityAlreadyExistsException, awsQueryError EntityAlreadyExists / HTTP 409) PASSED at check line 32 — only the message diverges, so the operation is not a stub in this checkout.
  Not yet graded (downstream of the failing assertion): that exactly one device carries that name+path, and that the same name under Path='/sim2real-path-b/' is ACCEPTED with a distinct SerialNumber ending in '/sim2real-path-b/<name>'.

Cause 1 (second replay key) — [mismatch] iam.duplicate-create-returns-entityalreadyexists, check line 269:
  AssertionError: mfa: unexpected message 'MFADevice entity at the same path and name already exists.' (expected 'MFA device already exists.')
  The duplicate-create legs for user, group, role, managed policy, instance profile and OIDC provider all passed before this one, which localizes the defect to the virtual-MFA-device branch.

Cause 2 — [mismatch] iam.enable-mfa-device-wrong-code-rejected, check line 39:
  AssertionError: expected InvalidAuthenticationCode, got 'NotImplemented'
  Sequence: CreateUser U; CreateVirtualMFADevice -> SerialNumber S and Base32StringSeed (both legs succeed); EnableMFADevice(UserName=U, SerialNumber=S, AuthenticationCode1='000000', AuthenticationCode2='111111').
  Expected: InvalidAuthenticationCode / HTTP 403 with exactly 'Authentication code for device is not valid.'; S still listed under AssignmentStatus='Unassigned' and absent from 'Assigned'; then the same call with _totp(seed, now) and _totp(seed, now+30) succeeds and S appears under 'Assigned'.
  Actual: NotImplemented / HTTP 501, 'operation EnableMFADevice is scaffolded but not implemented in this emulator'. Both the refusal leg and the positive enrolment leg are unreachable.

Cause 3 — [mismatch] iam.policy-invalid-effect, check line 58:
  AssertionError: expected the exact MalformedPolicyDocument message AWS returns, got 'The policy failed legacy parsing'
  Request: CreatePolicy(PolicyName=<name>, PolicyDocument={"Version":"2012-10-17","Statement":[{"Effect":"Maybe","Action":"iam:GetUser","Resource":"arn:aws:iam::*:user/sim2real-behav-*"}]}).
  The code assertion (MalformedPolicyDocument / HTTP 400) PASSED at check line 55, and the positive control passed first: the identical statement with Effect='Allow' was accepted and returned the ARN arn:aws:iam::<account>:policy/<name>.
  Expected message: 'Syntax errors in policy.' (entirely fixed text). Not yet graded: GetPolicy on the rejected policy's ARN answering NoSuchEntity / 404 with 'Policy <policyArn> was not found.'

All entity names, serials, seeds and account ids in the replay output are this run's generated values filling the templates; they are not part of any requirement.

## Steps to Reproduce
Boot the emulator from this checkout and point an `iam` client at it, signed as a principal permitted to call IAM.

Cause 1:
1. CreateVirtualMFADevice(VirtualMFADeviceName='dev1', Path='/p-a/') — succeeds, note the SerialNumber.
2. CreateVirtualMFADevice(VirtualMFADeviceName='dev1', Path='/p-a/') again.
3. Observe EntityAlreadyExists / HTTP 409 (correct) with 'MFADevice entity at the same path and name already exists.' instead of exactly 'MFA device already exists.'
4. Then CreateVirtualMFADevice(VirtualMFADeviceName='dev1', Path='/p-b/') — it must be ACCEPTED with a distinct SerialNumber ending '/p-b/dev1', and ListVirtualMFADevices must then show two devices sharing that name.
   Replay: iam.create-virtual-mfa-device-duplicate-name-different-path-rejected, iam.duplicate-create-returns-entityalreadyexists

Cause 2:
1. CreateUser U.
2. CreateVirtualMFADevice(VirtualMFADeviceName='devX') — note SerialNumber S and Base32StringSeed.
3. EnableMFADevice(UserName=U, SerialNumber=S, AuthenticationCode1='000000', AuthenticationCode2='111111').
4. Observe NotImplemented / HTTP 501 instead of InvalidAuthenticationCode / HTTP 403 with 'Authentication code for device is not valid.'; also check that S remains under ListVirtualMFADevices(AssignmentStatus='Unassigned') and is absent from 'Assigned'.
5. Compute two consecutive RFC-6238 TOTP codes from the Base32StringSeed (SHA-1, 30 s step, 6 digits, counters now and now+30) and call EnableMFADevice with them — it must succeed and S must then appear under AssignmentStatus='Assigned'.
   Replay: iam.enable-mfa-device-wrong-code-rejected

Cause 3:
1. CreatePolicy(PolicyName=P1, PolicyDocument={"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:GetUser","Resource":"arn:aws:iam::*:user/x-*"}]}) — positive control, succeeds.
2. CreatePolicy(PolicyName=P2, PolicyDocument= the same document with "Effect":"Maybe").
3. Observe MalformedPolicyDocument / HTTP 400 (correct) with 'The policy failed legacy parsing' instead of exactly 'Syntax errors in policy.'
4. GetPolicy(PolicyArn='arn:aws:iam::<account>:policy/P2') must answer NoSuchEntity / 404 with 'Policy arn:aws:iam::<account>:policy/P2 was not found.'
   Replay: iam.policy-invalid-effect
