Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.get-microvm-image-access-denied lambda-microvms.get-microvm-image-build-access-denied lambda-microvms.managed-image-read-through-customer-api-is-rejected lambda-microvms.representation-managed-microvm-image-fields-round-trip
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms.get-microvm-image-access-denied|run#7|AssertionError|assert real_msg == f"User: {caller} is not authorized to perform this operation", real_msg
Issue-Key: lambda-microvms.get-microvm-image-build-access-denied|run#12|AssertionError|assert real_list_msg == denied, real_list_msg
Issue-Key: lambda-microvms.managed-image-read-through-customer-api-is-rejected|run#20|AssertionError|assert message == denied, message
Issue-Key: lambda-microvms|GetMicrovmImage|AccessDeniedException
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# lambda-microvms: AccessDeniedException message is the model's generic sentence instead of AWS's IAM denial message naming the caller principal

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMicrovmImage

## Description
One root cause, grounded in 4 failing behavioral checks of this suite.

=== Cause 1 of 1: every customer-facing MicroVM-image read answers AccessDeniedException with the modeled error-class sentence as its `message`, never AWS's IAM denial message ===
Failing checks: lambda-microvms.get-microvm-image-access-denied, lambda-microvms.get-microvm-image-build-access-denied, lambda-microvms.managed-image-read-through-customer-api-is-rejected, lambda-microvms.representation-managed-microvm-image-fields-round-trip

AWS: Smithy (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json) declares AccessDeniedException as a service-common error, errorKind client, httpError 403, with a single optional member `message`; its DOC string is the error-class description "You do not have sufficient access to perform this action." — a description of the class, not the wire message. It is bound to the customer-facing image reads GetMicrovmImage, ListMicrovmImageVersions, ListMicrovmImageBuilds and GetMicrovmImageBuild. The model says nothing about the message text, so the requirement is recorded behavior only, and the recordings show AWS puts an IAM-style denial sentence there, in two forms:
  (a) when the identifier names a resource the principal is simply not permitted on (an AWS-owned managed base image ARN, account segment literally `aws`): HTTP 403, code `AccessDeniedException`, message `User: <caller-arn> is not authorized to perform this operation`. Fixed text: `User: ` and ` is not authorized to perform this operation`. The single variable part is <caller-arn>, the ARN of the calling principal — the same value STS GetCallerIdentity answers in `Arn` for the credentials that signed the request. No value from the request or from one recording belongs in it.
  (b) when the identifier is well-formed but names nothing the principal was granted (e.g. `arn:aws:lambda:<region>:<account>:microvm-image/<name>` for a name nobody created): HTTP 403, code `AccessDeniedException`, message `User: <caller-arn> is not authorized to perform: lambda:<OperationName> on resource: <identifier>`. Fixed text: `User: `, ` is not authorized to perform: `, `lambda:`, ` on resource: `. Variable parts: <caller-arn> (caller identity), <OperationName> (the operation being refused — `GetMicrovmImage`, `GetMicrovmImageBuild`, …, giving the IAM action name) and <identifier> (the identifier string the request carried, verbatim).
Both forms confirm authorization is decided BEFORE existence, which the emulator already does correctly.

Emulator: the verdict, the 403 and the code are right; only the message diverges. Every denial path sets `message: Some(crate::state::ACCESS_DENIED_MESSAGE.to_string())`, and that constant is literally the model's doc sentence: `pub const ACCESS_DENIED_MESSAGE: &str = "You do not have sufficient access to perform this action.";` (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs:686, whose own comment states the decision: "AWS's own message additionally names the CALLING PRINCIPAL, which is caller identity rather than anything of this request, so the modelled sentence for the error class is the whole envelope here"). So all four checks fail with `AssertionError: You do not have sufficient access to perform this action.` where form (a) was expected. Both the existing-resource form and the bogus-identifier form come from that one constant, so neither form is produced anywhere; the checks fail at the form-(a) assertion first, and form (b) is unreachable for the same reason. Confirmed diverging on GetMicrovmImage (both the managed-ARN read and the read of an own-account-absent identifier) and ListMicrovmImageBuilds; ListMicrovmImageVersions and GetMicrovmImageBuild take the identical path with the identical constant and are asserted by the same checks after the first failure.

Why: the message is the only part of a 403 that tells a client WHICH principal was refused and, in form (b), WHICH action on WHICH resource. Operators and SDK/middleware layers parse exactly this sentence to turn a denial into an actionable IAM diagnosis ("add lambda:GetMicrovmImage for arn:…" / "you are running as the wrong role"). Against the emulator every denial is one indistinguishable sentence that names neither the principal, the action nor the resource, so permission-diagnosis tooling, test assertions on the denial text, and log-based alerting on a specific principal all break — and a caller cannot tell a permission boundary it is never meant to cross (a managed base image) from a missing grant on its own resource.

Fix: the denial message belongs in this service's error construction — the shared message builder in the service state alongside the other message templates (image_not_found_message / invalid_arn_format_message), replacing the single constant with a per-form builder, plus the call sites in the denial branch of each customer-facing read handler so each can pass its own IAM action name and the requested identifier. The caller principal must come from the request's signing identity (the same identity the emulator's STS GetCallerIdentity answers with), not be hardcoded.

Traces: lambda-microvms.get-microvm-image-access-denied (GetMicrovmImage on a managed ARN, then on a well-formed bogus ARN — asserts form (a) then form (b)); lambda-microvms.get-microvm-image-build-access-denied (ListMicrovmImageBuilds and GetMicrovmImageBuild on a managed image version, then GetMicrovmImageBuild on a bogus image ARN); lambda-microvms.managed-image-read-through-customer-api-is-rejected (GetMicrovmImage and ListMicrovmImageVersions on a managed ARN); lambda-microvms.representation-managed-microvm-image-fields-round-trip (GetMicrovmImage on a managed ARN; the item-shape assertions of that check pass). Citations: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs:680-686 (the constant and the reasoning behind it); services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image.rs:198-205 (ImageAccess::Denied -> AccessDeniedException with that constant); services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/list_microvm_image_builds.rs:193-200 (the same).

Related open issue, which does NOT cover this: 20261025T210105Z-lambda-microvms-image-builds-never-fail-image-store.md Cause 3 of 3 covers only the VERDICT for a managed base-image ARN (404 ResourceNotFoundException where AWS answers 403 AccessDeniedException) and states explicitly that "only the error CODE and the 403 status are required here; the check deliberately asserts the code only". The code and status now match; the checks here fail on the message instead, which that issue does not describe or require. The other three open lambda-microvms issues (image-build-failure-is-never-modeled, managed-microvm-image-catalog-is-synthesized, image-version-records-are-written-from-defaults) concern build outcomes, the managed catalogue's contents and version records, not the denial message.

## Evidence
Behavioral suite: 6/41 passed. 28 failures are blocked by the unimplemented GetMicrovmImageVersion stub (ALREADY FILED) and 3 are OWNED by 20261025T210105Z-lambda-microvms-image-builds-never-fail-image-store.md; the remaining 4 are the ones below and all fail identically.

Expected (AWS, recorded): HTTP 403, Error.Code=AccessDeniedException, Error.Message="User: <caller-arn> is not authorized to perform this operation" for an identifier the principal is not permitted on, and "User: <caller-arn> is not authorized to perform: lambda:<OperationName> on resource: <identifier>" for a well-formed identifier naming nothing granted. <caller-arn> is the signing principal's ARN (STS GetCallerIdentity Arn), <identifier> is the request's own imageIdentifier.
Actual (emulator): HTTP 403, Error.Code=AccessDeniedException (correct), Error.Message="You do not have sufficient access to perform this action." for every denial of every operation.

  [mismatch] lambda-microvms.get-microvm-image-access-denied
    check_lambda_microvms_get_microvm_image_access_denied.py:27
    assert real_msg == f"User: {caller} is not authorized to perform this operation", real_msg
    AssertionError: You do not have sufficient access to perform this action.
    (real_code == "AccessDeniedException" passed on the line before; GetMicrovmImage on items[0]["imageArn"] from ListManagedMicrovmImages.)

  [mismatch] lambda-microvms.get-microvm-image-build-access-denied
    check_lambda_microvms_get_microvm_image_build_access_denied.py:35
    assert real_list_msg == denied, real_list_msg
    AssertionError: You do not have sufficient access to perform this action.
    (ListMicrovmImageBuilds(imageIdentifier=<managed image arn>, imageVersion=<its first version>); the code assertion passed. The check then asserts the same message for GetMicrovmImageBuild and form (b) for a bogus image ARN.)

  [mismatch] lambda-microvms.managed-image-read-through-customer-api-is-rejected
    check_lambda_microvms_managed_image_read_through_customer_api_is_rejected.py:70
    assert message == denied, message
    AssertionError: You do not have sufficient access to perform this action.
    (GetMicrovmImage on a managed ARN; the code assertion and all the preceding namespace-disjointness assertions passed. ListMicrovmImageVersions on the same ARN is asserted next with the same message.)

  [mismatch] lambda-microvms.representation-managed-microvm-image-fields-round-trip
    check_lambda_microvms_representation_managed_microvm_image_fields_round_trip.py:58
    assert message == f"User: {caller} is not authorized to perform this operation", message
    AssertionError: You do not have sufficient access to perform this action.
    (exc.response["Error"]["Code"] == "AccessDeniedException" passed; the managed-ARN pattern and createdAt/updatedAt timestamp assertions of this check also passed.)

Source grounding:
  services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs:686
    pub const ACCESS_DENIED_MESSAGE: &str = "You do not have sufficient access to perform this action.";
  services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image.rs:198-205
    ImageAccess::Denied => AccessDeniedException { message: Some(crate::state::ACCESS_DENIED_MESSAGE.to_string()) }
  services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/list_microvm_image_builds.rs:193-200
    ImageReference::Absent => AccessDeniedException { message: Some(crate::state::ACCESS_DENIED_MESSAGE.to_string()) }

Model grounding: smithy_query(query="shape", shape="com.amazonaws.lambdamicrovms#AccessDeniedException") -> {errorKind: client, httpError: 403, members: {message: String}, doc: "You do not have sufficient access to perform this action."} — the sentence is the class's documentation; the model constrains neither the message text nor its parts.

## Steps to Reproduce
1. mvm = client('lambda-microvms'); caller = client('sts').get_caller_identity()['Arn'].
2. arn = mvm.list_managed_microvm_images()['items'][0]['imageArn']  (account segment is 'aws').
3. mvm.get_microvm_image(imageIdentifier=arn).
   Emulator: 403 AccessDeniedException, Message="You do not have sufficient access to perform this action."
   AWS:      403 AccessDeniedException, Message=f"User: {caller} is not authorized to perform this operation"
4. version = mvm.list_managed_microvm_image_versions(imageIdentifier=arn)['items'][0]['imageVersion'];
   mvm.list_microvm_image_builds(imageIdentifier=arn, imageVersion=version) and
   mvm.get_microvm_image_build(imageIdentifier=arn, imageVersion=version, buildId=<any>) and
   mvm.list_microvm_image_versions(imageIdentifier=arn) — same expected/actual divergence as step 3.
5. bogus = f"arn:aws:lambda:{region}:{account}:microvm-image/<a-name-nobody-created>";
   mvm.get_microvm_image(imageIdentifier=bogus).
   Emulator: 403 AccessDeniedException, Message="You do not have sufficient access to perform this action."
   AWS:      403 AccessDeniedException, Message=f"User: {caller} is not authorized to perform: lambda:GetMicrovmImage on resource: {bogus}"
   (and mvm.get_microvm_image_build(imageIdentifier=bogus, imageVersion='1.0', buildId='build-does-not-exist') -> "... is not authorized to perform: lambda:GetMicrovmImageBuild on resource: {bogus}").
