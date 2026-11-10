Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.integration-error-semantics
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.integration-error-semantics|run#15|AssertionError|assert code_a == "InvalidParameterValue", (
Issue-Commit: 250d476ecb357920d67b44d09bfeff057fcec9e1

# CreateIntegration refuses a nonexistent DynamoDB SourceArn with IntegrationConflictOperationFault instead of InvalidParameterValue and AWS's source-table access message

> _Note: this issue was identified against the code base at commit `250d476ecb357920d67b44d09bfeff057fcec9e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
One net-new root cause on the Redshift zero-ETL integration surface: the SOURCE-side existence/permission check of CreateIntegration.

Two open issues touch this surface but neither carries this failing check: 20261104T045922Z-redshift-zero-etl-integration-refusals-and-state-machine.md (its causes cover the nonexistent TARGET namespace, the target resource-policy authorization check, the IntegrationAlreadyExists/IntegrationNotFound message wording and the DeleteIntegration state machine — none of them the source ARN path) and 20261024T115115Z-redshift-zero-etl-integration-error-semantics-unreachable-dynamodb.md (its cause 1 claimed this check was blocked in setup by DynamoDB CreateTable answering NotImplemented for SSESpecification; the check now gets past setup and reaches CreateIntegration, so the divergence it hits is a different, deeper one). This issue is grounded in the surviving failing check.

Cause 1 of 1: CreateIntegration classifies a well-formed SourceArn naming a DynamoDB table that does not exist as IntegrationConflictOperationFault, instead of refusing it synchronously as InvalidParameterValue with AWS's zero-ETL source-table access message.

AWS: CreateIntegration validates the SourceArn's existence and the caller's access to it synchronously, up front, and refuses with HTTP 400, error code 'InvalidParameterValue', and this message — entirely fixed text, with NO variable part substituted from the request: "You don't have access to zero-ETL integrations because the source DynamoDB table doesn't exist or you don't have the required permissions. Check the resource and key policy on the source table and make sure you have the required permissions. Refer to the Amazon DynamoDB Developer Guide for instructions on how to update the policy if required and then continue creating the integration." (recorded behavior for the classification and the wording). Smithy: CreateIntegration.SourceArn is @required, @length 1..255, @pattern ^arn:aws[a-z\-]*:(s3|dynamodb):.*:.*:[a-zA-Z0-9._\-/]+$ — the bogus ARN the check sends satisfies the pattern, so this is an existence/authorization decision, not a shape decision. CreateIntegration models IntegrationSourceNotFoundFault (httpError 404, awsQueryError IntegrationSourceNotFoundFault) and IntegrationConflictOperationFault (httpError 400) among its errors; AWS does not use either for a nonexistent source table — it collapses the case into InvalidParameterValue with the message above, which is what the recording shows.

Emulator: the same call is refused with error code 'IntegrationConflictOperationFault'. No source-existence decision is made at all: the refusal is a conflict class, reported even though the target namespace has been authorized for the inbound integration (the check calls the target-authorization step first precisely so an authorization refusal cannot mask the source check) and nothing else is in flight against that target.

Why: the two refusals mean opposite things to a client. 'IntegrationConflictOperationFault' is a transient, retry-after-the-other-operation-settles class — a provisioning flow backs off and re-issues CreateIntegration indefinitely for a source table that will never appear. 'InvalidParameterValue' with the source-table text is terminal and tells the operator exactly what to fix (the table, its resource policy, its KMS key policy). The emulator also makes the source-ARN existence check unobservable: a caller cannot distinguish an emulator that validates SourceArn from one that accepts any pattern-conforming ARN. The positive control in the same check (two real ARNs must be accepted) shows this is not an unmet zero-ETL prerequisite.

Fix: the CreateIntegration handler's source-resolution branch (services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/create_integration.rs) must resolve SourceArn against the DynamoDB/S3 source before taking any conflict decision, and refuse an unresolvable or inaccessible source with InvalidParameterValue carrying the fixed message above. The conflict check must only fire when another operation is genuinely in flight against the target.

Traces: redshift.integration-error-semantics.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator.

[mismatch] redshift.integration-error-semantics — check_redshift_integration_error_semantics.py line 56:
  AssertionError: expected InvalidParameterValue for nonexistent source, got IntegrationConflictOperationFault

Expected (AWS): CreateIntegration(IntegrationName=<name>, SourceArn=<well-formed arn:aws:dynamodb:<region>:<account>:table/<never-created-name>>, TargetArn=<real, authorized redshift-serverless namespace>) is refused synchronously with HTTP 400, Error.Code 'InvalidParameterValue' and Error.Message exactly "You don't have access to zero-ETL integrations because the source DynamoDB table doesn't exist or you don't have the required permissions. Check the resource and key policy on the source table and make sure you have the required permissions. Refer to the Amazon DynamoDB Developer Guide for instructions on how to update the policy if required and then continue creating the integration." (fixed text; no request- or state-derived values).

Actual (emulator): Error.Code 'IntegrationConflictOperationFault'.

The check isolates the source: the TargetArn is a real namespace it created and authorized for the inbound integration, and it retries the transient authorization/readiness codes (AccessDenied*, InvalidPolicyFault, IntegrationTargetNotFoundFault, KMSKeyNotAccessibleFault, ...) before asserting, so the conflict class is the terminal answer rather than a propagation artefact. The same check's positive control asserts that two real ARNs are accepted, so the expected InvalidParameterValue is an ARN-existence verdict and not a setup defect.

Region, account id, table name, namespace uuid and integration names are this run's environment; the requirements are the HTTP status, the error code and the fixed message text.

## Steps to Reproduce
1. Create a KMS key and a DynamoDB table to serve as a real zero-ETL source; create a Redshift Serverless namespace to serve as the target.
2. Authorize the target namespace for inbound integration from both the real source ARN and a bogus source ARN.
3. Call redshift CreateIntegration with SourceArn = arn:aws:dynamodb:<region>:<account>:table/<a table name that was never created> (pattern-valid) and TargetArn = the real, authorized namespace ARN.
4. Observe the emulator answers error code 'IntegrationConflictOperationFault' instead of HTTP 400 'InvalidParameterValue' with the fixed source-table access message quoted above.
5. Control: repeat the call with the real source ARN and the real target ARN and confirm it is accepted (Status 'creating' or 'active'), so step 4's refusal is attributable to the source-existence check.
6. Re-replay the behavioral check redshift.integration-error-semantics to confirm the fix.
