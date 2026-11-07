Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.cfn-nested-stack-template-wrong-region-negative
Issue-Key: cloudformation__iam__lambda__s3.cfn-nested-stack-template-wrong-region-negative|run#13|AssertionError|assert failed == [expected_reason], (
Issue-Commit: 547f35aa751bdfccaa69888f930e73773cf7bea8

# cloudformation: an AWS::CloudFormation::Stack resource failure reason is wrapped in the registry handler envelope ("Resource handler returned message: ... (RequestToken: ..., HandlerErrorCode: ...)"), where AWS reports the bare reason

> _Note: this issue was identified against the code base at commit `547f35aa751bdfccaa69888f930e73773cf7bea8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
Cause 1 of 1: the ResourceStatusReason the emulator records for a failed nested-stack resource is the generic resource-provider envelope wrapping the real reason, instead of the reason itself.

AWS: recorded behavior only (stack events are modelled; the reason text is CloudFormation's own). A nested stack whose TemplateURL is an S3 ARN (`arn:aws:s3:::<bucket>/<key>`) rather than an HTTPS URL fails, and the DescribeStackEvents entry for that logical resource with ResourceStatus=CREATE_FAILED carries exactly the ResourceStatusReason "The location for an Amazon S3 bucket must start with https://" — all fixed text, no variable parts. AWS::CloudFormation::Stack is not a registry resource type, so its failure reason is not wrapped in the "Resource handler returned message: \"...\" (RequestToken: <token>, HandlerErrorCode: <code>)" envelope that AWS uses for registry-provider types (AWS::S3::Bucket and friends).

Emulator: the stack does now roll back and the underlying reason is right, but the recorded reason is `Resource handler returned message: "The location for an Amazon S3 bucket must start with https://" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`. Here the envelope text `Resource handler returned message: "` ... `" (RequestToken: ` ... `, HandlerErrorCode: InvalidRequest)` is fixed emulator-added text and the RequestToken is a per-run value; neither belongs in the reason AWS reports for this resource type.

Why: the CREATE_FAILED reason is the only machine-readable explanation a deploy pipeline has for why a nested stack failed. Clients and CI gates that match CloudFormation's exact reason for a bad TemplateURL (or display it to an operator) see an envelope that AWS never emits for AWS::CloudFormation::Stack, so exact-match assertions fail and log scrapers mis-attribute the failure to a resource provider.

Related open issue 20261104T232822Z-cloudformation-iam-lambda-s3-4-net-new-root.md (Cause 2) covers the earlier state of this same check — the S3-ARN TemplateURL being accepted so the stack never rolled back. That refusal now happens; the remaining divergence is the reason's wording, and that issue does not carry this check's replay key.

Fix: the CloudFormation stack provisioner's event recording / reason formatting — the handler envelope must be applied only to registry resource-provider failures, and AWS::CloudFormation::Stack (and other internal types) must record their reason verbatim.

Traces: cloudformation__iam__lambda__s3.cfn-nested-stack-template-wrong-region-negative.

## Evidence
Check cloudformation__iam__lambda__s3.cfn-nested-stack-template-wrong-region-negative [mismatch].

Expected: the DescribeStackEvents reasons for LogicalResourceId='NestedStack' with ResourceStatus='CREATE_FAILED' == ['The location for an Amazon S3 bucket must start with https://'].

Actual: AssertionError — "the NestedStack CREATE_FAILED reasons are ['Resource handler returned message: \"The location for an Amazon S3 bucket must start with https://\" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)'], expected ['The location for an Amazon S3 bucket must start with https://']". The parent did reach ROLLBACK_COMPLETE, so only the reason text diverges.

## Steps to Reproduce
1) CreateBucket and PutObject a child template (one AWS::SNS::Topic) at key child-template.json.
2) CreateStack a parent whose 'NestedStack' resource is an AWS::CloudFormation::Stack with TemplateURL = "arn:aws:s3:::<bucket>/child-template.json".
3) Poll DescribeStacks to ROLLBACK_COMPLETE.
4) Page DescribeStackEvents and read ResourceStatusReason for LogicalResourceId='NestedStack', ResourceStatus='CREATE_FAILED': it is the handler envelope rather than the bare reason.
