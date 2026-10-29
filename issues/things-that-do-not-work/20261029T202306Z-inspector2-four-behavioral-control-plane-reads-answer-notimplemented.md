Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: inspector2.associate-member-non-org-account-rejected inspector2.code-security-integration-create-get-delete-lifecycle inspector2.get-encryption-key-without-custom-key-returns-not-found inspector2.get-findings-report-status-no-report-returns-not-found
Issue-Shape-Id: com.amazonaws.inspector2#Inspector2
Issue-Key: inspector2.associate-member-non-org-account-rejected|run#3|AssertionError|assert admin_code == "ResourceNotFoundException", (
Issue-Key: inspector2.code-security-integration-create-get-delete-lifecycle|run#4|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"
Issue-Key: inspector2.get-encryption-key-without-custom-key-returns-not-found|run#10|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: inspector2.get-findings-report-status-no-report-returns-not-found|run#3|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# inspector2: four behavioral control-plane reads answer NotImplemented instead of their modeled response/refusal (4 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEncryptionKey

## Description
FOUR causes, one shared shape of defect: four inspector2 operations that the behavioral suite reaches are still scaffold stubs. Each answers an error whose code is `NotImplemented` and whose message is "operation <OperationName> is scaffolded but not implemented in this emulator" — a code and message that are in no inspector2 Smithy model and that no AWS SDK error-shape factory can map, so botocore surfaces a generic ClientError instead of the modeled exception. Because the calling checks catch the ClientError and inspect its code, these did NOT surface as blocked-stub failures in the replay report and are not covered by the auto-filed stub issues; they are listed here as separate work items. Note on overlap: cause 3 below is the SAME handler (src/ops/get_encryption_key.rs) that the auto-filed `GetEncryptionKey` stub issue names (it was filed from check inspector2.update-encryption-key-invalid-arn-pattern-rejected); a fixer should close both together, since one implementation satisfies both. The requirement in every cause is the modeled status, error code and message, with the variable parts taken from the request and the emulator's own account state — never from one recording's ids.

Cause 1 of 4: GetDelegatedAdminAccount on an account with no delegated administrator on file.
AWS: Smithy: GetDelegatedAdminAccount, POST /delegatedadminaccounts/get, output GetDelegatedAdminAccountResponse{delegatedAdmin: DelegatedAdmin}; modeled errors include ResourceNotFoundException (httpError 404, required member `message`). Recorded behavior: when the caller's organization has no Inspector2 delegated administrator, AWS answers 404 ResourceNotFoundException with message "No delegated admin found for caller account." — that whole sentence is fixed text, with no request- or account-derived variable part. (The check uses this read as the precondition for asserting AssociateMember's AccessDeniedException "Invoking account does not have access to associate member account", so the stub also blocks that assertion.)
Emulator: answers error code 'NotImplemented' with message "operation GetDelegatedAdminAccount is scaffolded but not implemented in this emulator".
Why: a client cannot determine whether its account is the delegated administrator, so every delegated-admin-gated flow (AssociateMember, DisassociateMember, organization-wide enablement) is undiscoverable; code that branches on ResourceNotFoundException to mean "not delegated admin" takes the unexpected-error path instead.
Fix: implement the GetDelegatedAdminAccount handler and its not-found refusal in the inspector2 crate's delegated-admin state (services/service-com-amazonaws-inspector2-inspector2/src/ops/get_delegated_admin_account.rs).
Traces: inspector2.associate-member-non-org-account-rejected.

Cause 2 of 4: CreateCodeSecurityIntegration for an account that has never activated Code Security.
AWS: Smithy: CreateCodeSecurityIntegration, POST /codesecurity/integration/create, input {name (required, IntegrationName @length 1..60, @pattern ^[a-zA-Z0-9-_$:.]*$), type (required, IntegrationType enum), details, tags}; modeled errors include AccessDeniedException (httpError 403). Recorded behavior: an account that has not completed the one-time, console/OAuth-driven Code Security activation is refused 403 AccessDeniedException with message "Invoking account is not Code Security enabled." — fixed text, no variable part; and no integration is created (the name never appears in ListCodeSecurityIntegrations).
Emulator: answers error code 'NotImplemented' with message "operation CreateCodeSecurityIntegration is scaffolded but not implemented in this emulator".
Why: a client cannot distinguish "your account has not activated Code Security" from an emulator fault, so the activation-gated onboarding path cannot be exercised at all; retry logic keyed on AccessDeniedException never engages.
Fix: implement the CreateCodeSecurityIntegration handler, including the account-level Code Security activation gate that precedes any integration write, in services/service-com-amazonaws-inspector2-inspector2/src/ops/create_code_security_integration.rs.
Traces: inspector2.code-security-integration-create-get-delete-lifecycle.

Cause 3 of 4: GetEncryptionKey when no customer managed key is configured.
AWS: Smithy: GetEncryptionKey, GET /encryptionkey/get, input {scanType (required, httpQuery), resourceType (required, httpQuery)}, output {kmsKeyId (required, KmsKeyArn)}; modeled errors include ResourceNotFoundException (httpError 404) and AccessDeniedException (403). Recorded behavior: with LAMBDA_CODE scanning enabled and no customer key set for scanType=CODE / resourceType=AWS_LAMBDA_FUNCTION, AWS answers 404 ResourceNotFoundException with message "Customer managed KMS key not found" — fixed text, no variable part. Once a key is configured via UpdateEncryptionKey, the identical call must answer 200 with kmsKeyId equal to the configured key ARN (that ARN is a value from state, not a literal).
Emulator: answers error code 'NotImplemented' with message "operation GetEncryptionKey is scaffolded but not implemented in this emulator".
Why: the encryption-key surface is unreadable, so a client can never tell "using the AWS-owned default key" (404) from "using my own key" (200 + kmsKeyId), and the Update/Reset round trip cannot be confirmed.
Fix: implement the GetEncryptionKey handler against the account/scan-type-keyed encryption-key state, including the 404 fallback when no customer key is set (services/service-com-amazonaws-inspector2-inspector2/src/ops/get_encryption_key.rs) — same handler as the auto-filed GetEncryptionKey stub issue.
Traces: inspector2.get-encryption-key-without-custom-key-returns-not-found.

Cause 4 of 4: GetFindingsReportStatus with no reportId and no report ever created.
AWS: Smithy: GetFindingsReportStatus, POST /reporting/status/get, input {reportId (optional, ReportId UUID pattern)}, output {reportId, status, errorCode, errorMessage, destination, filterCriteria}; modeled errors include ResourceNotFoundException (httpError 404). Recorded behavior: with reportId omitted and no findings report ever created for the account, AWS answers 404 ResourceNotFoundException with message "No report was found for this account" — fixed text, no variable part.
Emulator: answers error code 'NotImplemented' with message "operation GetFindingsReportStatus is scaffolded but not implemented in this emulator".
Why: a client polling report progress cannot distinguish "no report exists yet" from an emulator fault, so the CreateFindingsReport -> poll-status loop is unusable.
Fix: implement the GetFindingsReportStatus handler and its no-report-on-file refusal over the findings-report state (services/service-com-amazonaws-inspector2-inspector2/src/ops/get_findings_report_status.rs).
Traces: inspector2.get-findings-report-status-no-report-returns-not-found.

## Evidence
Cause 1 — check inspector2.associate-member-non-org-account-rejected: `assert admin_code == "ResourceNotFoundException"` failed with "expected no delegated admin on file, got 'NotImplemented'". Expected: 404 ResourceNotFoundException, message "No delegated admin found for caller account."; actual: error code NotImplemented, message "operation GetDelegatedAdminAccount is scaffolded but not implemented in this emulator".

Cause 2 — check inspector2.code-security-integration-create-get-delete-lifecycle: `assert code == "AccessDeniedException"` failed with "expected AccessDeniedException, got 'NotImplemented'". Expected: 403 AccessDeniedException, message "Invoking account is not Code Security enabled."; actual: NotImplemented / "operation CreateCodeSecurityIntegration is scaffolded but not implemented in this emulator".

Cause 3 — check inspector2.get-encryption-key-without-custom-key-returns-not-found: `assert code == "ResourceNotFoundException"` failed with "expected ResourceNotFoundException, got 'NotImplemented'". Expected: 404 ResourceNotFoundException, message "Customer managed KMS key not found"; actual: NotImplemented / "operation GetEncryptionKey is scaffolded but not implemented in this emulator".

Cause 4 — check inspector2.get-findings-report-status-no-report-returns-not-found: `assert code == "ResourceNotFoundException"` failed with "expected ResourceNotFoundException, got 'NotImplemented'". Expected: 404 ResourceNotFoundException, message "No report was found for this account"; actual: NotImplemented / "operation GetFindingsReportStatus is scaffolded but not implemented in this emulator".

In all four the modeled error shape for the condition exists in the inspector2 model (ResourceNotFoundException httpError 404, AccessDeniedException httpError 403); only the handler is absent, so the generic scaffold refusal is served in its place.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (simparity-behavioral-aws-v1), service inspector2.
2. Cause 1: call GetDelegatedAdminAccount (POST /delegatedadminaccounts/get, empty body) with an account that has no Inspector2 delegated administrator. Expect 404 ResourceNotFoundException "No delegated admin found for caller account."; observe NotImplemented. Re-replay check inspector2.associate-member-non-org-account-rejected.
3. Cause 2: call CreateCodeSecurityIntegration (POST /codesecurity/integration/create) with name=<any valid name>, type="GITHUB" from an account that has not activated Code Security. Expect 403 AccessDeniedException "Invoking account is not Code Security enabled."; observe NotImplemented. Re-replay inspector2.code-security-integration-create-get-delete-lifecycle.
4. Cause 3: enable LAMBDA_CODE scanning, then call GetEncryptionKey (GET /encryptionkey/get?scanType=CODE&resourceType=AWS_LAMBDA_FUNCTION) with no customer key configured. Expect 404 ResourceNotFoundException "Customer managed KMS key not found"; observe NotImplemented. Re-replay inspector2.get-encryption-key-without-custom-key-returns-not-found.
5. Cause 4: call GetFindingsReportStatus (POST /reporting/status/get, no reportId) on an account with no findings report. Expect 404 ResourceNotFoundException "No report was found for this account"; observe NotImplemented. Re-replay inspector2.get-findings-report-status-no-report-returns-not-found.
