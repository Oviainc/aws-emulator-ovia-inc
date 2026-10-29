Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.get-microvm-image-access-denied
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms.get-microvm-image-access-denied|run#8|AssertionError|assert bogus_code == "AccessDeniedException", bogus_code
Issue-Commit: 42eb24959c6a4eb33ac3ba3d48b702ee83f6032d

# lambda-microvms: GetMicrovmImage leaks existence with 404 ResourceNotFoundException where AWS denies with 403 AccessDeniedException for an identifier the caller holds no permission on

> _Note: this issue was identified against the code base at commit `42eb24959c6a4eb33ac3ba3d48b702ee83f6032d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMicrovmImage

## Description
REOPENS 20261027T003752Z-lambda-microvms-re-delete-of-a-gone-image.md: its fix left the call refused. That issue's Cause 3 of 3 required the customer-facing GetMicrovmImage read to answer the AUTHORIZATION verdict (403 AccessDeniedException) instead of reporting absence (404), and the fix landed for exactly ONE identifier class: the AWS-owned managed namespace. GetMicrovmImage still answers a 404 refusal for the other identifier class the same recording covers, so the authorization-before-existence rule the fix was supposed to establish is still not in force.

This issue carries ONE root cause.

Cause 1 of 1: the authorization verdict is applied only to the AWS-managed namespace, so a well-formed same-account microvm-image identifier the caller has no permission on is answered with 404 ResourceNotFoundException instead of 403 AccessDeniedException ===
Failing check: lambda-microvms.get-microvm-image-access-denied

AWS: GetMicrovmImage (Smithy: http GET /2025-09-09/microvm-images/{imageIdentifier}, success 200; input imageIdentifier @required @httpLabel, length 1..256) models BOTH AccessDeniedException (errorKind client, httpError 403, "You do not have sufficient access to perform this action.") and ResourceNotFoundException (errorKind client, httpError 404, message @required, "The specified resource does not exist."). Which one AWS picks is decided by authorization, not by existence: recorded behavior is that the verification principal — which holds no lambda-microvms:GetMicrovmImage grant on the identifier — is refused with AccessDeniedException BOTH for an AWS-managed base image ARN just returned by ListManagedMicrovmImages (a resource that unambiguously EXISTS) and for a well-formed same-account ARN of the form arn:<partition>:lambda:<region>:<account-id>:microvm-image/<name> that names nothing. The service decides access before it ever reaches an existence check, so it never reports absence for an identifier the caller may not read. The recorded message names the CALLING PRINCIPAL's own ARN, so its only variable part is caller identity and nothing in it comes from the request; the required parts here are the error code AccessDeniedException and HTTP 403 (the check asserts the code only, and says so in its own comment). Hypothesis: the discriminator is whether the identifier names a resource this principal was granted, not whether it exists — the 404 path stays correct for an identifier whose record this account itself created and then deleted, which lambda-microvms.deleted-image-name-is-released-for-reuse requires (that check polls GetMicrovmImage until it answers ResourceNotFoundException for an image this account created). Only the model and the two recorded refusals above are relied on for the requirement itself.

Emulator: GetMicrovmImage decides the authorization verdict for the managed namespace only, then falls through to a not-found answer for everything else. In services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image.rs the handler first tests crate::state::is_managed_microvm_image_arn and returns AccessDeniedException with crate::state::ACCESS_DENIED_MESSAGE (so the managed arm of this check now passes), but a same-account microvm-image identifier that matches nothing in the account's own store resolves to crate::state::ImageReference::Absent and is answered with ResourceNotFoundException built by crate::state::image_not_found_message — HTTP 404 instead of 403. Replay: the first probe (a real managed base image ARN from ListManagedMicrovmImages) is correctly denied, and the check then fails on the second probe with "AssertionError: ResourceNotFoundException" where "AccessDeniedException" was asserted.

Why: a client gets both the wrong error class and information it is not entitled to. 403 means "stop, you need a permission"; 404 means "this does not exist yet, keep polling / create it". A caller refused this way retries or creates forever, and SDK error-classification and retry layers treat the two codes differently. Worse, answering 404 for an identifier the principal cannot read turns GetMicrovmImage into an existence oracle: by probing identifiers and reading 404-vs-200 a caller with no permission can enumerate which MicroVM images exist in an account — the permission-bypassing not-found leak this check exists to rule out.

Fix: the authorization branch in GetMicrovmImage's handler (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image.rs), i.e. the same pre-lookup verdict that already special-cases the managed namespace. It must deny — modeled AccessDeniedException, HTTP 403, crate::state::ACCESS_DENIED_MESSAGE — any identifier the caller holds no grant on, including a well-formed same-account microvm-image identifier that resolves to no record this account created, instead of falling through to the ResourceNotFoundException path; the 404 answer must be reserved for identifiers whose record this account owns or owned (so the deleted-image poll keeps its 404). The malformed-identifier ValidationException arm is unaffected. The predicate belongs beside the existing managed-namespace predicate in crate::state rather than being open-coded in the handler.

Traces: lambda-microvms.get-microvm-image-access-denied (asserts GetMicrovmImage is denied with AccessDeniedException for an existing managed base image ARN — passes today — and identically for a bogus well-formed same-account ARN — fails today). No OPEN issue covers this: list_existing_issues(query='lambda-microvms') returns only the GetMicrovmImageVersion scaffold stub and 20261025T210105Z-lambda-microvms-image-builds-never-fail-image-store.md, whose Cause 3 of 3 is likewise confined to the managed-namespace ARN (grounded in lambda-microvms.representation-managed-microvm-image-fields-round-trip) and says nothing about a same-account identifier; the replay report did not mark this check as owned by it.

## Evidence
Behavioral suite: 9/42 passed (21.4%). lambda-microvms 9/40, 31 failed; of the failing clusters 29 are marked ALREADY FILED against the unimplemented GetMicrovmImageVersion scaffold stub and 3 are marked OWNED by 20261025T210105Z-lambda-microvms-image-builds-never-fail-image-store.md. The single remaining cluster is this one.

Expected (AWS, recorded): GetMicrovmImage(imageIdentifier=<well-formed same-account ARN arn:aws:lambda:<region>:<account-id>:microvm-image/<name-that-exists-nowhere>>) -> AccessDeniedException, HTTP 403 (message names the calling principal's ARN; only code+status are required). Same code and status as the probe against a REAL managed base image ARN.
Actual (emulator): ResourceNotFoundException, HTTP 404.
  check_lambda_microvms_get_microvm_image_access_denied.py:33
  assert bogus_code == "AccessDeniedException", bogus_code
  AssertionError: ResourceNotFoundException
Passing preconditions in the same check (so the divergence is confined to the second identifier class): ListManagedMicrovmImages returned at least one Lambda-managed base image, and GetMicrovmImage on that managed ARN WAS denied with AccessDeniedException.

Emulator source: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image.rs lines 180-187 (managed-namespace AccessDeniedException branch — the part of the earlier fix that landed) and lines 196-224 (ImageReference::Absent -> None -> ResourceNotFoundException via crate::state::image_not_found_message, with no authorization verdict for a non-managed identifier).

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json):
  GetMicrovmImage: http GET /2025-09-09/microvm-images/{imageIdentifier}, code 200, @readonly; input imageIdentifier @required @httpLabel (MicrovmImageIdentifier, length 1..256); errors AccessDeniedException(403), InternalServerException(500, retryable), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400).
  AccessDeniedException: errorKind client, httpError 403, "You do not have sufficient access to perform this action."
  ResourceNotFoundException: errorKind client, httpError 404, message @required, "The specified resource does not exist."

## Steps to Reproduce
1. mvm = client('lambda-microvms'); items = mvm.ListManagedMicrovmImages()['items'] -- must be non-empty.
2. mvm.GetMicrovmImage(imageIdentifier=items[0]['imageArn']) -> AccessDeniedException, HTTP 403 (emulator already correct).
3. account = client('sts').GetCallerIdentity()['Account']; bogus = 'arn:aws:lambda:' + <region> + ':' + account + ':microvm-image/missing-image-<unique>' -- a well-formed same-account identifier that names no image.
4. mvm.GetMicrovmImage(imageIdentifier=bogus).
   Emulator: ResourceNotFoundException, HTTP 404.
   AWS: AccessDeniedException, HTTP 403 -- identical code and status to step 2, because authorization is decided before any existence check.
5. Regression guard while fixing: an image this account creates and then deletes must STILL become unreadable with ResourceNotFoundException (404) once the delete completes -- see lambda-microvms.deleted-image-name-is-released-for-reuse. The 403 verdict applies to identifiers the caller holds no grant on, not to every miss.
