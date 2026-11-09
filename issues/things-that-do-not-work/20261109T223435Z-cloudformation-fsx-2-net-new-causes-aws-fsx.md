Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__fsx.deploy-fsx-lustre-data-repository-association-round-trip-and-delete cloudformation__fsx.deploy-fsx-lustre-filesystem-invalid-perunitstoragethroughput-rejected
Issue-Key: cloudformation__fsx|CreateStack|ValidationError
Issue-Key: cloudformation__fsx.deploy-fsx-lustre-filesystem-invalid-perunitstoragethroughput-rejected|run#10|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__fsx: 2 net-new causes — AWS::FSx::DataRepositoryAssociation unregistered, and an invalid PerUnitStorageThroughput is accepted instead of rolling the stack back

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct root causes. Each is numbered with its own failing check id; both must be closed before this issue is done.

Cause 1 of 2: AWS::FSx::DataRepositoryAssociation is not a recognized resource type.
AWS: it is a public CloudFormation resource type, so CreateStack accepts a template declaring it and provisions the association against the Lustre file system.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [AWS::FSx::DataRepositoryAssociation]` — fixed text plus the bracketed type list from the request.
Why: Lustre data-repository associations cannot be deployed through CloudFormation at all.
Fix: register AWS::FSx::DataRepositoryAssociation in the CloudFormation resource-type registry and add its resource handler.
Traces: cloudformation__fsx.deploy-fsx-lustre-data-repository-association-round-trip-and-delete.

Cause 2 of 2: an invalid PerUnitStorageThroughput on an FSx Lustre file system is accepted and the stack reaches CREATE_COMPLETE.
AWS: FSx refuses a PerUnitStorageThroughput value outside the set allowed for the chosen deployment type; CloudFormation then marks the resource CREATE_FAILED and the stack ROLLBACK_COMPLETE.
Emulator: the stack ends `CREATE_COMPLETE`.
Why: a template that AWS rejects appears to deploy, so the client never learns the throughput value is invalid and believes it provisioned a file system with a configuration FSx would not accept.
Fix: the AWS::FSx::FileSystem resource handler's property validation / propagation of the FSx CreateFileSystem refusal.
Traces: cloudformation__fsx.deploy-fsx-lustre-filesystem-invalid-perunitstoragethroughput-rejected.

## Evidence
Cause 1: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::FSx::DataRepositoryAssociation].
Cause 2: AssertionError: rejected stack ended 'CREATE_COMPLETE' — expected ROLLBACK_COMPLETE with a CREATE_FAILED event naming the FSx refusal of the PerUnitStorageThroughput value.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Cause 1: CreateStack with a template declaring AWS::FSx::DataRepositoryAssociation.
3. Cause 2: CreateStack with an AWS::FSx::FileSystem (FileSystemType Lustre) whose PerUnitStorageThroughput is not allowed for its DeploymentType, and watch the final stack status.
