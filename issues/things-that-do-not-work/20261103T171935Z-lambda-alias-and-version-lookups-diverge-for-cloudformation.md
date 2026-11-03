Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-cfnuf-lambda-crosscheck-then-delete-from-update-failed
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-cfnuf-lambda-crosscheck-then-delete-from-update-failed
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: lambda|GetAlias|2xx->4xx|$.AliasArn
Issue-Key: lambda|GetAlias|2xx->4xx|$.Description
Issue-Key: lambda|GetAlias|2xx->4xx|$.FunctionVersion
Issue-Key: lambda|GetAlias|2xx->4xx|$.Name
Issue-Key: lambda|GetAlias|2xx->4xx|$.RevisionId
Issue-Key: lambda|GetAlias|2xx->4xx|status
Issue-Key: lambda|GetFunction|4xx->4xx|$.Message
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# Lambda alias and version lookups diverge for CloudFormation-created functions

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAlias

## Description
Lambda cannot resolve the alias a CloudFormation stack created, and its function-not-found message drops the qualifier the request asked for. This bundle carries 2 distinct causes.

Cause 1 of 2 - GetAlias for an alias created by an AWS::Lambda::Alias resource.
AWS: 200 with AliasArn (arn:<partition>:lambda:<region>:<account>:function:<functionName>:<aliasName>), Name (the alias name), FunctionVersion (the version the alias points at), Description and RevisionId. Smithy: GetAlias is GET /2015-03-31/functions/{FunctionName}/aliases/{Name} with success code 200 and output AliasConfiguration{AliasArn, Name, FunctionVersion, Description, RoutingConfig, RevisionId}; its ResourceNotFoundException (404) is for an alias that does not exist.
Emulator: 404, so the alias the stack reports as created does not exist in Lambda.
Why: the alias is the invocation target a template publishes; a 404 means nothing can invoke the deployed function through it, and the cross-check between the stack's resource view and Lambda's own view is impossible.
Fix: the AWS::Lambda::Alias provisioning path must register the alias in Lambda state that GetAlias' handler reads.
Traces: cloudformation-cfnuf-lambda-crosscheck-then-delete-from-update-failed#task#6.

Cause 2 of 2 - GetFunction's not-found message drops the Qualifier.
AWS: 404 ResourceNotFoundException with Message 'Function not found: <functionArn>:<qualifier>' - fixed text 'Function not found: ' followed by the function ARN AND the qualifier from the request, so the message names the exact version that is missing. Smithy: GetFunction takes a Qualifier httpQuery member and declares ResourceNotFoundException (404).
Emulator: 404 with 'Function not found: <functionArn>' - the qualifier is dropped.
Why: when a deployment fails because a version does not exist, the qualifier in the message is the only thing identifying WHICH version; without it the diagnosis points at the function instead of the version, and CloudFormation's resource-status reason (which embeds this message) is wrong too.
Fix: GetFunction's not-found error construction must include the requested Qualifier in the resource identifier it reports.
Traces: cloudformation-cfnuf-lambda-crosscheck-then-delete-from-update-failed#task#7.

## Evidence
Replay of simparity-wire-aws-v1, 2 divergence clusters, both stub_path='' and owned_by=[].
Cause 1: expected status 200 with AliasArn 'arn:aws:lambda:<region>:<account>:function:<fn>:LATEST', Name 'LATEST', FunctionVersion '1', Description '' and RevisionId; got 404 with all members absent.
Cause 2: expected Message 'Function not found: arn:aws:lambda:<region>:<account>:function:<fn>:<qualifier>', got 'Function not found: arn:aws:lambda:<region>:<account>:function:<fn>' (status 404 on both sides).

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Cause 1: fixture cloudformation-cfnuf-lambda-crosscheck-then-delete-from-update-failed, step task#6 - deploy a stack with AWS::Lambda::Function/Version/Alias, then call Lambda GetAlias for the alias name.
3. Cause 2: same fixture, step task#7 - call GetFunction with a Qualifier naming a version that does not exist and read Message.
4. Compare status, AliasConfiguration members and the not-found Message against the recorded AWS responses.
