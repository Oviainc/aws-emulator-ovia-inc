Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.deploy-cfn-missing-dynamodb-table-arn-env-negative
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-missing-dynamodb-table-arn-env-negative|run#10|AssertionError|assert payload["ok"] is True, f"positive control failed: {payload}"
Issue-Commit: ad889ea564c74ff66a8d5d7080e6fcd3e62d9684

# A CloudFormation-created DynamoDB table is not addressable by the name Ref publishes — the deployed function's PutItem is refused ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `ad889ea564c74ff66a8d5d7080e6fcd3e62d9684`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
This issue carries 1 net-new root cause. It must be closed before submitting.

Cause 1 of 1: a stack whose AWS::Lambda::Function receives `TABLE_NAME: !Ref DDBTable` cannot write to that table — every PutItem from the deployed handler is refused ResourceNotFoundException, even though the stack reached CREATE_COMPLETE and published the same `!Ref DDBTable` value as its `TableName` output.

AWS: Smithy (service-com-amazonaws-dynamodb-dynamodb-20120810) models PutItem.TableName as required, target TableArn, @length min 1 max 1024, and models `ResourceNotFoundException` (client, HTTP 400) for it — the operation's documented meaning is that the named table does not exist or is not in ACTIVE state. CloudFormation's `Ref` on an AWS::DynamoDB::Table returns that table's NAME, so a value taken from `Ref` and passed back as PutItem.TableName must resolve to the table the stack created, and PutItem must succeed (no modelled error) for a role granted dynamodb:PutItem on it. The check's negative arm — a name derived with `!Sub "${DDBTable}-typo-nonexistent"` — is the only arm for which ResourceNotFoundException is correct.

Emulator: the positive control is refused. The handler's `ddb.put_item(TableName=os.environ['TABLE_NAME'], ...)` returns the modelled error and the handler reports `{'ok': False, 'error': 'ResourceNotFoundException', 'id': '<generated-id>'}` where AWS returns `{'ok': True, 'id': '<generated-id>'}`. The id in that payload is a per-run generated value and is not part of the requirement; the diverging parts are `ok` (False instead of True) and the presence of the error code `ResourceNotFoundException` at all. So both arms of the check now answer identically, and the negative arm's assertion is no longer distinguishable from the positive one.

Hypothesis (neither the Smithy model nor the recording states this): the value CloudFormation resolves for `Ref` on AWS::DynamoDB::Table is not the physical table name under which the DynamoDB state store registered the table — e.g. the logical resource id, or a name minted in the CFN provider that the table was never created under. The same family of defect is already filed for another resource type in the open issue 20261107T075601Z-cloudformation-ec2-ssm-cloudformation-ssm-aws-ssm-maintenancewindowtarget-unrecognised-ec2-ref.md ("EC2 Ref yields a logical name"); that issue does not carry this replay key or this resource type. The nearby open issue 20261106T020636Z-apigateway-dynamodb-lambda-logs-a-cloudformation-create-of.md covers a stack that ends ROLLBACK_COMPLETE, which is a different failure: here the stack reaches CREATE_COMPLETE and publishes outputs.

Why: a client that wires a table into a function the documented way — `Environment.Variables.TABLE_NAME: !Ref Table` — gets a function that cannot reach its own table, and sees DynamoDB's "table does not exist" error for a table the same stack just created. Infrastructure-as-code for any DynamoDB-backed workload is unusable, and a negative test that asserts ResourceNotFoundException for a mistyped table name passes for the wrong reason, because the correct name fails the same way.

Fix: the CloudFormation provider for AWS::DynamoDB::Table — the physical id it registers and the value it resolves for `Ref`/`Fn::Sub` — and its write-through to the DynamoDB state store that PutItem resolves TableName against.

Traces: apigateway__dynamodb__lambda__logs.deploy-cfn-missing-dynamodb-table-arn-env-negative.

## Evidence
Check apigateway__dynamodb__lambda__logs.deploy-cfn-missing-dynamodb-table-arn-env-negative:

  File "check_apigateway__dynamodb__lambda__logs_deploy_cfn_missing_dynamodb_table_arn_env_negative.py", line 133, in run
    assert payload["ok"] is True, f"positive control failed: {payload}"
  AssertionError: positive control failed: {'ok': False, 'error': 'ResourceNotFoundException', 'id': 'good-b194455fec48'}

Expected (AWS): the positive-control invoke returns {'ok': True, 'id': <the id from the request payload>}, and a subsequent GetItem on the table named by the stack's TableName output returns that item. ResourceNotFoundException is expected ONLY for the second, mistargeted function whose TABLE_NAME is `!Sub "${DDBTable}-typo-nonexistent"`.
Actual (emulator): the positive-control function is refused with DynamoDB's modelled ResourceNotFoundException for the table name the stack itself published via `!Ref DDBTable`.

The stack itself succeeded: _deploy_cfn asserted CREATE_COMPLETE and read back the Outputs TableName / CorrectFunctionName / WrongFunctionName before the invoke, so the divergence is not a stack-create failure.

Template shape (from the check): AWS::DynamoDB::Table (BillingMode PAY_PER_REQUEST, key `id` S) + AWS::IAM::Role granting dynamodb:PutItem/GetItem and logs:* on '*' + two AWS::Lambda::Function resources (python3.12) whose Environment.Variables.TABLE_NAME are `!Ref DDBTable` and `!Sub "${DDBTable}-typo-nonexistent"`; Outputs TableName: !Ref DDBTable.

## Steps to Reproduce
Replay apigateway__dynamodb__lambda__logs.deploy-cfn-missing-dynamodb-table-arn-env-negative.

1. CreateStack with a template holding an AWS::DynamoDB::Table (no explicit TableName), an execution role granting dynamodb:PutItem/GetItem, and an AWS::Lambda::Function (python3.12) whose Environment.Variables.TABLE_NAME is `!Ref <table logical id>`; export `!Ref <table logical id>` as an output.
2. Poll DescribeStacks until CREATE_COMPLETE and read the TableName output.
3. Invoke the function (InvocationType=RequestResponse) with {"id": "<some-id>"}; the handler calls dynamodb PutItem with TableName=os.environ['TABLE_NAME'].
4. Observe the handler's response: AWS returns ok=True; the emulator returns ok=False with error ResourceNotFoundException.
5. For contrast, call DynamoDB DescribeTable/ListTables directly and compare the table names the service holds against the value the stack's `Ref` published.
