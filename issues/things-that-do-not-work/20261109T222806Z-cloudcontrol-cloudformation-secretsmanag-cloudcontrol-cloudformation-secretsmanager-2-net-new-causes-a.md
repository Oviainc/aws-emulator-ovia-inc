Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__cloudformation__secretsmanager.cfn-secretsmanager-secret-unknown-property-rejected-with-positive-control cloudcontrol__cloudformation__secretsmanager.secretsmanager-secretstring-writeonly-update-hidden-from-cloudcontrol
Issue-Key: cloudcontrol__cloudformation__secretsmanager.cfn-secretsmanager-secret-unknown-property-rejected-with-positive-control|_failure_reason#10|AssertionError|raise AssertionError(f"no CREATE_FAILED event for {logical_id!r} carrying a reason in {stack_id!r}")
Issue-Key: cloudcontrol__cloudformation__secretsmanager|GetResource|ResourceNotFoundException
Issue-Commit: 785433b7ada0bbe790b60a1839b6b0233a99f365

# cloudcontrol__cloudformation__secretsmanager: 2 net-new causes — a rolled-back AWS::SecretsManager::Secret stack carries no stack-level CREATE_FAILED reason, and Cloud Control GetResource reports a CFN-provisioned secret NotFound

> _Note: this issue was identified against the code base at commit `785433b7ada0bbe790b60a1839b6b0233a99f365`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two independent root causes, both grounded in failing checks of this service group.

Cause 1 of 2: a stack whose AWS::SecretsManager::Secret declares an unsupported property rolls back without a stack-level CREATE_FAILED reason.
AWS:      early resource-schema validation of the template fails the create and CloudFormation records a CREATE_FAILED event whose LogicalResourceId is the STACK's own name, carrying ResourceStatusReason — fixed text, no variable part except the error count: `Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.` DescribeEvents then lists one VALIDATION_ERROR OperationEvent: ValidationName=PROPERTY_VALIDATION, ValidationStatus=FAILED, ValidationFailureMode=FAIL, ValidationStatusReason=`Unsupported property [<property>]`, ValidationPath=`/Resources/<logical id>/Properties/<property>` (the property name and the path are taken from the submitted template). Recorded behavior only for the event text; the stack ends ROLLBACK_COMPLETE and no secret is created (DescribeSecret then answers ResourceNotFoundException 'Secrets Manager can\'t find the specified secret.').
Emulator: the stack does reach ROLLBACK_COMPLETE, but DescribeStackEvents (paginated over every page) contains NO CREATE_FAILED event whose LogicalResourceId is the stack name and that carries a ResourceStatusReason at all, so the validation reason and the VALIDATION_ERROR rows a caller is told to fetch are unobtainable.
Why:      a client cannot learn WHY its deployment rolled back — the documented recovery path (read the stack-level reason, then DescribeEvents for the offending property and its template path) yields nothing, and tooling that surfaces the failure reason shows an empty cause for a deterministic schema error.
Fix:      the CloudFormation create/rollback path's event recorder (template resource-schema validation) must write the stack-level CREATE_FAILED row with that reason and the matching VALIDATION_ERROR operation events.
Failing check: cloudcontrol__cloudformation__secretsmanager.cfn-secretsmanager-secret-unknown-property-rejected-with-positive-control.

Cause 2 of 2: Cloud Control GetResource refuses an AWS::SecretsManager::Secret that CloudFormation provisioned.
AWS:      a secret that exists (CFN CREATE_COMPLETE, Secrets Manager GetSecretValue/DescribeSecret return it) resolves through cloudcontrol GetResource(TypeName=AWS::SecretsManager::Secret, Identifier=<secret ARN>): the ResourceDescription's Properties carry the readable model (Id = the secret ARN, Name = the secret name) and never the write-only SecretString. Smithy: GetResource takes TypeName+Identifier and answers ResourceNotFoundException (errorKind client, httpError 404) only when the resource does not exist.
Emulator: GetResource is refused 404 ResourceNotFoundException with message `AWS::SecretsManager::Secret Handler returned status FAILED: Resource of type 'AWS::SecretsManager::Secret' with identifier '<secret ARN>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)` for a secret the same emulator serves through Secrets Manager, so neither the write-only-hiding assertion nor the change-set in-place-update assertions downstream of it are ever reached.
Why:      a client treating Cloud Control as the uniform resource API cannot read any CFN-provisioned secret, and the guarantee that the write-only SecretString is absent from the resource model is unverifiable.
Fix:      the AWS::SecretsManager::Secret provider behind cloudcontrol GetResource must resolve a secret ARN against Secrets Manager state (as the already-open sibling issues do for SSM/Logs/SQS/DynamoDB/Lambda: 20261109T142303Z-cloudcontrol-ssm-..., 20261109T143515Z-cloudcontrol-...-aws-logs..., 20261109T185037Z-cloudcontrol-sqs-..., 20261109T172142Z-cloudcontrol-dynamodb-..., 20261109T210043Z-cloudcontrol-cloudformation-lambda-...; none of them covers Secrets Manager).
Failing check: cloudcontrol__cloudformation__secretsmanager.secretsmanager-secretstring-writeonly-update-hidden-from-cloudcontrol.

## Evidence
Cause 1 — expected: a CREATE_FAILED StackEvent with LogicalResourceId = <stack name> and ResourceStatusReason = 'Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.', plus one DescribeEvents VALIDATION_ERROR row ('Secret', 'PROPERTY_VALIDATION', 'FAILED', 'FAIL', 'Unsupported property [ThisPropertyDoesNotExist]', '/Resources/Secret/Properties/ThisPropertyDoesNotExist').
Cause 1 — actual: AssertionError: no CREATE_FAILED event for '<stack name>' carrying a reason in '<stack arn>' (the stack status itself was ROLLBACK_COMPLETE, as expected).

Cause 2 — expected: GetResource returns a ResourceDescription whose Properties have Id = the secret ARN and Name = the declared secret name, with no SecretString member.
Cause 2 — actual: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetResource operation: AWS::SecretsManager::Secret Handler returned status FAILED: Resource of type 'AWS::SecretsManager::Secret' with identifier '<secret arn>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>) — raised for a secret whose GetSecretValue on the same emulator had just returned the stored value. Names/ARNs/tokens above are per-run values, not requirements.

## Steps to Reproduce
Cause 1:
1. cloudformation CreateStack with Resources.Secret = {Type: AWS::SecretsManager::Secret, Properties: {Name: <n>, SecretString: <v>, ThisPropertyDoesNotExist: 1}}.
2. Poll DescribeStacks to a terminal status (ROLLBACK_COMPLETE).
3. Page DescribeStackEvents and look for a CREATE_FAILED row whose LogicalResourceId is the stack name and that carries ResourceStatusReason — none exists; DescribeEvents likewise lists no VALIDATION_ERROR row.

Cause 2:
1. cloudformation CreateStack with Resources.Secret = {Type: AWS::SecretsManager::Secret, Properties: {Name: <n>, SecretString: 'v1-secret-value'}}; wait for CREATE_COMPLETE.
2. Take the secret ARN from ListStackResources[0].PhysicalResourceId; confirm secretsmanager GetSecretValue returns 'v1-secret-value'.
3. cloudcontrol GetResource TypeName=AWS::SecretsManager::Secret Identifier=<that ARN> — refused 404 ResourceNotFoundException instead of returning the resource model.
