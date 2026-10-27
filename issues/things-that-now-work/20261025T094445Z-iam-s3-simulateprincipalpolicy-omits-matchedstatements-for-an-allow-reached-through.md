Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__s3.sep-policy-composition-s3-identity-or-bucket-policy-each-suffice-denied-with-neither
Issue-Key: iam__s3.sep-policy-composition-s3-identity-or-bucket-policy-each-suffice-denied-with-neither|run#22|AssertionError|assert (s_ident0["EvalDecision"], _types(s_ident0)) == ("allowed", ["IAM Policy"]), f"{s_ident0!r}"
Issue-Commit: 3bc9be6f406ac9203ca7b4af142bb62bbe4e14d6

# SimulatePrincipalPolicy omits MatchedStatements for an allow reached through the principal's own identity policy, so the S3 policy-composition view is unexplainable (1 cause)

> _Note: this issue was identified against the code base at commit `3bc9be6f406ac9203ca7b4af142bb62bbe4e14d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SimulatePrincipalPolicy

## Description
One net-new root cause, grounded in the failing check iam__s3.sep-policy-composition-s3-identity-or-bucket-policy-each-suffice-denied-with-neither. The replay report marks this check neither ALREADY FILED nor OWNED, so no open issue carries its replay key.

SAME ROOT CAUSE as cause 2 of the open issue 20261025T085808Z-iam-kms-simulateprincipalpolicy-ignores-the-simulated-principal-when-evaluating-resourcepolicy.md, which observed it from the iam__kms leg (Issue-Trace-Ids: iam__kms.sep-policy-composition-kms-key-policy-required-identity-policy-alone-denied) and does not list this check. Filed so the iam__s3 leg is bound to a work item and gets re-replayed; a fixer closing that issue should re-replay this check too, and may close this one with the same change. The open stub issue 20261022T131025Z-iam-simulateprincipalpolicy-stub.md no longer applies: the operation answered HTTP 200 with EvaluationResults in this replay.

Cause 1 of 1: EvaluationResult.MatchedStatements is absent from every result, including the simplest case — an `allowed` decision produced by the simulated principal's OWN attached identity policy, with no ResourcePolicy supplied at all.
AWS: Smithy (com.amazonaws.iam) EvaluationResult.MatchedStatements (StatementListType) is documented 'A list of the statements in the input policies that determine the result for this scenario ... Only statements that contributed to the reported decision are included', and each entry is a `Statement` carrying SourcePolicyId, SourcePolicyType (PolicySourceType), StartPosition and EndPosition; ResourceSpecificResult carries the per-resource equivalent. EvalDecision targets PolicyEvaluationDecisionType (allowed / explicitDeny / implicitDeny). Recorded behavior (this check's oracle): for PolicySourceArn = an IAM user whose inline identity policy Allows s3:GetObject on arn:aws:s3:::<bucket>/*, ActionNames ['s3:GetObject'] and ResourceArns [<object ARN>] and NO ResourcePolicy, AWS answers EvalDecision 'allowed' AND a MatchedStatements entry whose SourcePolicyType is the identity-policy source — the recorded value the check reads is 'IAM Policy'. The same check's second leg records 'Resource Policy' for the user allowed only by the bucket policy supplied as ResourcePolicy, i.e. the member is the discriminator between the two grant paths.
Emulator: EvalDecision is correct ('allowed') but the result carries no MatchedStatements member at all, on the top-level EvaluationResult and on ResourceSpecificResults alike. The observed object is {'EvalActionName': 's3:GetObject', 'EvalResourceName': 'arn:aws:s3:::<bucket>/composition/probe.bin', 'EvalDecision': 'allowed', 'ResourceSpecificResults': [{'EvalResourceName': '<same>', 'EvalResourceDecision': 'allowed'}]}, so the set of SourcePolicyType values the check derives is empty.
Why: MatchedStatements is the only part of the response that explains a decision. The S3 union this check grades — a same-account principal authorized by EITHER its identity policy OR the bucket policy — is indistinguishable without it: a caller cannot tell an allow granted by its own IAM policy from one granted by the bucket policy, which is exactly the question a least-privilege review, a policy-explanation UI, a CI permissions gate or a drift detector asks before shipping. Omitting the member because statement source POSITIONS are untracked also drops SourcePolicyId and SourcePolicyType, neither of which needs position tracking.
Fix: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/simulate_principal_policy.rs together with the shared evaluator in service_common::iam_authorization — the evaluator must report which document contributed to the decision so the handler can emit MatchedStatements with SourcePolicyId and SourcePolicyType on both the top-level EvaluationResult and each ResourceSpecificResult, identifying an identity-policy allow distinctly from a resource-policy allow. StartPosition/EndPosition may legitimately stay absent if positions are not measured; the source identification may not.
Traces: iam__s3.sep-policy-composition-s3-identity-or-bucket-policy-each-suffice-denied-with-neither.

## Evidence
Failing check: [mismatch] iam__s3.sep-policy-composition-s3-identity-or-bucket-policy-each-suffice-denied-with-neither

Frame (check_iam__s3_sep_policy_composition_s3_identity_or_bucket_policy_each_suffice_denied_with_neither.py, line 194):
    assert (s_ident0["EvalDecision"], _types(s_ident0)) == ("allowed", ["IAM Policy"]), f"{s_ident0!r}"
AssertionError: {'EvalActionName': 's3:GetObject', 'EvalResourceName': 'arn:aws:s3:::sim2real-sep-s3-37d7b2ab5501/composition/probe.bin', 'EvalDecision': 'allowed', 'ResourceSpecificResults': [{'EvalResourceName': 'arn:aws:s3:::sim2real-sep-s3-37d7b2ab5501/composition/probe.bin', 'EvalResourceDecision': 'allowed'}]}

where _types(result) = sorted({m.get('SourcePolicyType') for m in result.get('MatchedStatements', [])}).

Expected (real AWS): HTTP 200, EvalDecision 'allowed' AND MatchedStatements present with SourcePolicyType identifying the identity policy (recorded value 'IAM Policy').
Actual (emulator): HTTP 200, EvalDecision 'allowed' (correct) and MatchedStatements absent entirely, so the derived SourcePolicyType set is empty.

The operation is not a scaffold stub in this replay: it answered 200 with EvaluationResults, and the report did not mark this check as blocked by an unimplemented operation. The later legs of the check (the bucket-policy-only user's 'Resource Policy' match, the implicitDeny for the user granted by neither, and the simulator-vs-S3 agreement comparison) were never reached and remain unverified.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. s3:CreateBucket B and s3:PutObject B/composition/probe.bin with any body.
3. iam:CreateUser U and iam:PutUserPolicy U with {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::B/*"}]}; note U's ARN. Supply NO bucket policy.
4. Call iam:SimulatePrincipalPolicy with PolicySourceArn=<U's ARN>, ActionNames=['s3:GetObject'], ResourceArns=['arn:aws:s3:::B/composition/probe.bin'] and no ResourcePolicy.
5. Observe EvaluationResults[0].EvalDecision == 'allowed' (correct) but no MatchedStatements member on the result or on ResourceSpecificResults[0]; AWS returns a MatchedStatements entry whose SourcePolicyType identifies the identity policy ('IAM Policy' as recorded).
6. Then s3:PutBucketPolicy on B allowing s3:GetObject to a second user V that has no identity policy, and re-simulate with PolicySourceArn=<V's ARN> and ResourcePolicy set to that bucket policy: the allow must come back with SourcePolicyType identifying the resource policy ('Resource Policy' as recorded), so the two grant paths are distinguishable.
7. Re-replay iam__s3.sep-policy-composition-s3-identity-or-bucket-policy-each-suffice-denied-with-neither to confirm the fix; its later legs (bucket-policy-only read, the neither-policy AccessDenied with S3's exact implicit-deny message, and the simulator/enforcement agreement check) must also pass.
