Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.describe-instance-patches-invalid-instance-rejected ssm.describe-inventory-deletions-unknown-id-rejected ssm.create-document-duplicate-name-fails ssm.deploy-maintenance-window-via-cloudformation-then-registerable ssm.deploy-ssm-maintenancewindow-stack-lifecycle ssm.public-parameter-reference-data-readable-not-owned
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.describe-instance-patches-invalid-instance-rejected|run#10|AssertionError|assert message == (
Issue-Key: ssm.describe-inventory-deletions-unknown-id-rejected|run#5|AssertionError|assert error["Message"] == (
Issue-Key: ssm.create-document-duplicate-name-fails|run#11|AssertionError|assert message == f"Document with same name {name} already exists", message
Issue-Key: ssm|GetMaintenanceWindow|ValidationException
Issue-Key: ssm|CreateStack|ValidationError
Issue-Key: ssm.public-parameter-reference-data-readable-not-owned|run#3|AssertionError|assert len(public) >= 5, (
Issue-Commit: 69e3918bc598d03b2c669434388419738bf97909

# SSM ValidationException constraint text, DocumentAlreadyExists wording, the CloudFormation maintenance-window surface and the public /aws/service hierarchy (5 causes)

> _Note: this issue was identified against the code base at commit `69e3918bc598d03b2c669434388419738bf97909`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstancePatches

## Description
Five further root causes outside the Parameter Store read/write path. 5 CAUSES.

Cause 1 of 5 — the shared ValidationException constraint formatter publishes the Smithy @pattern literal with its outer anchors and reports only one of several violated constraints.
AWS: the message is '<N> validation error(s) detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: <constraint sentence>', one clause per violated constraint joined by '; ', with N the number of clauses; the regex is printed WITHOUT the model's outer ^...$ anchors. For DescribeInstancePatches with a malformed InstanceId AWS prints two clauses: "2 validation errors detected: Value '<v>' at 'instanceId' failed to satisfy constraint: Member must have length less than or equal to 20; Value '<v>' at 'instanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: (^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)" (Smithy: InstanceId @pattern ^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$ — the max-length-20 rule is recorded behavior only). For DescribeInventoryDeletions with a malformed DeletionId AWS prints one clause with the unanchored UUID regex: "1 validation error detected: Value '<v>' at 'deletionId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}" (Smithy: DeletionId @pattern ^[a-f0-9]{8}-...-[a-f0-9]{12}$).
Emulator: the status (400) and the class are right, but it emits '1 validation error detected: ...' with the length clause dropped, and prints the pattern with the model's '^'/'$' still attached ('^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$', '^[a-f0-9]{8}-...-[a-f0-9]{12}$').
Why: a client that surfaces or matches the printed regex sees a pattern AWS never prints, and one that counts or parses the reported violations sees one of two.
Fix: the ssm validation layer's constraint-message formatter (strip the model's outer anchors; emit every violated constraint) and the per-operation input validation that must apply the length rule as well as the pattern. The open wire issue 20261103T173026Z-ssm-validationexception-text-misrenders-constraints-anchored-patterns-dropped.md describes the same formatter; it does not carry these behavioral check ids.
Traces: ssm.describe-instance-patches-invalid-instance-rejected, ssm.describe-inventory-deletions-unknown-id-rejected.

Cause 2 of 5 — CreateDocument's duplicate-name refusal message.
AWS: 400 DocumentAlreadyExists (Smithy: CreateDocument error DocumentAlreadyExists, httpError 400) with 'Document with same name <name> already exists' — fixed text around the requested document name, and no region in it.
Emulator: 400 with 'Document <name> already exists in us-west-2' — authored word order plus a region the service never prints.
Why: a caller matching AWS's sentence (or parsing the name out of it) fails, and the injected region makes the message environment-dependent.
Fix: the CreateDocument handler's already-exists error construction.
Traces: ssm.create-document-duplicate-name-fails.

Cause 3 of 5 — a CloudFormation-created AWS::SSM::MaintenanceWindow gets its template Name as its physical id, so SSM cannot be asked about it.
AWS: the resource's physical id is the service-minted window id matching ^mw-[0-9a-f]{17}$, so GetMaintenanceWindow(WindowId=<the stack's physical id / Ref>) succeeds and the window is registerable.
Emulator: the physical id is the window's Name, so GetMaintenanceWindow is refused 400 ValidationException reporting that the value fails the windowId length (<= 20) and pattern (^mw-[0-9a-f]{17}$) constraints — the window CloudFormation reports as created is unreachable through the SSM API.
Why: every post-deploy step that follows a Ref or a stack output into SSM (register target, register task, read the window) fails, so a CFN-provisioned maintenance window is unusable.
Fix: the CloudFormation resource provider for AWS::SSM::MaintenanceWindow — create the window through SSM and record the minted mw- id as the physical resource id.
Traces: ssm.deploy-maintenance-window-via-cloudformation-then-registerable.

Cause 4 of 5 — CloudFormation does not recognise AWS::SSM::MaintenanceWindowTarget.
AWS: a template declaring AWS::SSM::MaintenanceWindowTarget is accepted and the stack reaches CREATE_COMPLETE with the target registered on the window.
Emulator: CreateStack is refused 400 ValidationError with 'Template format error: Unrecognized resource types: [AWS::SSM::MaintenanceWindowTarget]'.
Why: the maintenance-window stack lifecycle cannot even be created, so no window/target/task deployment path exists.
Fix: register the AWS::SSM::MaintenanceWindowTarget resource type (and its sibling task type) in the CloudFormation resource-type registry and give it a provider that calls RegisterTargetWithMaintenanceWindow.
Traces: ssm.deploy-ssm-maintenancewindow-stack-lifecycle.

Cause 5 of 5 — the AWS-published public parameter hierarchy is empty under path reads.
AWS: GetParametersByPath on '/aws/service/ami-amazon-linux-latest' returns AWS's published Amazon Linux AMI parameters (many entries), readable by any caller and not owned by the account.
Emulator: the path read returns 0 parameters.
Why: every workflow that resolves a latest-AMI id from the public namespace (launch templates, CFN SSM parameter types, resolve:ssm references) has nothing to read.
Fix: seed and serve the AWS-published /aws/service hierarchy in the parameter store's read paths. Open issues 20261103T173026Z-ssm-public-aws-published-aws-service-parameter-hierarchy-is.md and 20261103T154947Z-ssm-getparameter-cannot-resolve-aws-published-public-aws.md describe the same gap from other corpora but do not carry this check id.
Traces: ssm.public-parameter-reference-data-readable-not-owned.

## Evidence
Replay of simparity-behavioral-aws-v1; all 6 checks are listed under '### ssm' in the suite report.

Cause 1 — ssm.describe-instance-patches-invalid-instance-rejected: expected "2 validation errors detected: Value 'sim2real-behav-not-an-instance-id' at 'instanceId' failed to satisfy constraint: Member must have length less than or equal to 20; Value '...' at 'instanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: (^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)", got "1 validation error detected: Value 'sim2real-behav-not-an-instance-id' at 'instanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$". ssm.describe-inventory-deletions-unknown-id-rejected: expected the same sentence with the pattern '[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}', got it rendered as '^[a-f0-9]{8}-...-[a-f0-9]{12}$'.
Cause 2 — ssm.create-document-duplicate-name-fails: assert message == 'Document with same name <name> already exists' → got 'Document sim2real-behav-probe-doc-dup-<id> already exists in us-west-2'.
Cause 3 — ssm.deploy-maintenance-window-via-cloudformation-then-registerable: uncaught botocore.errorfactory.ValidationException on GetMaintenanceWindow — "2 validation errors detected: Value 'sim2real-cfn-mw-<id>' at 'windowId' failed to satisfy constraint: Member must have length less than or equal to 20; Value 'sim2real-cfn-mw-<id>' at 'windowId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^mw-[0-9a-f]{17}$" — i.e. the physical id handed back by the stack is the template Name.
Cause 4 — ssm.deploy-ssm-maintenancewindow-stack-lifecycle: botocore ClientError on CreateStack — 'An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::SSM::MaintenanceWindowTarget]'.
Cause 5 — ssm.public-parameter-reference-data-readable-not-owned: "/aws/service/ami-amazon-linux-latest must expose AWS's published Amazon Linux AMI parameters; got 0: []".

Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json (DescribeInstancePatches.InstanceId @pattern ^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$; DescribeInventoryDeletions.DeletionId @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$ with error InvalidDeletionIdException; CreateDocument error DocumentAlreadyExists).

## Steps to Reproduce
1. Build and boot the emulator from this checkout. 2. Replay simparity-behavioral-aws-v1 and re-run the check ids in trace_ids. 3. Per cause: DescribeInstancePatches(InstanceId='sim2real-behav-not-an-instance-id') and DescribeInventoryDeletions(DeletionId='not-a-valid-deletion-id'), comparing Error.Message clause count and regex rendering (cause 1); CreateDocument twice with the same Name (cause 2); CreateStack with an AWS::SSM::MaintenanceWindow resource, then GetMaintenanceWindow with the stack's physical resource id / Ref (cause 3); CreateStack with a template declaring AWS::SSM::MaintenanceWindowTarget (cause 4); GetParametersByPath(Path='/aws/service/ami-amazon-linux-latest', Recursive=True) (cause 5).
