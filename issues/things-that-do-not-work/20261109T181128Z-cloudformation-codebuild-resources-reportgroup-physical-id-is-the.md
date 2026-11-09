Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__codebuild.deploy-cfn-codebuild-reportgroup-resource-lifecycle cloudformation__codebuild.deploy-cfn-codebuild-reportgroup-s3-export-without-destination-rejected cloudformation__codebuild.deploy-cfn-codebuild-project-invalid-compute-type-rejected
Issue-Key: cloudformation__codebuild.deploy-cfn-codebuild-reportgroup-resource-lifecycle|run#11|AssertionError|assert physical_id == want_arn, (
Issue-Key: cloudformation__codebuild.deploy-cfn-codebuild-reportgroup-s3-export-without-destination-rejected|run#12|AssertionError|assert batch_resp["reportGroupsNotFound"] == [], (
Issue-Key: cloudformation__codebuild.deploy-cfn-codebuild-project-invalid-compute-type-rejected|run#19|AssertionError|assert match is not None, f"CREATE_FAILED reason is not a resource-handler message: {reason!r}"
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# CloudFormation/CodeBuild resources: ReportGroup physical id is the name instead of its ARN, and a failed resource's reason omits the service clause and CodeBuild's own wording (3 causes)

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
THREE net-new root causes in the CloudFormation emulation of CodeBuild resource types. ALL THREE must be closed before this issue is resolved. Related open issue 20261026T054805Z-codebuild-listsharedreportgroups-leaks-the-owner-s-own-report.md mentions a CFN-deployed AWS::CodeBuild::ReportGroup being unaddressable by PutResourcePolicy but carries no replay key for the checks below and does not cover them.

Cause 1 of 3: the PhysicalResourceId (and therefore Ref and Fn::GetAtt Arn) of an AWS::CodeBuild::ReportGroup is the report group's NAME, not its ARN.
AWS: recorded behavior only for the CFN resource provider; the CodeBuild API itself addresses report groups by ARN — Smithy BatchGetReportGroups(reportGroupArns: ReportGroupArns) -> {reportGroups, reportGroupsNotFound}, and ReportGroup.arn is the identifier. AWS returns the report group's ARN as the resource's PhysicalResourceId, so Ref/GetAtt Arn and DescribeStackResource all yield arn:<partition>:codebuild:<region>:<account>:report-group/<name>, where the fixed text is the ARN template and the variable parts are the partition of the stack id, the request's region and account, and the Name property of the resource.
Emulator: DescribeStackResource returns PhysicalResourceId '<report-group-name>' (the bare Name property). Passing that value straight to BatchGetReportGroups — the only thing a template consumer can do with a Ref — puts the name in reportGroupsNotFound and returns no report group, so the resource reads as absent even though the stack is CREATE_COMPLETE.
Why: every consumer of a CFN-managed report group (Ref in a policy resource, GetAtt Arn passed to BatchGetReportGroups/PutResourcePolicy, an export consumed by another stack) receives a value that is not an ARN and that the CodeBuild API cannot resolve, so a deployed report group is unusable and a round-trip read reports it missing.
Fix: the AWS::CodeBuild::ReportGroup resource provider in the CloudFormation emulation — set the resource's physical id to the created report group's ARN (the value CodeBuild's CreateReportGroup returns) and derive Ref/GetAtt Arn from it.
Traces: cloudformation__codebuild.deploy-cfn-codebuild-reportgroup-resource-lifecycle (assertion on PhysicalResourceId), cloudformation__codebuild.deploy-cfn-codebuild-reportgroup-s3-export-without-destination-rejected (its positive control reads the same physical id back through BatchGetReportGroups and gets reportGroupsNotFound).

Cause 2 of 3: a CREATE_FAILED event's ResourceStatusReason omits the '(Service: ..., Status Code: ..., Request ID: ...)' clause AWS wraps around the underlying service error.
AWS: recorded behavior only (the reason string is not modeled). AWS wraps a resource-handler failure as: Resource handler returned message: "<service message> (Service: <ServiceId>, Status Code: <http status>, Request ID: <request id>)" (RequestToken: <token>, HandlerErrorCode: <code>). The fixed text is 'Resource handler returned message: ', ' (Service: ', ', Status Code: ', ', Request ID: ', ') (RequestToken: ', ', HandlerErrorCode: ', ')'; the variable parts are the service's own message, the service id (CodeBuild here), the HTTP status of the refusal (400), and the per-call request id and request token.
Emulator: the reason is 'Resource handler returned message: "<message>" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)' — the whole '(Service: ..., Status Code: ..., Request ID: ...)' clause is missing, so a client parsing the reason cannot recover which service refused the resource or with what status.
Why: tooling and operators parse this reason to attribute a stack failure to a service and a status code; without the clause the failure is unattributable, and any client regex built against AWS's format fails to match at all.
Fix: the reason/ResourceStatusReason formatting in the CloudFormation resource-handler error path — wrap the downstream service error with its service id, HTTP status and request id before appending the RequestToken/HandlerErrorCode suffix.
Traces: cloudformation__codebuild.deploy-cfn-codebuild-project-invalid-compute-type-rejected.

Cause 3 of 3: the CodeBuild message carried inside that reason is a generic Smithy enum-validation string instead of CodeBuild's own refusal wording.
AWS: Smithy ProjectEnvironment.computeType targets ComputeType @enum [BUILD_GENERAL1_SMALL, BUILD_GENERAL1_MEDIUM, BUILD_GENERAL1_LARGE, BUILD_GENERAL1_XLARGE, BUILD_GENERAL1_2XLARGE, BUILD_LAMBDA_1GB, BUILD_LAMBDA_2GB, BUILD_LAMBDA_4GB, BUILD_LAMBDA_8GB, BUILD_LAMBDA_10GB, ATTRIBUTE_BASED_COMPUTE, CUSTOM_INSTANCE_TYPE]; CreateProject refuses an out-of-set compute type with 400 InvalidInputException and the message 'Invalid compute type provided' (wholly fixed text, no interpolation), which CFN surfaces with HandlerErrorCode InvalidRequest.
Emulator: the message is '1 validation error detected: Value <value> at \'environment.computeType\' failed to satisfy constraint: Member must satisfy enum value set: [ATTRIBUTE_BASED_COMPUTE, ...]' — the framework's generic constraint report, naming the member path and the whole enum set, rather than CodeBuild's sentence.
Why: a client matching CodeBuild's documented refusal text (directly on CreateProject, or inside a stack event) never recognises the condition, and the message it does get describes a modelled constraint path that CodeBuild never emits.
Fix: the CreateProject compute-type validation in the codebuild crate — emit InvalidInputException with CodeBuild's own message instead of letting the generic enum-constraint reporter answer.
Traces: cloudformation__codebuild.deploy-cfn-codebuild-project-invalid-compute-type-rejected.

## Evidence
Replay of the behavioral suite (simparity-behavioral-aws-v1) against the emulator built from this checkout; per-service line: cloudformation__codebuild 1/4 (25%), 3 failed.

Cause 1 — cloudformation__codebuild.deploy-cfn-codebuild-reportgroup-resource-lifecycle: AssertionError 'expected the PhysicalResourceId to be the report group's documented ARN, got <report-group-name>'; the check compared it to arn:<partition>:codebuild:<region>:<account>:report-group/<name> built from the stack id's partition and the run's region/account.
Cause 1 — cloudformation__codebuild.deploy-cfn-codebuild-reportgroup-s3-export-without-destination-rejected: AssertionError 'positive control: expected the report group found, got not-found [<report-group-name>]' — BatchGetReportGroups(reportGroupArns=[DescribeStackResource PhysicalResourceId]) returned that value in reportGroupsNotFound while the stack was CREATE_COMPLETE.
Causes 2 and 3 — cloudformation__codebuild.deploy-cfn-codebuild-project-invalid-compute-type-rejected: AssertionError 'CREATE_FAILED reason is not a resource-handler message', actual reason 'Resource handler returned message: "1 validation error detected: Value \'BUILD_GENERAL1_NONEXISTENT\' at \'environment.computeType\' failed to satisfy constraint: Member must satisfy enum value set: [ATTRIBUTE_BASED_COMPUTE, BUILD_GENERAL1_2XLARGE, BUILD_GENERAL1_LARGE, BUILD_GENERAL1_MEDIUM, BUILD_GENERAL1_SMALL, BUILD_GENERAL1_XLARGE, BUILD_LAMBDA_10GB, BUILD_LAMBDA_1GB, BUILD_LAMBDA_2GB, BUILD_LAMBDA_4GB, BUILD_LAMBDA_8GB, CUSTOM_INSTANCE_TYPE]" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. Expected shape: 'Resource handler returned message: "Invalid compute type provided (Service: CodeBuild, Status Code: 400, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — ComputeType enum values; BatchGetReportGroups(reportGroupArns) -> {reportGroups, reportGroupsNotFound}.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Cause 1: CreateStack with a template containing an AWS::CodeBuild::ReportGroup (Type TEST, ExportConfig NO_EXPORT) and outputs for Ref and Fn::GetAtt Arn; wait for CREATE_COMPLETE. DescribeStackResource(LogicalResourceId='ReportGroup') — expect PhysicalResourceId arn:aws:codebuild:<region>:<account>:report-group/<name>; observe the bare <name>. Pass that value to BatchGetReportGroups(reportGroupArns=[...]) — expect one reportGroups entry; observe it in reportGroupsNotFound.
3. Causes 2 and 3: CreateStack (DisableRollback=true) with an AWS::CodeBuild::Project whose Environment.ComputeType is 'BUILD_GENERAL1_NONEXISTENT'; wait for CREATE_FAILED and read the Project resource's CREATE_FAILED ResourceStatusReason via DescribeStackEvents.
4. Expect 'Resource handler returned message: "Invalid compute type provided (Service: CodeBuild, Status Code: 400, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'; observe the generic enum-validation message and no '(Service: ..., Status Code: ..., Request ID: ...)' clause.
5. Re-replay the behavioral checks listed in trace_ids to confirm.
