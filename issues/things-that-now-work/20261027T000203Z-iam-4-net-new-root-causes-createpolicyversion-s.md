Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.iam-bound-deep-iam-3-managed-policy-version-over-6144-significant-chars-rejected iam.malformed-policy-document-rejected iam.non-user-caller-must-specify-username-rejected iam.service-specific-credential-cross-user-isolation-rejected
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.iam-bound-deep-iam-3-managed-policy-version-over-6144-significant-chars-rejected|run#16|AssertionError|assert message == expected, f"message: expected {expected!r}, got {message!r}"
Issue-Key: iam.malformed-policy-document-rejected|run#14|AssertionError|assert code_cpv_message == expected, f"expected message {expected!r}, got {code_cpv_message!r}"
Issue-Key: iam.non-user-caller-must-specify-username-rejected|run#33|AssertionError|assert assumed_outcome == expected, (
Issue-Key: iam.service-specific-credential-cross-user-isolation-rejected|_assert_eventually_rejected#32|AssertionError|raise AssertionError(f"expected rejection {expected!r}, got {last!r} after {timeout_s}s")
Issue-Commit: df75a05586985baf68c72ff4d4d2cf208170da4a

# iam: 4 net-new root causes — CreatePolicyVersion's and the policy-document validator's refusal messages are emulator prose, the implicit-UserName refusal uses the code ValidationException, and service-specific-credential ownership mismatch answers NoSuchEntity

> _Note: this issue was identified against the code base at commit `df75a05586985baf68c72ff4d4d2cf208170da4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicyVersion

## Description
Four net-new root causes in the iam emulator. None is an unimplemented scaffold stub — every operation below answers and refuses, just with the wrong code or the wrong message text. Nearest open neighbours, none of which covers any cause here: 20261025T000319Z-iam-9-net-new-behavioral-root-causes-cloudformation.md and 20261025T224240Z-iam-2-net-new-root-causes-putgrouppolicy-s.md (refusal text for CreatePolicy / PutGroupPolicy size limits, not CreatePolicyVersion and not the syntax-error message), 20261025T000319Z-iam-10-net-new-behavioral-root-causes-four.md (password policy, credential report, STS preferences), and 20261025T153722Z-iam-3-net-new-root-causes-the-built.md (the AWS-managed catalog and ListPolicies' enum message). A fixer must close all FOUR causes below before submitting.

Cause 1 of 4: CreatePolicyVersion's over-size refusal message is emulator prose that names the policy ARN, not AWS's quota message.
AWS: Smithy iam#CreatePolicyVersion takes PolicyDocument (policyDocumentType, required, @length 1-131072) and models LimitExceededException (awsQueryError code LimitExceeded, HTTP 409). The managed-policy size quota is 6,144 characters and IAM does not count whitespace toward it. Recorded behavior for the refusal text: the message is exactly "Cannot exceed quota for PolicySize: 6144" — fixed text "Cannot exceed quota for PolicySize: " plus the quota value for the entity class (6144 for a managed policy). It names no ARN, no policy name and no measured size. The error CODE the emulator returns is already right (LimitExceeded); only the message diverges.
Emulator: the same call is refused with the message "Maximum policy size of 6144 characters exceeded for policy arn:aws:iam::<account>:policy/<path>/<name>" — a different template that interpolates the policy's ARN (a value from the request) into text AWS never produces.
Why: a client or test that matches IAM's quota message, or extracts the exceeded quota name/limit from it, sees text AWS never emits; the ARN interpolation makes the message vary per resource where AWS's does not. Anything keying on "Cannot exceed quota for PolicySize" to distinguish a size-quota refusal from the other LimitExceeded causes (version count, attachment count) cannot classify it.
Fix: the refusal-message rendering in the CreatePolicyVersion handler's size-validation step, which must emit AWS's fixed quota template rather than an ARN-bearing sentence. (The same template is used by CreatePolicy; keep them consistent.)
Traces: iam.iam-bound-deep-iam-3-managed-policy-version-over-6144-significant-chars-rejected.

Cause 2 of 4: the policy-document syntax refusal appends a generated explanation to AWS's fixed message.
AWS: Smithy iam#CreatePolicy / iam#CreatePolicyVersion model MalformedPolicyDocumentException (awsQueryError code MalformedPolicyDocument, HTTP 400). Recorded behavior for a document that is syntactically unusable (not a JSON object, or carrying no Statement element): the message is exactly the fixed sentence "Syntax errors in policy." — the whole message is fixed text, with NO variable part at all and no diagnostic detail.
Emulator: the same document is refused with "Syntax errors in policy: PolicyDocument must be a JSON object carrying a Statement element" — AWS's sentence with its terminating period replaced by a colon and an emulator-authored diagnosis appended.
Why: AWS deliberately gives no detail here; a client that compares IAM's message, logs it, or branches on it sees a longer, differently punctuated string. Worse, the appended clause will drift with the emulator's parser while AWS's text never changes, so no client can rely on it.
Fix: the policy-document validator that produces MalformedPolicyDocumentException for this service's operations must emit AWS's fixed message verbatim and carry its diagnosis internally, not in the wire message.
Traces: iam.malformed-policy-document-rejected.

Cause 3 of 4: the implicit-UserName refusal is returned under the error code ValidationException instead of IAM's ValidationError.
AWS: Smithy iam#ListSigningCertificates models UserName as OPTIONAL (existingUserNameType) and declares only NoSuchEntityException and ServiceFailureException — so the refusal for a non-User caller omitting UserName is IAM's unmodeled bad-request error, and the awsQuery wire code for it is `ValidationError` (as the other IAM validation refusals in this emulator already use). Recorded behavior: a caller whose credentials are an assumed role (not an IAM user) that omits UserName is refused with code ValidationError and the message "Must specify userName when calling with non-User credentials" — the entire message is fixed text. An IAM-user caller omitting UserName has it resolved implicitly to the signing user.
Emulator: the message is exactly right, but the error code is `ValidationException` — the Smithy/JSON-protocol spelling, not the awsQuery `ValidationError` IAM uses. The check confirms the caller was fully authorized for ListSigningCertificates, so this is implicit-resolution's own refusal and not an authorization failure.
Why: every IAM client dispatches on the error code, not the message. `ValidationException` is not an IAM error code, so retry/branch logic written against IAM (and the SDKs' own IAM error shapes) fails to recognise a plain validation refusal and treats it as an unknown error.
Fix: the error-code mapping for unmodeled bad-request refusals in the ListSigningCertificates handler's implicit-UserName resolution path must emit the awsQuery code ValidationError that the rest of this service uses.
Traces: iam.non-user-caller-must-specify-username-rejected.

Cause 4 of 4: naming another user's service-specific credential is refused NoSuchEntity with emulator prose, where AWS answers ValidationError naming the user and the credential id.
AWS: Smithy iam#UpdateServiceSpecificCredential takes UserName (optional, userNameType) and ServiceSpecificCredentialId (required, serviceSpecificCredentialId) and models only NoSuchEntityException, so the ownership-mismatch refusal is recorded behavior only. The recording shows that pairing an existing credential id with a DIFFERENT user's name is refused with code `ValidationError` and the message "User <userName> does not have a service specific credential with id <credentialId>" — fixed text "User " ... " does not have a service specific credential with id ", with both variable parts taken from the request (the UserName and the ServiceSpecificCredentialId). AWS reports it as a validation failure of the (user, credential) pair, not as a missing credential, which is what makes the ownership boundary observable; the same holds for ResetServiceSpecificCredential and DeleteServiceSpecificCredential.
Emulator: the refusal is code `NoSuchEntity` with the message "No such credential <credentialId> exists" — it drops the UserName from both the classification and the text, so the response claims the credential does not exist at all even though it does exist under its real owner.
Why: a client cannot distinguish "this credential does not exist" from "this credential is not yours". Code that treats NoSuchEntity as a cue to re-create the credential will mint a duplicate; code that audits cross-tenant isolation sees a not-found instead of the validation refusal AWS gives, and the response leaks the wrong conclusion about the credential's existence.
Fix: the ownership check in the UpdateServiceSpecificCredential handler (and the identical branch in Reset/DeleteServiceSpecificCredential) must, when the credential exists but belongs to another user, answer ValidationError with AWS's template filled from the request's UserName and ServiceSpecificCredentialId, rather than the state layer's generic not-found.
Traces: iam.service-specific-credential-cross-user-isolation-rejected.

## Evidence
Suite simparity-behavioral-ga-prod-v1; iam is bootstrapped and every operation below answers (no scaffold stub in any of these paths).

Cause 1 — iam.iam-bound-deep-iam-3-managed-policy-version-over-6144-significant-chars-rejected [mismatch], check line 64:
  AssertionError: message: expected 'Cannot exceed quota for PolicySize: 6144', got 'Maximum policy size of 6144 characters exceeded for policy arn:aws:iam::000000000000:policy/sim2real-psv-e440ce60661b/sim2real-psv-e440ce60661b'
  The error code assertion (code == "LimitExceeded", iam#CreatePolicyVersion's modeled LimitExceededException, awsQueryError LimitExceeded / HTTP 409) passed on line 55; only the message diverges. Fixed text of AWS's message: "Cannot exceed quota for PolicySize: "; variable part: the quota value (6144 for a managed policy). The ARN in the emulator's message (account 000000000000 and the generated policy name/path of this run) is request-derived and has no place in AWS's template.
  Request: CreatePolicyVersion(PolicyArn=<an existing policy>, PolicyDocument=<6,145 significant characters padded with ~4 KB of whitespace>, SetAsDefault=True). The two positive controls — 6,140 and exactly 6,144 significant characters, each raw-oversize by ~4 KB of whitespace — were both ACCEPTED and read back intact, so the whitespace-exclusion rule and the boundary itself are already correct.

Cause 2 — iam.malformed-policy-document-rejected [mismatch], check line 100:
  AssertionError: expected message 'Syntax errors in policy.', got 'Syntax errors in policy: PolicyDocument must be a JSON object carrying a Statement element'
  Expected: MalformedPolicyDocument (HTTP 400, awsQueryError MalformedPolicyDocument) with the message exactly "Syntax errors in policy." — entirely fixed text, no variable part.
  Actual: the same code with AWS's sentence truncated at the period and an emulator-authored clause appended.

Cause 3 — iam.non-user-caller-must-specify-username-rejected [mismatch], check line 174:
  AssertionError: a non-User (assumed-role) caller omitting UserName must be rejected ('ValidationError', 'Must specify userName when calling with non-User credentials') -- it is fully authorized for ListSigningCertificates, so this is implicit-resolution's own refusal, not an authorization failure; got ('ValidationException', 'Must specify userName when calling with non-User credentials')
  Expected code: ValidationError. Actual code: ValidationException. The message matches AWS's fixed text exactly; the code alone diverges. The check's positive control (an IAM user omitting UserName gets it resolved implicitly to the signing user) passed.

Cause 4 — iam.service-specific-credential-cross-user-isolation-rejected [mismatch], check line 110 via _assert_eventually_rejected at line 185:
  AssertionError: expected rejection ('ValidationError', 'User sim2real-behav-sscother-1654a5ab7fec does not have a service specific credential with id EMULATORAKFURU7KVYOE'), got ('NoSuchEntity', 'No such credential EMULATORAKFURU7KVYOE exists') after 30.0s
  Request: UpdateServiceSpecificCredential(UserName=<a second, unrelated user>, ServiceSpecificCredentialId=<the first user's real credential id>, Status=...).
  Fixed text of AWS's message: "User " + " does not have a service specific credential with id "; variable parts: the UserName and the ServiceSpecificCredentialId, both from the request (the specific user name and credential id above are this run's generated values, not part of the template).
  The emulator answered a stable wrong result for the full 30s re-poll window, so this is not an eventual-consistency artifact.

## Steps to Reproduce
Boot the emulator and sign requests as a principal permitted to call IAM.

Cause 1:
1. CreatePolicy with a small document; note its Arn.
2. CreatePolicyVersion(PolicyArn=<arn>, PolicyDocument=<a document of 6,145 significant (non-whitespace) characters, padded with several KB of whitespace>, SetAsDefault=True).
3. Observe error code LimitExceeded (correct) with message "Maximum policy size of 6144 characters exceeded for policy <arn>" instead of AWS's "Cannot exceed quota for PolicySize: 6144".
   Replay: iam.iam-bound-deep-iam-3-managed-policy-version-over-6144-significant-chars-rejected

Cause 2:
1. CreatePolicy (or CreatePolicyVersion) with a PolicyDocument that is valid JSON but not a policy — e.g. a JSON array, or an object with no Statement element.
2. Observe MalformedPolicyDocument with message "Syntax errors in policy: PolicyDocument must be a JSON object carrying a Statement element" instead of AWS's "Syntax errors in policy."
   Replay: iam.malformed-policy-document-rejected

Cause 3:
1. Create a role, attach a policy allowing iam:ListSigningCertificates on *, and AssumeRole it.
2. Using those assumed-role credentials, call ListSigningCertificates with NO UserName.
3. Observe error code ValidationException instead of IAM's ValidationError (the message "Must specify userName when calling with non-User credentials" is already correct).
   Replay: iam.non-user-caller-must-specify-username-rejected

Cause 4:
1. CreateUser A and CreateUser B.
2. CreateServiceSpecificCredential for user A; note its ServiceSpecificCredentialId.
3. Call UpdateServiceSpecificCredential(UserName=B, ServiceSpecificCredentialId=<A's id>, Status=Inactive). Repeat with ResetServiceSpecificCredential and DeleteServiceSpecificCredential.
4. Observe ('NoSuchEntity', 'No such credential <id> exists') instead of ('ValidationError', 'User B does not have a service specific credential with id <id>'), and confirm with ListServiceSpecificCredentials(UserName=A) that the credential still exists under A.
   Replay: iam.service-specific-credential-cross-user-isolation-rejected
