Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__s3__ssm.config-remediation-execution-without-configuration-rejected-and-resource-untouched
Issue-Key: ssm|DescribeDocument|InvalidDocument
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# No AWS-owned SSM documents exist: DescribeDocument answers InvalidDocument for AWS-ConfigureS3BucketVersioning, so every Config remediation flow that targets an AWS-managed automation document is unreachable

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDocument

## Description
Cause 1 of 1: the SSM document store contains only documents a caller created, so the AWS-owned (AWS-*) automation documents that exist in every account and region are absent.
AWS: DescribeDocument resolves AWS-owned public documents by name without the caller ever creating them — AWS-ConfigureS3BucketVersioning is an AWS-managed Automation runbook present in every account/region and is the standard TargetId for an AWS Config remediation configuration on AWS::S3::Bucket. A name that does exist returns the document description (Name, DocumentType=Automation, Owner=Amazon, Status=Active); InvalidDocument is reserved for a document that genuinely does not exist. Recorded behavior: the check resolves this document successfully before it attaches a remediation configuration.
Emulator: ssm DescribeDocument answers the modeled InvalidDocument error — 'Document AWS-ConfigureS3BucketVersioning doesn't exist' — and the check aborts there, before it can assert that StartRemediationExecution without a remediation configuration is refused and that the S3 bucket is left untouched. The handler's own contract confirms the cause: services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_document.rs 'reports what CreateDocument persisted, from the one store in crate::state', i.e. nothing is seeded.
Why: any client that references an AWS-managed runbook — Config remediation configurations, Automation executions, Systems Manager console flows, CloudFormation templates naming AWS-* documents — gets a not-found for a document AWS guarantees exists, and must be rewritten to create a private copy to work against the emulator. The whole Config-remediation-on-S3 path is unreachable, including its negative leg (remediation without a configuration must be refused with the resource untouched).
Fix: seed the AWS-owned document catalogue into the SSM document store so AWS-* documents resolve read-only (services/service-com-amazonaws-ssm-amazonssm/src/state.rs document store plus services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_document.rs and the list/get document paths); at minimum the AWS-managed Automation runbooks Config remediation targets, starting with AWS-ConfigureS3BucketVersioning.
Traces: config__s3__ssm.config-remediation-execution-without-configuration-rejected-and-resource-untouched.

Note on attribution: the failing check id's leading token is the multi-service token 'config__s3__ssm', which does not join by exact equality to any entry returned by the per-service bootstrap status (that listing uses crate-style tokens such as service-com-amazonaws-ssm-amazonssm). The services this check touches all answer on the wire — the failure IS a typed SSM response, not a NotImplemented stub — so it is filed as a fidelity divergence of the SSM document surface reached from a Config remediation flow.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false; 45/108 checks passed).

[error] config__s3__ssm.config-remediation-execution-without-configuration-rejected-and-resource-untouched:
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.InvalidDocument: An error occurred (InvalidDocument) when calling the DescribeDocument operation: Document AWS-ConfigureS3BucketVersioning doesn't exist

The check uses DescribeDocument only to resolve the AWS-managed runbook it will name as the remediation TargetId; the assertions it exists to make (StartRemediationExecution with no remediation configuration is refused, and the target S3 bucket's versioning is unchanged afterwards) are never reached.

Source grounding in the checkout: services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_document.rs lines 7-10 — 'It reports what `CreateDocument` persisted, from the one store in [`crate::state`]' — so no AWS-owned document can ever resolve.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Call ssm DescribeDocument --name AWS-ConfigureS3BucketVersioning against the emulator with no prior CreateDocument: it answers InvalidDocument ("Document AWS-ConfigureS3BucketVersioning doesn't exist") instead of returning the AWS-owned Automation document.
3. Replay config__s3__ssm.config-remediation-execution-without-configuration-rejected-and-resource-untouched: it aborts at that DescribeDocument call.
4. Same for any other AWS-managed runbook name (e.g. AWS-PublishSNSNotification) used as a Config remediation TargetId.
