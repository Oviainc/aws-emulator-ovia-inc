Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.denied-states-action-names-resource-by-api-category stepfunctions.explicit-deny-blocks-every-states-api stepfunctions.iam-resource-prefix-on-state-machine-scopes-sm-ops-not-executions stepfunctions.create-and-update-alias-qualifier-condition-needs-name-and-every-version stepfunctions.iam-resource-prefix-on-activity-grants-activity-ops-and-matching-token stepfunctions.statemachinequalifier-condition-restricts-qualifier-value-not-its-absence stepfunctions.create-state-machine-request-tags-evaluated-as-resource-tag stepfunctions.request-tag-and-tag-keys-conditions-deny-forbidden-tag-mutations stepfunctions.resource-tag-condition-denies-tagged-activity-state-machine-and-its-executions stepfunctions.sfn-service-level-only-actions-succeed-resource-scoped-actions-denied stepfunctions.create-with-tags-implicitly-requires-tagresource-permission stepfunctions.publish-state-machine-version-permission-denied-on-create-and-update stepfunctions.create-state-machine-express-logging-role-source-arn-mismatch-denied stepfunctions.execution-role-trust-source-arn-naming-another-account-blocks-task stepfunctions.cross-account-alias-version-and-list-ops-access-denied stepfunctions.sfn-startexecution-integration-child-started-by-alias-or-version-arn stepfunctions.sqs-send-message-task-without-permission-is-denied stepfunctions.send-task-failure-kms-access-denied stepfunctions.sqs-send-message-optimized-integration-output-carries-the-sdk-envelope
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.denied-states-action-names-resource-by-api-category|expect#16|AssertionError|assert message == expected, f"{op}: message was {message!r}, expected {expected!r}"
Issue-Key: stepfunctions.explicit-deny-blocks-every-states-api|run#20|AssertionError|assert message == expected, f"{op}: message was {message!r}, expected {expected!r}"
Issue-Key: stepfunctions.iam-resource-prefix-on-state-machine-scopes-sm-ops-not-executions|_access_denied#45|AssertionError|assert actual_message == expected, (
Issue-Key: stepfunctions.create-and-update-alias-qualifier-condition-needs-name-and-every-version|run#16|AssertionError|assert message == (
Issue-Key: sfn|SendTaskSuccess|AccessDeniedException
Issue-Key: stepfunctions.statemachinequalifier-condition-restricts-qualifier-value-not-its-absence|_retry_iam_propagation#15|AssertionError|raise AssertionError(f"the caller role's policy never took effect: {last!r}")
Issue-Key: stepfunctions.create-state-machine-request-tags-evaluated-as-resource-tag|_poll_create#11|Timeout|time.sleep(interval_s)
Issue-Key: stepfunctions.request-tag-and-tag-keys-conditions-deny-forbidden-tag-mutations|_poll_create#11|Timeout|time.sleep(interval_s)
Issue-Key: stepfunctions.resource-tag-condition-denies-tagged-activity-state-machine-and-its-executions|_poll_until_ok#16|Timeout|time.sleep(interval_s)
Issue-Key: stepfunctions.sfn-service-level-only-actions-succeed-resource-scoped-actions-denied|_retry_while_denied#12|Timeout|time.sleep(2)
Issue-Key: stepfunctions.create-with-tags-implicitly-requires-tagresource-permission|_refusal#16|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: stepfunctions.publish-state-machine-version-permission-denied-on-create-and-update|_error#8|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Key: stepfunctions.create-state-machine-express-logging-role-source-arn-mismatch-denied|_error#19|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Key: stepfunctions.execution-role-trust-source-arn-naming-another-account-blocks-task|run#10|AssertionError|assert std_final["status"] == "FAILED", (
Issue-Key: stepfunctions.cross-account-alias-version-and-list-ops-access-denied|run#15|AssertionError|assert err == expected, f"DescribeStateMachineAlias on a foreign-account alias ARN: expected {expected!r}, got {err!r}"
Issue-Key: stepfunctions.sfn-startexecution-integration-child-started-by-alias-or-version-arn|run#16|AssertionError|assert done["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.sqs-send-message-task-without-permission-is-denied|run#11|AssertionError|assert masked == (
Issue-Key: stepfunctions.send-task-failure-kms-access-denied|run#18|AssertionError|assert stable == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: stepfunctions.sqs-send-message-optimized-integration-output-carries-the-sdk-envelope|_assert_envelope#9|AssertionError|assert isinstance(req_id, str) and req_id, (
Issue-Commit: 4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666

# stepfunctions: 10 net-new causes — IAM authorization for states APIs: wrong resource in denial messages, unenforced permissions/conditions, and hangs on tag conditions

> _Note: this issue was identified against the code base at commit `4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStateMachines

## Description
Ten independent root causes in how the emulator authorizes and reports authorization for Step Functions APIs, and in how it relays a downstream service's denial into a task. Several are security-relevant: calls AWS denies succeed here.

Cause 1 of 10: AccessDeniedException names `resource: *` instead of the ARN pattern for the API's category.
AWS: the denial message is `User: <caller identity ARN> is not authorized to perform: <action> on resource: <the resource ARN pattern for that API category> because no identity-based policy allows the <action> action` (and `... with an explicit deny in an identity-based policy` for an explicit Deny). For a state-machine-category action such as states:ListStateMachines the resource is `arn:aws:states:<region>:<account>:stateMachine:*`, not `*`. Fixed text around the identity ARN, the action and the resource pattern; the identity and account come from the request.
Emulator: always renders `on resource: *`, for both the no-policy and the explicit-deny wordings, and for alias-category actions such as states:CreateStateMachineAlias.
Why: a client that reads the denial to learn which resource scope it needs is told `*`, and policy-troubleshooting output is wrong for every states action.
Fix: the resource-pattern derivation in the states authorization denial-message builder.
Traces: stepfunctions.denied-states-action-names-resource-by-api-category, stepfunctions.explicit-deny-blocks-every-states-api, stepfunctions.iam-resource-prefix-on-state-machine-scopes-sm-ops-not-executions, stepfunctions.create-and-update-alias-qualifier-condition-needs-name-and-every-version

Cause 2 of 10: an activity-ARN resource prefix does not authorize SendTaskSuccess for a matching task token.
AWS: a policy granting activity actions on an activity ARN prefix authorizes states:SendTaskSuccess for a token issued by a matching activity.
Emulator: SendTaskSuccess is refused AccessDeniedException `... is not authorized to perform: states:SendTaskSuccess on resource: * because no identity-based policy allows the states:SendTaskSuccess action`.
Why: a correctly scoped worker policy cannot complete its task.
Fix: resource matching for token-based activity actions in the authorizer.
Traces: stepfunctions.iam-resource-prefix-on-activity-grants-activity-ops-and-matching-token

Cause 3 of 10: the states:StateMachineQualifier condition key is not evaluated, so allowed calls are denied.
AWS: a policy allowing states:StartExecution with a condition on states:StateMachineQualifier permits a start on a matching alias/version qualifier (and restricts only the qualifier's VALUE, not its absence).
Emulator: the caller's policy never takes effect — StartExecution on the alias ARN is refused AccessDeniedException.
Why: qualifier-scoped policies are unusable; every start under them is denied.
Fix: support for the states:StateMachineQualifier condition key in the condition evaluator.
Traces: stepfunctions.statemachinequalifier-condition-restricts-qualifier-value-not-its-absence

Cause 4 of 10: authorization involving tag conditions and service-level-only actions never returns — the check hangs past 180 s.
AWS: policies using aws:RequestTag/aws:TagKeys/aws:ResourceTag conditions and service-level-only actions are evaluated and the calls answer promptly (allowed or AccessDeniedException).
Emulator: recorded behavior only — all four checks end `Check timed out after 180.0s`, so no response was produced at all.
Why: a client using tag-based policies gets no answer; requests hang rather than being allowed or denied.
Fix: the tag-condition / service-level-action path of the states authorizer (the non-terminating evaluation).
Traces: stepfunctions.create-state-machine-request-tags-evaluated-as-resource-tag, stepfunctions.request-tag-and-tag-keys-conditions-deny-forbidden-tag-mutations, stepfunctions.resource-tag-condition-denies-tagged-activity-state-machine-and-its-executions, stepfunctions.sfn-service-level-only-actions-succeed-resource-scoped-actions-denied

Cause 5 of 10: CreateStateMachine with tags does not require states:TagResource.
AWS: passing `tags` on CreateStateMachine implicitly requires states:TagResource; without it the call is refused AccessDeniedException and no state machine is created. Smithy: CreateStateMachine.tags is a TagList member of the input.
Emulator: returns 200 with a stateMachineArn.
Why: a caller escapes a permission boundary AWS enforces.
Fix: the implicit TagResource authorization in the CreateStateMachine handler.
Traces: stepfunctions.create-with-tags-implicitly-requires-tagresource-permission

Cause 6 of 10: publish=true does not require the version-publish permission.
AWS: CreateStateMachine/UpdateStateMachine with publish=true require the publish permission; denied, the call is refused and no version exists.
Emulator: returns 200 including `stateMachineVersionArn` for a caller denied that permission.
Fix: the publish-permission check in the create/update handlers.
Traces: stepfunctions.publish-state-machine-version-permission-denied-on-create-and-update

Cause 7 of 10: role trust aws:SourceArn is not evaluated (logging role and execution role).
AWS: CreateStateMachine with a logging role whose trust policy's aws:SourceArn does not match the state machine being created is refused (ClientError); and an execution whose role's trust names another account's SourceArn FAILS when the task runs.
Emulator: the CreateStateMachine succeeds, and the execution SUCCEEDS.
Why: confused-deputy protection a client relies on is not applied.
Fix: aws:SourceArn/aws:SourceAccount evaluation in the AssumeRole trust-policy check used for logging and execution roles.
Traces: stepfunctions.create-state-machine-express-logging-role-source-arn-mismatch-denied, stepfunctions.execution-role-trust-source-arn-naming-another-account-blocks-task

Cause 8 of 10: a foreign-account ARN answers ResourceNotFound instead of AccessDeniedException.
AWS: DescribeStateMachineAlias (and the version/list operations) on an ARN in another account is refused AccessDeniedException with message `User: <caller ARN> is not authorized to access this resource`.
Emulator: `{'Code': 'ResourceNotFound', 'Message': "Resource not found: '<the foreign ARN>'"}`.
Why: a cross-account misconfiguration is reported as a missing resource, and the emulator leaks that it simply has no such record.
Fix: the cross-account ownership check ahead of lookup in the alias/version read handlers.
Traces: stepfunctions.cross-account-alias-version-and-list-ops-access-denied

Cause 9 of 10: a states:startExecution task naming a version- or alias-qualified child ARN is denied though the policy allows it.
AWS: a policy granting states:StartExecution on the child state machine authorizes starting it by its version ARN (`...:<name>:1`) and by its alias ARN; the parent succeeds.
Emulator: the parent ends FAILED `StepFunctions.AWSStepFunctionsException` carrying AccessDeniedException `... not authorized to perform: states:StartExecution on resource: arn:...:stateMachine:<child>:1 because no identity-based policy allows the states:StartExecution action`.
Why: qualified ARNs are not matched against the granted unqualified resource, so versioned/aliased nesting is unusable.
Fix: qualifier-aware resource matching in the authorizer for StartExecution.
Traces: stepfunctions.sfn-startexecution-integration-child-started-by-alias-or-version-arn

Cause 10 of 10: a downstream service's denial/response envelope is rebuilt wrongly in the task's cause.
AWS: an optimized integration relays the downstream error verbatim. For sqs:sendMessage denied, the cause is `User: arn:aws:sts::<account>:assumed-role/<role-name>/<session> is not authorized to perform: sqs:sendmessage on resource: <queue ARN> because no identity-based policy allows the sqs:sendmessage action (Service: AmazonSQS; Status Code: 403; Error Code: AccessDenied; Request ID: <id>; Proxy: null)` — the identity is the assumed-role SESSION of the execution role, the action is lower-cased as SQS reports it, the status is 403 with code AccessDenied, and a `Request ID: <id>;` segment is present. A successful sqs:sendMessage task's TaskSucceeded output carries `SdkResponseMetadata.RequestId`. A KMS denial raised while writing a SendTaskFailure names the execution role's assumed-role session and the operation actually performed (kms:Decrypt).
Emulator: names the plain IAM role ARN (`arn:aws:iam::<acct>:role/<role>`) or even `arn:aws:iam::<acct>:user/emulator`, keeps the action's camel case (`sqs:SendMessage`), reports `Status Code: 400; Error Code: AccessDeniedException`, omits the `Request ID:` segment entirely, reports kms:GenerateDataKey where Decrypt was required, and returns `SdkResponseMetadata.RequestId = None` on success.
Why: a client parsing the relayed cause cannot recover the caller, the status, the error code or the request id — and the identity shown is not the one that made the call. (The `user/emulator` identity is also observed by the already-filed 20261106T214711Z-stepfunctions-4-net-new-causes-resultwriter-refused-as.md; the SQS/KMS envelope defects here are not covered by it.)
Fix: the downstream error/response envelope builder shared by the optimized task integrations (identity rendering, action casing, status/code mapping, request-id propagation).
Traces: stepfunctions.sqs-send-message-task-without-permission-is-denied, stepfunctions.send-task-failure-kms-access-denied, stepfunctions.sqs-send-message-optimized-integration-output-carries-the-sdk-envelope

## Evidence
1) got `... not authorized to perform: states:ListStateMachines on resource: * because no identity-based policy allows the states:ListStateMachines action`; expected `... on resource: arn:aws:states:us-west-2:<acct>:stateMachine:* ...`. Same with `with an explicit deny in an identity-based policy`.
2) `botocore.exceptions.ClientError: An error occurred (AccessDeniedException) when calling the SendTaskSuccess operation: User: ... is not authorized to perform: states:SendTaskSuccess on resource: *`.
3) `the caller role's policy never took effect: ClientError('An error occurred (AccessDeniedException) when calling the StartExecution operation: ... on resource: arn:aws:states:...:stateMachine:<sm>:<alias> ...')`.
4) `Error: Check timed out after 180.0s` (all four checks).
5) `expected the call to be refused, but it succeeded: {'stateMachineArn': '...createtag-sm-...', 'HTTPStatusCode': 200}`.
6) `expected a ClientError, but the call succeeded: {'stateMachineArn': '...', 'stateMachineVersionArn': '...:1'}`.
7) `expected a ClientError, but the call succeeded: 'arn:aws:states:...:stateMachine:sim2real-sfn-badsm-...'`; `an execution whose role trusts a foreign account's SourceArn must FAIL; got 'SUCCEEDED'`.
8) expected `{'Code': 'AccessDeniedException', 'Message': 'User: ... is not authorized to access this resource'}`; got `{'Code': 'ResourceNotFound', 'Message': "Resource not found: '...'"}`.
9) `the parent calling the child by version then alias ARN ended 'FAILED': 'StepFunctions.AWSStepFunctionsException' 'User: ... is not authorized to perform: states:StartExecution on resource: arn:...:<child>:1 ...'`.
10) got `User: arn:aws:iam::<acct>:role/<role> is not authorized to perform: sqs:SendMessage on resource: <queue arn> because no identity-based policy allows the sqs:SendMessage action (Service: AmazonSQS; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)`; expected `User: arn:aws:sts::<acct>:assumed-role/<role>/<session> ... sqs:sendmessage ... (Service: AmazonSQS; Status Code: 403; Error Code: AccessDenied; Request ID: <id>; Proxy: null)`. KMS: got `User: arn:aws:iam::<acct>:user/emulator ... kms:GenerateDataKey ...`; expected the assumed-role session and `kms:Decrypt`. Success path: `the plain sqs:sendMessage TaskSucceeded SdkResponseMetadata.RequestId was None`.

## Steps to Reproduce
1. Assume a role with no states permissions and call ListStateMachines; repeat with an explicit Deny policy and with CreateStateMachineAlias.
2. Grant activity actions on an activity ARN prefix, claim a task with GetActivityTask, then SendTaskSuccess.
3. Grant states:StartExecution with a condition on states:StateMachineQualifier and start via the alias ARN.
4. Create policies using aws:RequestTag / aws:TagKeys / aws:ResourceTag conditions and call the tag-mutating states APIs.
5. CreateStateMachine with tags under a role lacking states:TagResource.
6. CreateStateMachine/UpdateStateMachine with publish=true under a role denied the publish action.
7. CreateStateMachine (EXPRESS, logging) with a logging role whose trust aws:SourceArn names another state machine; and start an execution whose role trust names another account's SourceArn.
8. DescribeStateMachineAlias on an ARN in account 111122223333.
9. A parent state machine whose startExecution task names the child's version ARN and alias ARN, with StartExecution granted on the child.
10. An sqs:sendMessage task under a role without sqs:SendMessage (read the cause), the same task with the permission (read TaskSucceeded SdkResponseMetadata), and SendTaskFailure on a CMK-encrypted machine with kms:Decrypt denied.
