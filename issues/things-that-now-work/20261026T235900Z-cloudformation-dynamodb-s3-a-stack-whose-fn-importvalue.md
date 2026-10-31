Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__s3.cfn-cross-stack-export-dynamodb-table-name-to-s3-stack-negative
Issue-Key: cloudformation__dynamodb__s3.cfn-cross-stack-export-dynamodb-table-name-to-s3-stack-negative|run#21|AssertionError|assert final_status == "ROLLBACK_COMPLETE", (
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# cloudformation__dynamodb__s3: a stack whose Fn::ImportValue names an export that was never declared reaches CREATE_COMPLETE instead of failing and rolling back

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1 — an unresolvable Fn::ImportValue is resolved to an empty/default value instead of failing the resource.
AWS: recorded behavior only — CreateStack on a template whose resource property is {"Fn::ImportValue": "<export-name-never-declared-by-any-stack>"} fails: the import cannot be resolved, so the stack does not reach CREATE_COMPLETE and settles in ROLLBACK_COMPLETE (AWS reports the unresolved export as the failure reason, naming the export). The positive control in the same check — the identical template against a REAL export published by a live stack — succeeds and the importing resource carries the imported value.
Emulator: the stack reaches `CREATE_COMPLETE`. The missing cross-stack export is silently resolved (to an empty or default value) and the dependent resource is created from it.
Why: this is overly permissive and silently corrupting: a typo'd or not-yet-deployed export produces a stack that reports success while the dependent resource was configured with a value that does not exist (here an S3 bucket tagged with the imported DynamoDB table name). Deployment pipelines that rely on CloudFormation refusing an unsatisfied cross-stack dependency ship a broken estate and get no signal at all.
Fix: template intrinsic resolution in the cloudformation crate — Fn::ImportValue must look the name up in the account/Region export index and fail the resource (and hence the stack) when no live export carries it, instead of yielding an empty value.
Traces: cloudformation__dynamodb__s3.cfn-cross-stack-export-dynamodb-table-name-to-s3-stack-negative

## Evidence
cloudformation__dynamodb__s3.cfn-cross-stack-export-dynamodb-table-name-to-s3-stack-negative:
  assert final_status == "ROLLBACK_COMPLETE"
  AssertionError: stack referencing a never-declared export: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'

The check first deploys an exporter stack (AWS::DynamoDB::Table with Outputs.Export.Name = <unique export name>) as its positive control, then deploys a second stack whose S3 bucket tag value is {"Fn::ImportValue": <a never-declared export name>} and asserts the CREATE fails.

## Steps to Reproduce
1. CreateStack with a template whose resource property value is {"Fn::ImportValue": "sim2real-xsvc-export-does-not-exist"} (no stack anywhere declares that export).
2. Poll DescribeStacks until a CREATE terminal status: expect ROLLBACK_COMPLETE with a failure reason naming the unresolved export; the emulator reports CREATE_COMPLETE and creates the dependent resource.
3. Control: declare the export from a live stack and repeat — that case must still succeed with the imported value applied.
