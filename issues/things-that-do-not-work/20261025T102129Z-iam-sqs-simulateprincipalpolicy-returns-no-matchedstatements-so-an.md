Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__sqs.sep-policy-composition-sqs-identity-or-queue-policy-each-suffice-denied-with-neither
Issue-Key: iam__sqs.sep-policy-composition-sqs-identity-or-queue-policy-each-suffice-denied-with-neither|run#15|AssertionError|assert (s_ident["EvalDecision"], _types(s_ident)) == ("allowed", ["IAM Policy"]), f"{s_ident!r}"
Issue-Commit: ea7cea25a1e6705e0f44642ae0567838786a0067

# iam__sqs: SimulatePrincipalPolicy returns no MatchedStatements, so an allow cannot be attributed to the IAM policy vs the queue policy that produced it

> _Note: this issue was identified against the code base at commit `ea7cea25a1e6705e0f44642ae0567838786a0067`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SimulatePrincipalPolicy

## Description
One root cause.

Cause 1 of 1: SimulatePrincipalPolicy decides correctly but never reports MatchedStatements, so a caller cannot tell whether an allow came from the principal's identity policy or from the resource-based policy supplied as ResourcePolicy.

AWS: Smithy (com.amazonaws.iam) SimulatePrincipalPolicy returns SimulatePolicyResponse{EvaluationResults}; each EvaluationResult carries MatchedStatements (StatementListType) next to EvalDecision, and each Statement carries SourcePolicyId and SourcePolicyType. For an sqs:SendMessage simulation with ResourcePolicy set to the queue policy, AWS reports `allowed` with MatchedStatements whose SourcePolicyType is `IAM Policy` when the grant came from the user's identity policy, and `Resource Policy` when it came from the queue policy; a principal with neither is `implicitDeny` and matches nothing (recorded behavior for the attribution values; the model states the members).

Emulator: the member is hard-coded absent. services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/simulate_principal_policy.rs sets `matched_statements: None` on the EvaluationResult (and again on each ResourceSpecificResult), with the comment that the shared evaluator does not track statement source positions. The response therefore carries EvalDecision `allowed` and ResourceSpecificResults but no MatchedStatements key at all, so the set of SourcePolicyType values a caller can read is empty for both the identity-only and the resource-policy-only principal.

Why: the two cases a client is trying to distinguish — "the queue is open to this principal" versus "this principal holds an identity grant" — become indistinguishable, even though both are reported as allowed. Tooling that reviews an SQS queue's exposure, or that asserts a grant arrives through the intended document, reads an empty list and concludes no policy is responsible; removing either document would then look safe when it is not.

Fix: the operation's handler — services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/simulate_principal_policy.rs — must populate MatchedStatements (SourcePolicyId plus SourcePolicyType, distinguishing the identity documents from the request's ResourcePolicy), which requires the shared evaluator in services/common/src/iam_authorization.rs to report which statement of which document matched rather than only the Decision.

Traces: iam__sqs.sep-policy-composition-sqs-identity-or-queue-policy-each-suffice-denied-with-neither. Two open issues name the same omission under other service tokens and do NOT carry this check's replay key: 20261025T094445Z-iam-s3-simulateprincipalpolicy-omits-matchedstatements-for-an-allow-reached-through.md and 20261025T085808Z-iam-kms-simulateprincipalpolicy-ignores-the-simulated-principal-when-evaluating-resourcepolicy.md.

## Evidence
Replay of `iam__sqs.sep-policy-composition-sqs-identity-or-queue-policy-each-suffice-denied-with-neither`:

  #### [mismatch] iam__sqs.sep-policy-composition-sqs-identity-or-queue-policy-each-suffice-denied-with-neither
  AssertionError: {'EvalActionName': 'sqs:SendMessage', 'EvalResourceName': 'arn:aws:sqs:<region>:<account>:<queue>', 'EvalDecision': 'allowed', 'ResourceSpecificResults': [{'EvalResourceName': 'arn:aws:sqs:<region>:<account>:<queue>', 'EvalResourceDecision': 'allowed'}]}

The failing assertion is:

  assert (s_ident["EvalDecision"], _types(s_ident)) == ("allowed", ["IAM Policy"])

with

  def _types(result):
      return sorted({m.get("SourcePolicyType") for m in result.get("MatchedStatements", [])})

so `_types` evaluated to `[]` because MatchedStatements is absent. The next assertion requires `["Resource Policy"]` for the principal that holds only the queue-policy grant, and would fail identically.

Expected (AWS): EvalDecision `allowed` with MatchedStatements present — SourcePolicyType `IAM Policy` for the identity-policy-only user, `Resource Policy` for the queue-policy-only user; `implicitDeny` with no matched statements for the user with neither.
Actual (emulator): EvalDecision `allowed` for both, MatchedStatements omitted entirely. Source: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/simulate_principal_policy.rs line 629 `matched_statements: None,` (EvaluationResult) and line 829 `matched_statements: None,` (ResourceSpecificResult).

The decision legs of the check passed (the identity-only and resource-only principals are both `allowed`, which is what AWS reports), so the divergence is confined to the attribution members.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (both `iam` and `sqs` are bootstrapped and serving).
2. CreateQueue and capture its ARN.
3. Create three IAM users: (a) one with an inline identity policy `Allow sqs:SendMessage on <queue arn>`; (b) one with no identity policy, named as the Principal in the queue's own policy document `Allow sqs:SendMessage`; (c) one with neither.
4. For user (a), call SimulatePrincipalPolicy(PolicySourceArn=<user a arn>, ActionNames=["sqs:SendMessage"], ResourceArns=[<queue arn>], ResourcePolicy=<the queue policy JSON>). Observe EvalDecision `allowed` and NO MatchedStatements member; AWS returns MatchedStatements with SourcePolicyType `IAM Policy`.
5. Repeat for user (b): EvalDecision `allowed` with no MatchedStatements; AWS returns SourcePolicyType `Resource Policy`.
6. Repeat for user (c): `implicitDeny` (this leg already matches AWS).
7. Equivalently: replay the behavioral check `iam__sqs.sep-policy-composition-sqs-identity-or-queue-policy-each-suffice-denied-with-neither`.
