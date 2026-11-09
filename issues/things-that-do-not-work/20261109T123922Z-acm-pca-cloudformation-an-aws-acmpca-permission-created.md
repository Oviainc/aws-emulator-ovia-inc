Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca__cloudformation.deploy-cfn-acmpca-permission-invalid-principal-rejected acm-pca__cloudformation.deploy-cfn-acmpca-permission-provisions-and-revokes
Issue-Key: acm-pca__cloudformation.deploy-cfn-acmpca-permission-invalid-principal-rejected|run#11|AssertionError|assert got == [(_VALID_PRINCIPAL, sorted(_ACTIONS))], f"control CA permissions {got!r}"
Issue-Key: acm-pca__cloudformation.deploy-cfn-acmpca-permission-provisions-and-revokes|run#19|AssertionError|assert out["PermissionRef"] == f"{ca_arn}|{_PRINCIPAL}", (
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# acm-pca__cloudformation: an AWS::ACMPCA::Permission created by a stack is never written to ACM PCA state, and Ref returns a synthesized CloudFormation name

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the AWS::ACMPCA::Permission resource handler records a CloudFormation resource row but never creates the permission in ACM Private CA, and its Ref value is a generated physical name rather than the identifier AWS returns.

AWS: recorded behavior only — once a stack declaring AWS::ACMPCA::Permission reaches CREATE_COMPLETE, ListPermissions for the owning certificate authority returns that permission (so a positive control can be established before the negative leg), and Ref resolves to the identifier ACM PCA uses for the permission, not to a `<stack>-<logical-id>-<suffix>` string. Deleting the resource revokes the permission, and ListPermissions goes empty.
Emulator: ListPermissions for the CA returns `[]` for the permission the stack just created — the check's control reads `control CA permissions []` — and Ref resolves to `<stack-name>-CaPermission-<suffix>`, a name ACM PCA does not know.
Why: a client cannot read, audit or revoke a CA permission that a template created; nothing downstream can resolve the Ref; and a negative leg (an invalid principal) cannot be distinguished from the resource simply never existing, because the positive control is empty too.
Fix: the AWS::ACMPCA::Permission resource handler's create/delete path (call the acm-pca crate's CreatePermission/DeletePermission so the permission exists in ACM PCA state) and its Ref return-value table.
Traces: the 2 checks in trace_ids.

## Evidence
acm-pca__cloudformation.deploy-cfn-acmpca-permission-invalid-principal-rejected — AssertionError: control CA permissions []  (expected the control stack's permission to be listed by ListPermissions)

acm-pca__cloudformation.deploy-cfn-acmpca-permission-provisions-and-revokes — AssertionError: Permission Ref '<stack-name>-CaPermission-<suffix>'  (expected the ACM PCA permission identifier, and the permission to be listed)

## Steps to Reproduce
1. CreateStack with a template declaring AWS::ACMPCA::CertificateAuthority and AWS::ACMPCA::Permission; wait for CREATE_COMPLETE.
2. Call ListPermissions for the CA — observe an empty list.
3. Export Ref of the Permission as a stack Output — observe the generated `<stack>-<logical-id>-<suffix>` name.
4. Re-replay the 2 checks in trace_ids.
