Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fms.tag-resource-nonexistent-policy-arn-not-found fms.list-tags-for-resource-malformed-arn-rejected
Issue-Shape-Id: com.amazonaws.fms#AWSFMS_20180101
Issue-Key: fms.tag-resource-nonexistent-policy-arn-not-found|run#6|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: fms.list-tags-for-resource-malformed-arn-rejected|run#5|AssertionError|assert malformed_code == "ValidationException", f"expected ValidationException, got {malformed_code}"
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# TagResource and ListTagsForResource refuse with AccessDeniedException: the administrator gate is applied to tagging operations AWS answers ungated

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
REOPENS 20261024T012024Z-fms-tagresource-error-response.md: its fix left the call refused.

That issue asked for the Firewall Manager administrator gate on TagResource; the gate is now the first thing the tagging handlers do, so with no administrator association every tag write and every tag read is refused with an error code those operations do not model, instead of the ARN-shaped answer AWS gives. The still-open issue 20261024T012025Z-fms-listtagsforresource-error-response.md asks for the SAME gate on ListTagsForResource (the handler already has it, at src/ops/list_tags_for_resource.rs line 172) - that request contradicts the recorded behavior below and should be closed as invalid rather than implemented.

Cause 1 of 2 - TagResource is refused by the administrator gate instead of resolving the ARN (failing check: fms.tag-resource-nonexistent-policy-arn-not-found).

AWS: TagResource takes a generic `ResourceArn` (Smithy: com.amazonaws.fms#ResourceArn, @length 1..1024, pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$) plus a required `TagList`, and for a syntactically well-formed Firewall Manager policy ARN that names no existing policy it answers HTTP 400 `ResourceNotFoundException` (recorded message example: "The referenced item does not exist.") and creates no tag state. TagResource models InternalErrorException, InvalidInputException, InvalidOperationException, LimitExceededException, ResourceNotFoundException - it does NOT model AccessDeniedException, and the service's only common error is InternalErrorException. Recorded behavior: the recording's account has no Firewall Manager administrator configuration of its own and still receives the not-found refusal, so tag operations on a policy ARN are not gated on the caller's Firewall Manager administrator status.

Emulator: the handler runs `crate::state::require_administrator(&store, &region)?` ahead of the ARN form check and the ARN resolution, so with no admin associated it answers error code `AccessDeniedException` for every ResourceArn.

Why: a client cannot write tags at all without first associating an administrator, and the not-found path reports a code outside the operation's modeled error union, so SDK-level `ResourceNotFoundException` handling never fires and the caller cannot tell a bad ARN from an authorization failure.

Fix: the administrator gate's operation coverage in src/ops/tag_resource.rs handle - drop the `require_administrator` call so the answer is decided by the ARN resolution (`tagged_resource_for_arn` / `tags_for_arn`) and the no-side-effect not-found path already present.

Cause 2 of 2 - ListTagsForResource is refused by the administrator gate instead of validating the ARN form and resolving it (failing check: fms.list-tags-for-resource-malformed-arn-rejected).

AWS: ListTagsForResource's `ResourceArn` pattern technically accepts a string that is not an ARN, and the service's generic request-shape validator refuses such a string BEFORE any resource lookup with HTTP 400 `ValidationException`; the rule is built from the request (it names the offending value and why it is not an ARN), recorded message example: "Incorrect arn format. error:ARNs must start with 'arn:': this-is-not-an-arn-at-all". A well-formed FMS policy ARN naming nothing answers HTTP 400 `ResourceNotFoundException` (recorded message example: "The referenced item does not exist."). ListTagsForResource models InternalErrorException, InvalidInputException, InvalidOperationException, ResourceNotFoundException - not AccessDeniedException. Recorded behavior: both answers are given without any Firewall Manager administrator association.

Emulator: src/ops/list_tags_for_resource.rs calls `require_administrator` at line 172, ahead of the `arn_format_error` check at line 179 and the `tags_for_arn` lookup at line 190, so with no admin associated every request - malformed ARN or well-formed ARN alike - answers `AccessDeniedException`.

Why: tags written through the API can never be read back in a default-state emulator, and a caller validating its ARN handling receives an unmodeled authorization code instead of the ValidationException/ResourceNotFoundException distinction AWS draws.

Fix: the administrator gate's operation coverage in src/ops/list_tags_for_resource.rs handle - drop the `require_administrator` call, leaving the existing order of ARN-form validation (ValidationException) then resolution (ResourceNotFoundException) intact.

Traces: fms.tag-resource-nonexistent-policy-arn-not-found; fms.list-tags-for-resource-malformed-arn-rejected.

## Evidence
Check fms.tag-resource-nonexistent-policy-arn-not-found: AssertionError: expected ResourceNotFoundException, got AccessDeniedException (fms.tag_resource(ResourceArn="arn:aws:fms:<region>:<account>:policy/<uuid>", TagList=[{Key:k,Value:v}])). Expected HTTP 400 ResourceNotFoundException (message example "The referenced item does not exist.") and no tag state created.
Check fms.list-tags-for-resource-malformed-arn-rejected: AssertionError: expected ValidationException, got AccessDeniedException (fms.list_tags_for_resource(ResourceArn="this-is-not-an-arn-at-all")). Expected HTTP 400 ValidationException whose message names the value and why it is not an ARN (example "Incorrect arn format. error:ARNs must start with 'arn:': this-is-not-an-arn-at-all"), and HTTP 400 ResourceNotFoundException for a well-formed FMS policy ARN naming nothing.
AccessDeniedException is not a modeled error of either operation (TagResource: InternalError/InvalidInput/InvalidOperation/LimitExceeded/ResourceNotFound; ListTagsForResource: InternalError/InvalidInput/InvalidOperation/ResourceNotFound; service common_errors = [InternalErrorException]).

## Steps to Reproduce
1. Boot the emulator and do NOT call AssociateAdminAccount / PutAdminAccount.
2. Call fms TagResource {"ResourceArn":"arn:aws:fms:us-east-1:000000000000:policy/11111111-2222-3333-4444-555555555555","TagList":[{"Key":"k","Value":"v"}]}. Observed: 400 AccessDeniedException. Expected: 400 ResourceNotFoundException.
3. Call fms ListTagsForResource {"ResourceArn":"this-is-not-an-arn-at-all"}. Observed: 400 AccessDeniedException. Expected: 400 ValidationException naming the offending value.
4. Call fms ListTagsForResource with the well-formed policy ARN from step 2. Observed: 400 AccessDeniedException. Expected: 400 ResourceNotFoundException.
