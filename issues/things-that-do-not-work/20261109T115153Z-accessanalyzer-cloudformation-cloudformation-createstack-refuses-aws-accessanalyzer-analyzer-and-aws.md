Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: accessanalyzer__cloudformation.deploy-accessanalyzer-analyzer-and-archiverule-round-trip-then-child-and-stack-delete accessanalyzer__cloudformation.deploy-accessanalyzer-analyzer-invalid-type-rejected-and-rolled-back
Issue-Key: accessanalyzer__cloudformation|CreateStack|ValidationError
Issue-Commit: a0b5d56b12e90d4bdac3fa9524e8b0649f05b4d6

# cloudformation: CreateStack refuses AWS::AccessAnalyzer::Analyzer and AWS::AccessAnalyzer::ArchiveRule as unrecognized resource types, so no Access Analyzer template can be deployed (1 cause)

> _Note: this issue was identified against the code base at commit `a0b5d56b12e90d4bdac3fa9524e8b0649f05b4d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the emulator's CloudFormation template resource-type registry does not know the AWS::AccessAnalyzer::* resource types, so every CreateStack whose template declares an Access Analyzer analyzer or archive rule is rejected at submit time. Failing checks: accessanalyzer__cloudformation.deploy-accessanalyzer-analyzer-and-archiverule-round-trip-then-child-and-stack-delete, accessanalyzer__cloudformation.deploy-accessanalyzer-analyzer-invalid-type-rejected-and-rolled-back.

AWS: AWS::AccessAnalyzer::Analyzer is a supported CloudFormation resource type, and AWS::AccessAnalyzer::ArchiveRule is supported as its child (it is also expressible as the Analyzer's ArchiveRules property). On real AWS, CreateStack with a template whose Resources declare these types is ACCEPTED: it returns 200 with a StackId and the stack proceeds to CREATE_COMPLETE, provisioning a real analyzer whose Ref and Fn::GetAtt Arn are the analyzer ARN (arn:<partition>:access-analyzer:<region>:<account>:analyzer/<name>) and whose archive rule Ref/Arn are <analyzer-arn>/archive-rule/<rule-name>, with GetAtt CreatedAt/UpdatedAt matching GetArchiveRule. Smithy (com-amazonaws-cloudformation-cloudformation.json): CreateStack models only AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException and TokenAlreadyExistsException — the "Template format error: Unrecognized resource types: [...]" ValidationError is CloudFormation's common query-protocol refusal, reserved for resource types that genuinely do not exist in the registry (its fixed text is "Template format error: Unrecognized resource types: [" ... "]", with the bracketed, comma-separated type names taken verbatim from the submitted template). Applying it to a real, supported type is the defect; the error code and message themselves are the right shape, they are simply raised for types that should resolve.

Emulator: CreateStack answers HTTP 400 ValidationError with message "Template format error: Unrecognized resource types: [AWS::AccessAnalyzer::Analyzer, AWS::AccessAnalyzer::ArchiveRule]" for the two-resource template, and "Template format error: Unrecognized resource types: [AWS::AccessAnalyzer::Analyzer]" for the single-resource one. No stack is created, so DescribeStacks/DescribeStackEvents, the Ref/GetAtt outputs, the rule-removing UpdateStack, and the DeleteStack teardown are never reached, and the invalid-Type rollback variant can never exercise its CREATE_FAILED/ROLLBACK_COMPLETE path either (the stack is refused before any resource is attempted). Both checks terminate at the very first CreateStack call.

Why: a client that provisions Access Analyzer through IaC — a CloudFormation template, a CDK app, or a SAM/nested stack that includes an organization or account analyzer and its archive rules — cannot deploy at all against the emulator. The refusal arrives at submit time with a template-format ValidationError, which a client reads as "my template is malformed / these types do not exist in this partition", so the failure is indistinguishable from a genuine typo in a Type string. There is no workaround: the type cannot be declared, so no stack, change set or drift read over Access Analyzer resources is reachable, and the entire validate→create→update→delete lifecycle for these resources is unobservable.

Fix: CloudFormation's template resource-type registry / resource-provider dispatch table — the component that resolves a template Resources[*].Type to a provisioning handler and emits the "Unrecognized resource types" ValidationError when it cannot. Register AWS::AccessAnalyzer::Analyzer (properties AnalyzerName, Type, Tags, ArchiveRules; Ref and GetAtt Arn both the analyzer ARN) and AWS::AccessAnalyzer::ArchiveRule (properties AnalyzerName, RuleName, Filter; Ref and GetAtt Arn the <analyzer-arn>/archive-rule/<rule-name> form, GetAtt CreatedAt/UpdatedAt from the rule's stored timestamps), backing them with the existing Access Analyzer CreateAnalyzer/CreateArchiveRule/DeleteArchiveRule/DeleteAnalyzer state so stack create, the child-dropping update and stack delete are reflected in ListAnalyzers/ListArchiveRules. Note: once the types resolve, these two checks also assert the analyzer's aws:cloudformation stack tags, the exact ResourceNotFoundException messages ("Analyzer not found", "Archive rule not found") and the invalid-Type CREATE_FAILED/ROLLBACK_COMPLETE event sequence — re-replay both checks to confirm the whole path, not just the acceptance of the type.

Related but not covering this: 20261109T113214Z-cloudformation-ivs-mediatailor-createstack-refuses-aws-ivs-adconfiguration.md and 20261109T113214Z-cloudformation-mediatailor-createstack-refuses-every-aws-mediatailor-resource.md report the same registry gap for other service namespaces; neither names AWS::AccessAnalyzer::*.

Traces: the two accessanalyzer__cloudformation checks listed above; both are [error] verdicts raised from botocore at the CreateStack call.

## Evidence
Expected (real AWS): CreateStack(StackName=sim2real-cfnaa-rt-…, TemplateBody={"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Analyzer":{"Type":"AWS::AccessAnalyzer::Analyzer","Properties":{"AnalyzerName":<name>,"Type":"ACCOUNT","Tags":[…]}},"ArchiveRule":{"Type":"AWS::AccessAnalyzer::ArchiveRule","DependsOn":"Analyzer","Properties":{"AnalyzerName":<name>,"RuleName":<rule>,"Filter":{"resourceType":{"Eq":["AWS::S3::Bucket"]}}}}},"Outputs":{…}}) -> HTTP 200, {"StackId": "arn:<partition>:cloudformation:<region>:<account>:stack/<name>/<uuid>"}; DescribeStacks then reaches StackStatus CREATE_COMPLETE and DescribeStackEvents contains ("Analyzer","CREATE_COMPLETE") then ("ArchiveRule","CREATE_COMPLETE").

Actual (emulator): HTTP 400, Error.Code = "ValidationError", Error.Message = "Template format error: Unrecognized resource types: [AWS::AccessAnalyzer::Analyzer, AWS::AccessAnalyzer::ArchiveRule]".
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::AccessAnalyzer::Analyzer, AWS::AccessAnalyzer::ArchiveRule]

And for the single-resource template of the second check:
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::AccessAnalyzer::Analyzer]

Message anatomy: "Template format error: Unrecognized resource types: [" + comma-separated type names + "]" is CloudFormation's fixed text; the bracketed names are values taken from the request's template. The divergence is not the wording — it is that supported types are placed in that list at all, so the call is refused instead of returning a StackId.

No stack id is ever minted, so every later assertion of both checks (Ref/GetAtt equality with the analyzer ARN, ListAnalyzers/ListArchiveRules cross-reads, GetArchiveRule CreatedAt/UpdatedAt vs the GetAtt outputs, the rule-dropping UpdateStack leaving createdAt unchanged, DeleteStack followed by "Analyzer not found") is unreached rather than wrong.

## Steps to Reproduce
1. Boot the emulator and point a botocore client at its cloudformation endpoint.
2. Call CreateStack with StackName=<any> and TemplateBody = {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Analyzer":{"Type":"AWS::AccessAnalyzer::Analyzer","Properties":{"AnalyzerName":"<name>","Type":"ACCOUNT"}}},"Outputs":{"AnalyzerArn":{"Value":{"Fn::GetAtt":["Analyzer","Arn"]}}}}.
3. Observe HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::AccessAnalyzer::Analyzer]" instead of a 200 with a StackId.
4. Add a second resource {"ArchiveRule":{"Type":"AWS::AccessAnalyzer::ArchiveRule","DependsOn":"Analyzer","Properties":{"AnalyzerName":"<name>","RuleName":"<rule>","Filter":{"resourceType":{"Eq":["AWS::S3::Bucket"]}}}}} and re-submit: both type names now appear in the same refusal.
5. Equivalently, replay accessanalyzer__cloudformation.deploy-accessanalyzer-analyzer-invalid-type-rejected-and-rolled-back and accessanalyzer__cloudformation.deploy-accessanalyzer-analyzer-and-archiverule-round-trip-then-child-and-stack-delete; both abort at the first CreateStack.
