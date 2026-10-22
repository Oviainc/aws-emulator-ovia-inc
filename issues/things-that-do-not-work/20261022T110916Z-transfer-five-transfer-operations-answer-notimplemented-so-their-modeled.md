Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.test-identity-provider-invalid-request-error transfer.test-identity-provider-service-managed-rejected transfer.test-identity-provider-wrong-server-type-error transfer.delete-access-on-service-managed-server-invalid-request-rejected transfer.describe-webapp-access-denied-unauthorized-caller transfer.send-workflow-step-state-unauthorized-denied transfer.start-file-transfer-inactive-connector-rejected
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.test-identity-provider-invalid-request-error|run#11|AssertionError|assert invalid_code == "InvalidRequestException", (
Issue-Key: transfer.test-identity-provider-service-managed-rejected|run#5|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: transfer.test-identity-provider-wrong-server-type-error|run#7|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}"
Issue-Key: transfer.delete-access-on-service-managed-server-invalid-request-rejected|run#17|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}"
Issue-Key: transfer.describe-webapp-access-denied-unauthorized-caller|run#6|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException for an unauthorized caller, got {code}"
Issue-Key: transfer.send-workflow-step-state-unauthorized-denied|run#9|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: transfer.start-file-transfer-inactive-connector-rejected|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Commit: dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4

# Five Transfer operations answer NotImplemented, so their modeled rejections (InvalidRequestException / ResourceNotFoundException / AccessDeniedException) can never be observed (5 causes)

> _Note: this issue was identified against the code base at commit `dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestIdentityProvider

## Description
Five bound Transfer operations answer the emulator's non-AWS error code NotImplemented ("operation X is scaffolded but not implemented in this emulator") instead of their modeled behavior. None of these five is covered by the scaffold-stub issues the suite already filed for this service (those name CreateProfile, ListProfiles, CreateConnector, ListConnectors, CreateWebApp, ListWebApps, CreateWorkflow, ListWorkflows, ImportCertificate, ImportHostKey, ListHostKeys, ListSecurityPolicies, DescribeSecurityPolicy and CreateAccess). Each cause below is one unimplemented operation, with the modeled answer the caller never gets.

Cause 1 of 5: TestIdentityProvider.
AWS: TestIdentityProvider on a server whose IdentityProviderType is SERVICE_MANAGED is refused with InvalidRequestException (Smithy: TestIdentityProvider errors = InternalServiceError 503, InvalidRequestException 400, ResourceNotFoundException 404, ServiceUnavailableException 503; the success output is {Url (required), StatusCode (required), Response, Message}), because there is no customer identity provider to call.
Emulator: NotImplemented for every invocation.
Why: the only API a customer has for validating identity-provider wiring returns an uncatchable non-AWS code, so both its success envelope and its 400 family are unobservable.
Fix: implement the TestIdentityProvider handler in services/service-com-amazonaws-transfer-transferservice/src/ops/ (test_identity_provider), including the SERVICE_MANAGED / wrong-server-type refusal.
Traces: transfer.test-identity-provider-invalid-request-error, transfer.test-identity-provider-service-managed-rejected, transfer.test-identity-provider-wrong-server-type-error.

Cause 2 of 5: DeleteAccess.
AWS: DeleteAccess on a SERVICE_MANAGED server is refused with InvalidRequestException (Smithy: DeleteAccess errors = InternalServiceError, InvalidRequestException 400, ResourceNotFoundException 404, ServiceUnavailableException; output is empty) — the server-type check precedes any existence lookup.
Emulator: NotImplemented.
Why: the directory-vs-service-managed distinction is unobservable, and a caller cannot tell a wrong-server-type request from an absent grant.
Fix: implement the DeleteAccess handler (delete_access) with the IdP-type gate before the ExternalId lookup.
Traces: transfer.delete-access-on-service-managed-server-invalid-request-rejected.

Cause 3 of 5: DescribeWebApp.
AWS: DescribeWebApp models AccessDeniedException (403) alongside InvalidRequestException (400), ResourceNotFoundException (404) and ThrottlingException (429), and refuses an unauthorized caller with AccessDeniedException.
Emulator: NotImplemented.
Why: web-app reads cannot be exercised at all, and the authorization refusal the operation models is unreachable.
Fix: implement the DescribeWebApp handler (describe_web_app).
Traces: transfer.describe-webapp-access-denied-unauthorized-caller.

Cause 4 of 5: SendWorkflowStepState.
AWS: SendWorkflowStepState models AccessDeniedException (403) and refuses a principal lacking transfer:SendWorkflowStepState before validating WorkflowId/ExecutionId/Token; with permission but bogus ids it reaches domain validation and answers ResourceNotFoundException (404).
Emulator: NotImplemented.
Why: the custom-step callback is the whole data-plane contract of a Transfer workflow; a stub makes both the authorization and the sequencing behavior unobservable.
Fix: implement the SendWorkflowStepState handler (send_workflow_step_state).
Traces: transfer.send-workflow-step-state-unauthorized-denied.

Cause 5 of 5: StartFileTransfer.
AWS: StartFileTransfer against a connector that does not exist (or is not usable) answers ResourceNotFoundException (Smithy: StartFileTransfer errors = InternalServiceError, InvalidRequestException 400, ResourceNotFoundException 404, ServiceUnavailableException, ThrottlingException; success output {TransferId}).
Emulator: NotImplemented.
Why: callers cannot distinguish a missing connector from a service fault, and no transfer can be started.
Fix: implement the StartFileTransfer handler (start_file_transfer).
Traces: transfer.start-file-transfer-inactive-connector-rejected.

## Evidence
Cause 1: expected InvalidRequestException for a SERVICE_MANAGED server; got code NotImplemented ('operation TestIdentityProvider is scaffolded but not implemented in this emulator') — three checks.
Cause 2: expected InvalidRequestException; got 'NotImplemented'.
Cause 3: expected AccessDeniedException for an unauthorized caller; got NotImplemented.
Cause 4: expected AccessDeniedException; got NotImplemented.
Cause 5: expected ResourceNotFoundException; got 'NotImplemented'.
NotImplemented is not a code any Transfer operation models, so no SDK exception class maps to it.

## Steps to Reproduce
Replay the behavioral suite and run: transfer.test-identity-provider-invalid-request-error, transfer.test-identity-provider-service-managed-rejected, transfer.test-identity-provider-wrong-server-type-error (CreateServer with the default SERVICE_MANAGED IdP, then TestIdentityProvider on it); transfer.delete-access-on-service-managed-server-invalid-request-rejected (CreateServer default, then DeleteAccess(ExternalId='S-1-5-21-...')); transfer.describe-webapp-access-denied-unauthorized-caller (DescribeWebApp as an unauthorized principal); transfer.send-workflow-step-state-unauthorized-denied (SendWorkflowStepState with bogus ids from an unprivileged role session); transfer.start-file-transfer-inactive-connector-rejected (StartFileTransfer against an absent ConnectorId).
