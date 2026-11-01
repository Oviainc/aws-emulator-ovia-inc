Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.deploy-table-bucket-cfn-enforces-policy s3tables.deploy-table-bucket-then-create-namespace-and-table s3tables.deploy-table-bucket-via-cloudformation-behaves s3tables.deploy-table-bucket-via-cloudformation-then-behaves s3tables.deploy-table-via-cloudformation-then-policy-enforced
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables.deploy-table-bucket-cfn-enforces-policy|<genexpr>#10|KeyError|bucket_arn = next(o["OutputValue"] for o in outputs if o["OutputKey"] == "TableBucketArn")
Issue-Key: s3tables.deploy-table-bucket-then-create-namespace-and-table|_deploy_cfn#4|KeyError|arn_by_key = {o["OutputKey"]: o["OutputValue"] for o in outputs}
Issue-Key: s3tables.deploy-table-bucket-via-cloudformation-behaves|_deploy_cfn#4|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in resp["Stacks"][0].get("Outputs", [])}
Issue-Key: s3tables.deploy-table-bucket-via-cloudformation-then-behaves|run#9|KeyError|o["OutputKey"]: o["OutputValue"]
Issue-Key: s3tables.deploy-table-via-cloudformation-then-policy-enforced|run#13|KeyError|o["OutputKey"]: o["OutputValue"]
Issue-Commit: 2483fdb6af8e19e16b529c88e6a2504bdf5a7762

# s3tables via CloudFormation: Fn::GetAtt on AWS::S3Tables::TableBucket.TableBucketARN / Table.TableARN resolves to nothing, so stack Output rows are returned without OutputValue

> _Note: this issue was identified against the code base at commit `2483fdb6af8e19e16b529c88e6a2504bdf5a7762`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: a CloudFormation stack that declares an Output whose Value is `Fn::GetAtt` of an S3 Tables resource attribute (`!GetAtt TableBucket.TableBucketARN` for AWS::S3Tables::TableBucket, `!GetAtt Table.TableARN` for AWS::S3Tables::Table) reaches CREATE_COMPLETE, but the Output row DescribeStacks returns carries only `OutputKey` — the `OutputValue` member is absent entirely.
AWS: recorded behavior only (CloudFormation's Output shape is not in the s3tables model). A stack whose creation completed returns, for every declared Output, a row with both `OutputKey` and `OutputValue`, where OutputValue is the resolved attribute — for AWS::S3Tables::TableBucket.TableBucketARN the table bucket ARN of the form arn:<partition>:s3tables:<region>:<account-id>:bucket/<bucket-name> (the variable parts come from the stack's region/account and the TableBucketName property), and for AWS::S3Tables::Table.TableARN the table ARN .../bucket/<bucket-name>/table/<table-id>. The value is immediately usable as the `tableBucketARN` / `tableArn` input of the S3 Tables API. The same GetAtt is also consumed inside the template (AWS::S3Tables::Namespace / ::Table take `TableBucketARN: !GetAtt TableBucket.TableBucketARN`), so an unresolved attribute silently mis-provisions those resources too.
Emulator: the Output row is serialized with no `OutputValue` member at all, so `o["OutputValue"]` raises KeyError in every SDK that models it as present and `describe_stacks` gives a caller no way to obtain the ARN the stack created. Every one of the five CFN-deployment checks in this suite dies on exactly this access, before any s3tables assertion runs.
Why: a client cannot retrieve the identifier of a resource it just provisioned, so the entire "deploy with CloudFormation, then use the resource through its own API" path is unusable; templates that wire one S3 Tables resource to another through GetAtt also cannot be trusted to have resolved. The absence is worse than a wrong value because it breaks SDK consumers at the type level rather than returning something a client can inspect.
Fix: the GetAtt attribute resolution for the AWS::S3Tables::TableBucket and AWS::S3Tables::Table resource providers in CloudFormation, and the DescribeStacks output serializer, which must not emit an Output row whose value failed to resolve.
Traces: s3tables.deploy-table-bucket-cfn-enforces-policy, s3tables.deploy-table-bucket-then-create-namespace-and-table, s3tables.deploy-table-bucket-via-cloudformation-behaves, s3tables.deploy-table-bucket-via-cloudformation-then-behaves, s3tables.deploy-table-via-cloudformation-then-policy-enforced. NOTE: this is NOT the already-open issue 20261025T013226Z-cloudformation-s3-s3tables-s3vectors-cloudformation-aws-s3tables-tablebucket-physical-resource-id-ref.md — that one is about the resource row's PhysicalResourceId / `Ref` returning the bare bucket name instead of the ARN; here the `Fn::GetAtt` attribute is missing from the response altogether.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout. All five checks fail with `KeyError: 'OutputValue'` while reading the rows of `describe_stacks(StackName=...)["Stacks"][0]["Outputs"]` after the stack_create_complete waiter returned:
- s3tables.deploy-table-bucket-cfn-enforces-policy line 54: `bucket_arn = next(o["OutputValue"] for o in outputs if o["OutputKey"] == "TableBucketArn")` -> KeyError: 'OutputValue'. Template: `TableBucket: Type: AWS::S3Tables::TableBucket` with `Outputs: TableBucketArn: Value: !GetAtt TableBucket.TableBucketARN`.
- s3tables.deploy-table-bucket-then-create-namespace-and-table line 36 (`arn_by_key = {o["OutputKey"]: o["OutputValue"] for o in outputs}`) -> KeyError: 'OutputValue'. Template Outputs: {"TableBucketArn": {"Value": {"Fn::GetAtt": ["TableBucket", "TableBucketARN"]}}}.
- s3tables.deploy-table-bucket-via-cloudformation-behaves line 100 -> KeyError: 'OutputValue' (same GetAtt, and the template also feeds it to AWS::S3Tables::Namespace.TableBucketARN).
- s3tables.deploy-table-bucket-via-cloudformation-then-behaves -> KeyError: 'OutputValue' (`!GetAtt TableBucket.TableBucketARN`).
- s3tables.deploy-table-via-cloudformation-then-policy-enforced -> KeyError: 'OutputValue' (Outputs TableBucketArn `!GetAtt TableBucket.TableBucketARN` and TableArn `!GetAtt Table.TableARN`).
The OutputKey member is present (the comprehension reaches the OutputValue lookup), so the row exists and only the resolved value is missing. The stack names and bucket/table names in these runs are per-run values and are not part of the requirement; the requirement is that each declared Output row carries an OutputValue holding the resolved ARN.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-aws-v1.
2. create_stack with a template containing `TableBucket: {Type: AWS::S3Tables::TableBucket, Properties: {TableBucketName: <name>}}` and `Outputs: {TableBucketArn: {Value: !GetAtt TableBucket.TableBucketARN}}`; wait for stack_create_complete.
3. Call describe_stacks(StackName=<stack>) and inspect Stacks[0].Outputs — each row has OutputKey but no OutputValue; expect OutputValue to hold arn:<partition>:s3tables:<region>:<account>:bucket/<name>.
4. Repeat with an AWS::S3Tables::Table resource and `Outputs: {TableArn: {Value: !GetAtt Table.TableARN}}` — same omission.
