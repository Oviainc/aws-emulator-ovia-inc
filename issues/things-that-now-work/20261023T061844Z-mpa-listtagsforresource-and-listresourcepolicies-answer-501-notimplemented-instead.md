Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mpa.approval-team-fails-validation-for-unknown-approver
Issue-Shape-Id: com.amazonaws.mpa#AWSFluffyCoreService
Issue-Key: mpa.approval-team-fails-validation-for-unknown-approver|run#4|AssertionError|assert list_tags_code == "ValidationException", list_tags_code
Issue-Commit: 1bf7f94f5369037d1be2743fe86a59b83a69434a

# mpa: ListTagsForResource (and ListResourcePolicies) answer 501 NotImplemented instead of rejecting a malformed ResourceArn with 400 ValidationException

> _Note: this issue was identified against the code base at commit `1bf7f94f5369037d1be2743fe86a59b83a69434a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Cause 1 of 1: the resource-ARN-keyed tag/policy read operations of MPA are still scaffold stubs, so the service never reaches the ARN-validation boundary that real MPA enforces on ResourceArn. Failing check ids: mpa.approval-team-fails-validation-for-unknown-approver.

AWS: ListTagsForResource takes ResourceArn as an httpLabel member with NO pattern constraint, only @length max 1000 (Smithy: ListTagsForResource.ResourceArn target String, httpLabel, required, length max 1000; http GET /tags/{ResourceArn}), so a syntactically malformed value such as "not-an-arn-at-all" satisfies every modeled constraint and is refused by the operation's own validation instead. The operation models ValidationException with httpError 400 ("The input fails to satisfy the constraints specified by an Amazon Web Services service.") and, separately, ResourceNotFoundException with httpError 404 for a well-formed-but-nonexistent ARN — two distinct outcomes the check distinguishes. Recorded behavior: real AWS answers HTTP 400 with error code ValidationException for the malformed ARN (recorded message example: "Invalid Resource Arn"; the text is an example, the 400/ValidationException pair is the requirement). ListResourcePolicies has the identical ResourceArn member and the identical modeled error set (Smithy: ListResourcePolicies.ResourceArn httpLabel, required, length max 1000; ValidationException httpError 400), so the same boundary applies there.

Emulator: the call is answered with HTTP 501 and error code NotImplemented, message "operation ListTagsForResource is scaffolded but not implemented in this emulator". The check asserted the error code and got "NotImplemented" (AssertionError: NotImplemented), so nothing about ARN validation is exercised at all. ListResourcePolicies, probed next by the same check, is in the same state (its handler still carries the SCAFFOLDED marker and returns Scaffolded::not_implemented). Note: the third probe in that same check, GetResourcePolicy, is covered by its own already-filed stub issue and is NOT part of this one; this issue is about ListTagsForResource and ListResourcePolicies.

Why: a client cannot read tags or resource policies for any MPA resource at all, and error-handling code is misled twice over — a 501/NotImplemented is not retried, not mapped to an input error, and is indistinguishable in client logic from a broken endpoint, whereas the real 400 ValidationException tells the caller its ARN is malformed and the real 404 ResourceNotFoundException tells it the ARN is well formed but unknown. Any caller that branches on ValidationException vs ResourceNotFoundException to tell "bad input" from "missing resource" takes the wrong branch.

Fix: implement the two operation handlers and their ARN validation: reject a ResourceArn that is not a well-formed MPA ARN with the modeled ValidationException (400) before any state lookup, and return the modeled ResourceNotFoundException (404) for a well-formed ARN naming no existing resource; then back ListTagsForResource/ListResourcePolicies with the service's tag and resource-policy state. Files to touch (citations only): services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/list_tags_for_resource.rs line 162 (Err(Scaffolded::not_implemented("ListTagsForResource")), SCAFFOLDED marker at line 4) and services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/list_resource_policies.rs (same SCAFFOLDED marker at line 4).

Traces: mpa.approval-team-fails-validation-for-unknown-approver (ListTagsForResource probe with ResourceArn="not-an-arn-at-all").

## Evidence
Check mpa.approval-team-fails-validation-for-unknown-approver, verdict mismatch:

  mpa = ctx.client("mpa", region="us-east-1")
  mpa.list_tags_for_resource(ResourceArn="not-an-arn-at-all")

Expected (real AWS): HTTP 400, Error.Code = "ValidationException" (recorded message example: "Invalid Resource Arn").
Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Message = "operation ListTagsForResource is scaffolded but not implemented in this emulator".
Assertion: `assert list_tags_code == "ValidationException"` -> AssertionError: NotImplemented

Model grounding (smithy_query, services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json):
  ListTagsForResource: input ResourceArn {target String, httpLabel, required, length max 1000} (no pattern); errors include ValidationException (httpError 400) and ResourceNotFoundException (httpError 404); http GET /tags/{ResourceArn}.
  ListResourcePolicies: input ResourceArn {target String, httpLabel, required, length max 1000}; same error set; http POST /resource-policies/{ResourceArn}/?List.

Source grounding: list_tags_for_resource.rs:162 returns crate::scaffolded::Scaffolded::not_implemented("ListTagsForResource"); list_resource_policies.rs:4 still carries "SCAFFOLDED, NOT IMPLEMENTED".

## Steps to Reproduce
1. Boot the emulator and point a botocore mpa client at it in us-east-1 (MPA is single-Region).
2. Call ListTagsForResource with a syntactically malformed ARN:
     mpa.list_tags_for_resource(ResourceArn="not-an-arn-at-all")
   Observe HTTP 501 / Error.Code "NotImplemented" instead of HTTP 400 / "ValidationException".
3. Call ListResourcePolicies with the same malformed ARN:
     mpa.list_resource_policies(ResourceArn="not-an-arn-at-all")
   Observe the same 501 / "NotImplemented".
4. Re-replay check mpa.approval-team-fails-validation-for-unknown-approver to confirm the fix.
