Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.cfn-foreach-language-extensions-expands-to-24-working-queues-and-rejects-missing-capability cloudformation__s3__sqs.deploy-cloudformation-3-include-transform-injects-queue-properties cloudformation__s3__sqs.deploy-cloudformation-3-include-transform-missing-s3-key-fails cloudformation.get-template-stages cloudformation__dynamodb__s3.cfn-cross-stack-export-dynamodb-table-name-to-s3-stack-negative cloudformation.ssm-dynamic-reference-resolves-at-deploy-and-stays-stale cloudformation.cross-stack-import-value cloudformation.get-stack-output-reads-unexported-output-as-snapshot
Issue-Key: cloudformation__sns__sqs.cfn-foreach-language-extensions-expands-to-24-working-queues-and-rejects-missing-capability|run#12|AssertionError|assert ids == expected_ids, (
Issue-Key: cloudformation__s3__sqs.deploy-cloudformation-3-include-transform-injects-queue-properties|run#14|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Key: cloudformation__s3__sqs.deploy-cloudformation-3-include-transform-missing-s3-key-fails|run#14|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation.get-template-stages|run#15|AssertionError|assert "Fn::ToJsonString" not in str(processed["TemplateBody"]), processed["TemplateBody"]
Issue-Key: cloudformation__dynamodb__s3.cfn-cross-stack-export-dynamodb-table-name-to-s3-stack-negative|run#11|AssertionError|assert tags["SourceTable"] == table_name, (
Issue-Key: cloudformation.ssm-dynamic-reference-resolves-at-deploy-and-stays-stale|run#12|AssertionError|assert created["StackStatus"] == "CREATE_COMPLETE", (
Issue-Key: cloudformation.cross-stack-import-value|run#7|KeyError|o["OutputKey"]: o["OutputValue"]
Issue-Key: cloudformation.get-stack-output-reads-unexported-output-as-snapshot|_outputs#13|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# CloudFormation template transforms and intrinsics are not processed: AWS::Include and Fn::ForEach are passed through verbatim, GetTemplate(Processed) returns the original, Fn::ImportValue resolves to empty, {{resolve:ssm:...}} is refused, and Outputs carry no OutputValue (6 causes)

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTemplate

## Description
This issue bundles 6 net-new root causes in CloudFormation's template-processing stage. The open issue 20261022T043551Z-cloudformation-read-paths-return-unprocessed-or-unstored-stack.md covers the read-path family generally and 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md covers OutputValue for other check ids; neither carries the replay keys below.

Cause 1 of 6: Fn::ForEach is not expanded.
AWS: with 'Transform: AWS::LanguageExtensions', an Fn::ForEach in Resources expands to one resource per collection element; the stack then contains every generated logical id.
Emulator: only the hand-written sibling resource exists — the stack records logical ids ['Sentinel'] where 24 generated queues plus the sentinel were owed.
Why: templates written against the language extensions deploy a fraction of their resources while still reporting success.
Fix: the AWS::LanguageExtensions transform processor in the cloudformation template pipeline.
Traces: cloudformation__sns__sqs.cfn-foreach-language-extensions-expands-to-24-working-queues-and-rejects-missing-capability.

Cause 2 of 6: the AWS::Include transform (Fn::Transform) is not processed.
AWS: Fn::Transform with AWS::Include splices the referenced S3 snippet into the template before provisioning; a valid snippet yields CREATE_COMPLETE, and a snippet naming a missing S3 key fails the stack with a transform error.
Emulator: 'Fn::Transform' is handed to the resource provider as if it were a property — both the valid and the missing-key case fail identically with 'The following resource(s) failed to create: [Q]. InvalidAttributeName: Unknown Attribute Fn::Transform.'
Why: the valid template cannot deploy at all, and the negative case is indistinguishable from the positive one, so the error taxonomy is wrong too.
Fix: macro/transform expansion (AWS::Include) in the template pipeline, before resource provisioning.
Traces: cloudformation__s3__sqs.deploy-cloudformation-3-include-transform-injects-queue-properties, cloudformation__s3__sqs.deploy-cloudformation-3-include-transform-missing-s3-key-fails.

Cause 3 of 6: GetTemplate(TemplateStage=Processed) returns the original body.
AWS: Smithy: GetTemplate.TemplateStage (enum TemplateStage = Original | Processed) and output StagesAvailable. The Processed stage returns the template after transforms/intrinsics that CloudFormation resolves at processing time.
Emulator: the Processed body still contains the unresolved 'Fn::ToJsonString' intrinsic that the Original body had.
Why: tooling that diffs the processed template (CDK/CLI workflows) cannot see what was actually deployed.
Fix: store the processed template alongside the original and serve it for TemplateStage=Processed in the GetTemplate handler.
Traces: cloudformation.get-template-stages.

Cause 4 of 6: Fn::ImportValue resolves to an empty string inside a resource property.
AWS: an importing stack's Fn::ImportValue of an exported Output yields the exported value; the recorded case puts it on an S3 bucket tag.
Emulator: the tag is created with value '' while the export carries the table name.
Why: cross-stack wiring silently produces blank values instead of either the value or a refusal, so dependent resources are misconfigured with no error.
Fix: Fn::ImportValue resolution in the intrinsic evaluator (and it must fail the stack when the export is absent).
Traces: cloudformation__dynamodb__s3.cfn-cross-stack-export-dynamodb-table-name-to-s3-stack-negative.

Cause 5 of 6: the {{resolve:ssm:...}} dynamic reference is misparsed as a nested parameter.
AWS: a dynamic reference in a resource property is resolved against SSM at deploy time and the resolved value is then frozen in the stack's state.
Emulator: the resource fails with 'ValidationException: Value carries a value wrapped in double brackets: a parameter value can not reference or nest another parameter' and the stack ends CREATE_FAILED.
Why: every template using dynamic references is undeployable. (A sibling defect for ssm-secure is already open as 20261024T030130Z-cloudformation-rds-ssm-cloudformation-resolves-resolve-ssm-secure-against-a-plain.md; this is the plain ssm form and the opposite failure mode.)
Fix: dynamic-reference resolution in the template pipeline, ahead of property validation.
Traces: cloudformation.ssm-dynamic-reference-resolves-at-deploy-and-stays-stale.

Cause 6 of 6: DescribeStacks returns Outputs entries with no OutputValue.
AWS: Smithy: Output = {OutputKey, OutputValue, Description, ExportName}; a resolved Output always carries OutputValue.
Emulator: the member is absent, so consumers raise KeyError 'OutputValue'.
Why: no stack output can be read back, which breaks every output-driven workflow (including Fn::ImportValue consumers).
Fix: Output serialization in the DescribeStacks handler plus Output value resolution in the stack engine.
Traces: cloudformation.cross-stack-import-value, cloudformation.get-stack-output-reads-unexported-output-as-snapshot.

## Evidence
Replayed suite, service cloudformation:
- cfn-foreach-language-extensions...: "stack records logical ids ['Sentinel']; expected ['Qa'...'Qx','Sentinel'] (missing 24)".
- deploy-cloudformation-3-include-transform-injects-queue-properties and ...-missing-s3-key-fails: both "got 'CREATE_FAILED' with stack-level reasons ['User Initiated', \"The following resource(s) failed to create: [Q]. InvalidAttributeName: Unknown Attribute Fn::Transform.\"]".
- get-template-stages: the returned template still contains OrderedDict({'Fn::ToJsonString': ...}) in Outputs.
- cfn-cross-stack-export-dynamodb-table-name-to-s3-stack-negative: "bucket tag SourceTable: expected 'sim2real-xsvc-table-...', got ''".
- ssm-dynamic-reference-resolves-at-deploy-and-stays-stale: "expected CREATE_COMPLETE, got 'CREATE_FAILED': ... ValidationException: Value carries a value wrapped in double brackets".
- cross-stack-import-value and get-stack-output-reads-unexported-output-as-snapshot: "KeyError: 'OutputValue'".

## Steps to Reproduce
1. Boot the emulator, replay the suite, re-run the trace_ids.
2. Cause 2 repro: PutObject a snippet to S3, CreateStack with a resource whose Properties are {"Fn::Transform": {"Name": "AWS::Include", "Parameters": {"Location": "s3://.../snippet.json"}}} — AWS splices it, the emulator fails the resource with InvalidAttributeName.
3. Cause 3 repro: CreateStack with Transform: AWS::LanguageExtensions and an Fn::ToJsonString Output, then GetTemplate(StackName, TemplateStage='Processed').
4. Cause 6 repro: CreateStack with any Outputs entry, then DescribeStacks and read Stacks[0].Outputs[0].OutputValue.
