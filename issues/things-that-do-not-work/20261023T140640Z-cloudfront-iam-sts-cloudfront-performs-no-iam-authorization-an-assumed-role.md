Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__iam__sts.invalidation-role-is-denied-reading-back-its-own-invalidation cloudfront__iam__sts.invalidation-role-scoped-to-another-distribution-is-denied
Issue-Key: cloudfront__iam__sts.invalidation-role-is-denied-reading-back-its-own-invalidation|_error#14|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudfront__iam__sts.invalidation-role-scoped-to-another-distribution-is-denied|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CloudFront performs no IAM authorization: an assumed role with no matching grant can create and read invalidations

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvalidation

## Description
Cause 1 of 1: the CloudFront control plane does not evaluate the caller's IAM identity policy, so calls that real AWS denies succeed.
AWS: Smithy: CreateInvalidation models AccessDenied with httpError 403, as do GetInvalidation/ListInvalidations/GetDistributionConfig. For an assumed-role caller whose identity policy does not allow the action (or allows it only on another distribution's ARN), AWS answers 403 AccessDenied and the message is built from the request — "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: cloudfront:<Action> on resource: <resource ARN> because no identity-based policy allows the cloudfront:<Action> action" — so the message rule, not one recorded string, is the requirement.
Emulator: every such call succeeds. A role granted only CreateInvalidation/ListDistributions/GetDistribution is still allowed to read its invalidation back (GetInvalidation, ListInvalidations) and to read GetDistributionConfig; a role whose CreateInvalidation grant is scoped to one distribution ARN can still invalidate a sibling distribution. The checks fail at the point where they expect an error and there is none.
Why: authorization is unverifiable — a least-privilege policy cannot be tested, an over-broad or mis-scoped policy looks correct, and an agent or CI gate that proves 'this role cannot do X' gets a false pass for every X.
Fix: identity-policy evaluation on the CloudFront request path (resolve the SigV4 caller / assumed-role session, evaluate action and resource ARN against the role's policies, and emit the 403 AccessDenied envelope), in the cloudfront service crate's request authorization layer rather than in the individual handlers.
Traces: cloudfront__iam__sts.invalidation-role-is-denied-reading-back-its-own-invalidation, cloudfront__iam__sts.invalidation-role-scoped-to-another-distribution-is-denied.

## Evidence
Both checks fail with "AssertionError: the call succeeded; real AWS rejects it, so there is no error code" — raised by the checks' _error() helper when the call it expected to be denied returns successfully. The second check's expected value is ("AccessDenied", "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: cloudfront:CreateInvalidation on resource: <sibling distribution ARN> because no identity-based policy allows the cloudfront:CreateInvalidation action"). The roles are created in-check: the first grants exactly cloudfront:CreateInvalidation, ListDistributions, GetDistribution on '*'; the second grants cloudfront:CreateInvalidation on one distribution's ARN only.

## Steps to Reproduce
1. CreateRole assumable by the account root with an inline policy allowing exactly cloudfront:CreateInvalidation, cloudfront:ListDistributions, cloudfront:GetDistribution on '*'; AssumeRole into it.
2. Create a distribution, then as the role call CreateInvalidation (expect success), then GetInvalidation / ListInvalidations / GetDistributionConfig -> expect 403 AccessDenied; observe success.
3. Create two distributions A and B and a role whose only grant is cloudfront:CreateInvalidation on A's ARN; as that role call CreateInvalidation on B -> expect 403 AccessDenied with the 'not authorized to perform' message naming B's ARN; observe a created invalidation.
