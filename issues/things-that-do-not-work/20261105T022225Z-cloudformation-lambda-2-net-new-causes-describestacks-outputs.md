Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda.cfnuf-failed-alias-update-leaves-live-alias-serving-previous-version cloudformation__lambda.e2e-cfnuf-fix-cloudformation-update-failed-stack-lifecycle
Issue-Key: cloudformation__lambda.cfnuf-failed-alias-update-leaves-live-alias-serving-previous-version|outputs#5|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in self.describe().get("Outputs") or []}
Issue-Key: cloudformation__lambda|GetAlias|ResourceNotFoundException
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# cloudformation__lambda: 2 net-new causes — DescribeStacks Outputs omit OutputValue, and a stack-declared AWS::Lambda::Alias does not exist after CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Two distinct root causes. Neither failing check is marked OWNED or ALREADY FILED in the replay report.

Cause 1 of 2: DescribeStacks returns Outputs entries without OutputValue.
AWS: Smithy com.amazonaws.cloudformation#Output has members OutputKey, OutputValue, Description, ExportName; AWS populates OutputValue with the resolved value of each template Output.
Emulator: the member is absent from each Output entry, so a client that reads Outputs[i]["OutputValue"] raises KeyError: 'OutputValue' right after the stack settles.
Why: the function version and alias names the stack publishes cannot be read, so nothing downstream can address the deployed resources; the failure is a hard client crash.
Fix: the CloudFormation DescribeStacks response serializer / the stack-output resolution that builds Stack.Outputs. The same symptom is described for other scenarios in the open issue 20261104T232822Z-cloudformation-lambda-s3-sqs-2-net-new-root.md, which does not carry this check's replay key.
Traces: cloudformation__lambda.cfnuf-failed-alias-update-leaves-live-alias-serving-previous-version.

Cause 2 of 2: a CloudFormation-deployed AWS::Lambda::Alias is reported created but does not exist.
AWS: Smithy lambda GetAlias (GET /2015-03-31/functions/{FunctionName}/aliases/{Name}) returns 200 with AliasArn, Name, FunctionVersion and RevisionId for an alias that exists, and 404 ResourceNotFoundException only when it does not. A stack that declares AWS::Lambda::Version plus AWS::Lambda::Alias and reaches CREATE_COMPLETE has created the alias, so GetAlias on it succeeds.
Emulator: the deploy phase reaches a successful stack status but the declared alias is missing — GetAlias answers 404 ResourceNotFoundException with message `Cannot find alias arn: <alias ARN>` (the fixed text is `Cannot find alias arn: `; the variable part is the fully qualified `arn:aws:lambda:<region>:<account>:function:<function>:<alias>` built from the request).
Why: a client that deploys versioned Lambda behind an alias and then routes traffic through it cannot use the alias the template declared — every subsequent GetAlias/UpdateAlias/Invoke on the qualifier is refused, and the whole alias-update lifecycle (the thing this scenario group exercises) is unreachable.
Fix: the CloudFormation resource provider for AWS::Lambda::Alias (and its ordering against AWS::Lambda::Version), which must create the alias in Lambda's state and report its ARN as the resource's physical id.
Traces: cloudformation__lambda.e2e-cfnuf-fix-cloudformation-update-failed-stack-lifecycle.

## Evidence
Cause 1 — expected: Stacks[0].Outputs[i]["OutputValue"] present. Actual: `KeyError: 'OutputValue'` (replay, check cfnuf-failed-alias-update-leaves-live-alias-serving-previous-version line 29).

Cause 2 — expected: GetAlias returns 200 with AliasArn/Name/FunctionVersion for the stack-declared alias. Actual: `[phase 1/5 deploy] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetAlias operation: Cannot find alias arn: arn:aws:lambda:<region>:<account>:function:<function>:<alias>` — the function/alias names and account are per-run values; the 404 status, the ResourceNotFoundException code and the `Cannot find alias arn: <arn>` wording are the requirement.

## Steps to Reproduce
1. CreateStack with a template declaring an AWS::Lambda::Function, an AWS::Lambda::Version of it, an AWS::Lambda::Alias pointing at that version, and Outputs exporting the function name and alias name.
2. Wait for a terminal CREATE status and call DescribeStacks — each Outputs entry has OutputKey but no OutputValue (cause 1).
3. Call lambda GetAlias with FunctionName/Name taken from the template — it answers 404 ResourceNotFoundException "Cannot find alias arn: ..." although the stack reported the alias created (cause 2).
