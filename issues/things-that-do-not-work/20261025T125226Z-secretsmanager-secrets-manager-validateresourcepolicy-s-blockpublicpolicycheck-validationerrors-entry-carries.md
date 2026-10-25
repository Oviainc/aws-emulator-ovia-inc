Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.validate-resource-policy-reports-validation-errors
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.validate-resource-policy-reports-validation-errors|run#15|AssertionError|assert public_reasons == [
Issue-Commit: 4051ebf28f0bd776eb1ce6243425fd1600c846dd

# Secrets Manager: ValidateResourcePolicy's BlockPublicPolicyCheck ValidationErrors entry carries emulator prose instead of the service's fixed ErrorMessage

> _Note: this issue was identified against the code base at commit `4051ebf28f0bd776eb1ce6243425fd1600c846dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidateResourcePolicy

## Description
One net-new root cause in the Secrets Manager emulator, grounded in one failing behavioral check. Cause count: 1. This cause must be closed before submitting.

Related open issue, named so it is not re-discovered as the same work: 20261025T031105Z-secretsmanager-secrets-manager-resource-based-policies-and-the-blockpublicpolicy.md. Its Cause 6 covers the message constants on the PUT path (PutResourcePolicy's PublicPolicyException and MalformedPolicyDocumentException), and it does not carry this check's replay key. The divergence below is on a different operation (ValidateResourcePolicy), in a different field (the reported ValidationErrors[].ErrorMessage of a 200 response, not an exception message), and emits a third, distinct string, so fixing the PutResourcePolicy constants does not close it.

=== Cause 1 of 1: the reported BlockPublicPolicyCheck ErrorMessage is locally authored prose, not the service's fixed text ===
AWS: ValidateResourcePolicy is a pure validator with two channels. A document that is not a policy is refused with MalformedPolicyDocumentException; a document that IS a valid policy but grants broad access is answered with HTTP 200, PolicyValidationPassed=false and a non-empty ValidationErrors list, one entry per failed check. For a statement whose Principal is the wildcard "*", the failed check is named BlockPublicPolicyCheck and its ErrorMessage is the service's constant string "You can not attach a resource policy that grants Public access to your secret." — the whole message is fixed text, with NO variable parts interpolated from the request or from state (no secret id, name, ARN, Sid or principal appears in it). It is the same constant the service uses for the PutResourcePolicy PublicPolicyException refusal of the same condition. Smithy: ValidateResourcePolicy output is PolicyValidationPassed (BooleanType, default false, "True if your policy passes validation, otherwise false") and ValidationErrors (ValidationErrorsType, "Validation errors if your policy didn't pass validation"), whose entries carry CheckName and ErrorMessage; the model documents the three checks (Zelkova broad-access reasoning, syntax, caller lock-out) but does not state the message text, so the ErrorMessage string itself is recorded behavior only, asserted byte-for-byte by the replayed check against real AWS.
Emulator: status 200, PolicyValidationPassed=false and CheckName="BlockPublicPolicyCheck" are all correct; only the text diverges. The entry's ErrorMessage is "This resource policy grants broad access to your secret." The replay fails with: AssertionError: unexpected BlockPublicPolicyCheck ErrorMessage: ['This resource policy grants broad access to your secret.']. The string is produced by crate::state::resource_policy_check_failures and mapped straight into ValidationErrorsEntry.error_message by the handler, so the wrong constant is the only defect; note it is also a THIRD distinct string, different from the emulator's PutResourcePolicy PublicPolicyException text recorded in the related issue above, so both sites hold their own local prose.
Why: ValidateResourcePolicy exists to be used as a pre-flight guard before PutResourcePolicy. A client that reads ValidationErrors[].ErrorMessage to report, log or match on WHY a policy was rejected — and any regression suite pinned to the service's constant text, as the blessed check is — sees a finding it cannot identify, and cannot correlate the validator's reported reason with the refusal text PutResourcePolicy will later return for the identical policy. The validation DECISION and the CheckName are already right, which is why this is LOW.
Fix: the ValidateResourcePolicy handler's check-failure message constants — services/service-com-amazonaws-secretsmanager-secretsmanager/src/state.rs (resource_policy_check_failures, the broad-access/BlockPublicPolicyCheck entry), surfaced through services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/validate_resource_policy.rs:237-244. Emit the service's fixed string for the BlockPublicPolicyCheck entry instead of locally authored prose, and use the same constant the PutResourcePolicy PublicPolicyException path must emit.
Traces: secretsmanager.validate-resource-policy-reports-validation-errors.

Not asserted by this run and therefore NOT part of this issue: the same check's later assertions on the throwing channel (MalformedPolicyDocumentException "This resource policy contains a syntax error." for valid JSON with no Statement, and "This resource policy contains an unsupported principal." for a principal naming an account that does not resolve) were never reached, because the run aborted at the ErrorMessage assertion. They may or may not diverge; re-replaying the check after fixing the constant is what reveals that.

## Evidence
Failing check secretsmanager.validate-resource-policy-reports-validation-errors, verdict mismatch, at check_secretsmanager_validate_resource_policy_reports_validation_errors.py line 127:

  assert public_reasons == [
      "You can not attach a resource policy that grants Public access to your secret."
  ], f"unexpected BlockPublicPolicyCheck ErrorMessage: {public_reasons!r}"
  AssertionError: unexpected BlockPublicPolicyCheck ErrorMessage: ['This resource policy grants broad access to your secret.']

Expected (real AWS, recorded): HTTP 200; PolicyValidationPassed=false; ValidationErrors=[{CheckName: "BlockPublicPolicyCheck", ErrorMessage: "You can not attach a resource policy that grants Public access to your secret."}]. The ErrorMessage is entirely fixed text — no request- or state-derived values.
Actual (emulator): HTTP 200; PolicyValidationPassed=false; ValidationErrors=[{CheckName: "BlockPublicPolicyCheck", ErrorMessage: "This resource policy grants broad access to your secret."}].

Everything the check asserted before line 127 PASSED against the emulator, which localises the divergence to the message constant alone: the scoped single-account-principal policy validated with PolicyValidationPassed=true and ValidationErrors=[]; the wildcard-principal policy came back with PolicyValidationPassed=false; the entry was non-empty and carried both a CheckName and an ErrorMessage; and the CheckName was "BlockPublicPolicyCheck".

## Steps to Reproduce
1. Call secretsmanager ValidateResourcePolicy with no SecretId and ResourcePolicy = a syntactically valid policy naming the caller's own account root as principal, e.g. {"Version":"2012-10-17","Statement":[{"Sid":"ScopedRead","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<caller account>:root"},"Action":"secretsmanager:GetSecretValue","Resource":"*"}]}. Observe PolicyValidationPassed=true with ValidationErrors=[] (correct).
2. Call ValidateResourcePolicy again with the same document but Principal set to the wildcard "*": {"Version":"2012-10-17","Statement":[{"Sid":"PublicRead","Effect":"Allow","Principal":"*","Action":"secretsmanager:GetSecretValue","Resource":"*"}]}.
3. Read the single ValidationErrors entry whose CheckName is "BlockPublicPolicyCheck" and compare its ErrorMessage to AWS's fixed text. The emulator returns "This resource policy grants broad access to your secret."; AWS returns "You can not attach a resource policy that grants Public access to your secret." No secret is created and no policy is attached — the operation is a pure validator, so this reproduces with zero resources.
