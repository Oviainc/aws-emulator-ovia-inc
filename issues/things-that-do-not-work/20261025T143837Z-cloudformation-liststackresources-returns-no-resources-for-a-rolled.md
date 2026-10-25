Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back
Issue-Key: cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back|run#13|AssertionError|assert rows == [("AWS::S3Vectors::VectorBucket", "DELETE_COMPLETE")], f"rows are {rows!r}"
Issue-Commit: 2fdf5a5bbec8785c488e040ea81b8d4ee3df8e7f

# CloudFormation ListStackResources returns no resources for a rolled-back stack: the failed resource's DELETE_COMPLETE row is dropped instead of retained (1 cause)

> _Note: this issue was identified against the code base at commit `2fdf5a5bbec8785c488e040ea81b8d4ee3df8e7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStackResources

## Description
This issue carries 1 net-new root cause in CloudFormation's resource-view surface, grounded in cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back. The adjacent open issue 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md (its Cause 3) describes DescribeStackResources listing [] for a stack that ended in a *_FAILED state, and 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back.md describes rollback never happening at all; neither carries this check's replay key, and in this run the stack DID roll back to ROLLBACK_COMPLETE and the resource's CREATE_FAILED event WAS recorded — the remaining divergence is that ListStackResources serves an empty summary list for the rolled-back stack.

Cause 1 of 1: a ROLLBACK_COMPLETE stack lists zero stack resources; the rolled-back resource's row is dropped rather than retained with its terminal rollback status.
AWS: Smithy: ListStackResources input = {StackName (required), NextToken}, output = {StackResourceSummaries: list<StackResourceSummary>, NextToken}; it declares NO modeled errors, so a stack that exists always answers 200 with a summary list. StackResourceSummary requires LogicalResourceId, ResourceType, LastUpdatedTimestamp and ResourceStatus, and ResourceStatus is the enum that includes CREATE_FAILED, DELETE_IN_PROGRESS and DELETE_COMPLETE. On real AWS, when a resource fails to create and the stack rolls back, CloudFormation deletes the failed resource and leaves its row in the stack's resource list at its terminal rollback status — for a resource that never materialised and was cleaned up during ROLLBACK_IN_PROGRESS that status is DELETE_COMPLETE. The row therefore keeps naming the declared ResourceType (here AWS::S3Vectors::VectorBucket) and its LogicalResourceId from the template, and it is the only way a client can see WHICH declared resource the rollback disposed of.
Emulator: for the same stack — DescribeStacks StackStatus is ROLLBACK_COMPLETE, DescribeStackEvents contains a CREATE_FAILED event for the logical id — ListStackResources (paginated, every page) returns StackResourceSummaries == [], i.e. the check sees `rows are []` where AWS yields exactly one row [("AWS::S3Vectors::VectorBucket", "DELETE_COMPLETE")]. No status code or error code diverges: the call is a 200 with an empty list, so the defect is the state the handler reads, not a refusal.
Why: a client that inventories a stack's resources after a deploy fails can no longer tell what the stack declared or what became of it. Any derivation that classifies stacks by the resource types they contain (the standard 'which stacks own a vector / table / regular bucket' breakdown this check performs) silently sees a rolled-back stack as declaring nothing, and tooling that waits for per-resource DELETE_COMPLETE to confirm cleanup never observes it. The rows are also the only per-resource view left once events are trimmed, so failure diagnosis after rollback is lost.
Fix: the stack provisioning/rollback engine's resource state — rollback must transition each provisioned-or-attempted resource record to its terminal DELETE_* status and KEEP the record on the stack rather than removing it, and the ListStackResources handler (and the shared resource-summary serializer it uses with DescribeStackResources) must serve records for stacks in ROLLBACK_COMPLETE/*_FAILED states, not only for stacks in CREATE_COMPLETE/UPDATE_COMPLETE.
Traces: cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back.

## Evidence
Replayed suite, check cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back [mismatch]:

  AssertionError: rows are []
  at: assert rows == [("AWS::S3Vectors::VectorBucket", "DELETE_COMPLETE")], f"rows are {rows!r}"

Where `rows` is built from every page of `list_stack_resources(StackName=<bad stack>)` as [(r["ResourceType"], r["ResourceStatus"]) for r in summaries].

Assertions that PASSED before it, establishing that only the resource-row retention diverges:
- the sibling stack declaring a valid lower-case VectorBucketName settled at CREATE_COMPLETE;
- the stack declaring VectorBucketName='Upper_Case_Vec' (breaking S3 Vectors' lower-case naming rule) settled at ROLLBACK_COMPLETE, not CREATE_COMPLETE;
- DescribeStackEvents for that stack contains a CREATE_FAILED ResourceStatus for LogicalResourceId 'Vectors'.

Expected (AWS): ListStackResources on the rolled-back stack returns one StackResourceSummary — LogicalResourceId 'Vectors' (from the template), ResourceType 'AWS::S3Vectors::VectorBucket' (the declared type), ResourceStatus 'DELETE_COMPLETE' (the terminal status of a resource disposed of during rollback), plus the required LastUpdatedTimestamp.
Actual (emulator): HTTP 200 with StackResourceSummaries = [] on every page.

Smithy grounding: ListStackResources declares no errors and returns StackResourceSummaries; StackResourceSummary requires LogicalResourceId/ResourceType/ResourceStatus/LastUpdatedTimestamp; ResourceStatus enum includes CREATE_FAILED, DELETE_IN_PROGRESS, DELETE_COMPLETE. The only values quoted above that come from the request/state rather than the model are the logical id ('Vectors') and the resource type; the generated stack-name prefix in the recording is a per-run value and is not part of the requirement.

## Steps to Reproduce
1. Boot the emulator and point a cloudformation client at it.
2. CreateStack with a template whose single resource is {"Vectors": {"Type": "AWS::S3Vectors::VectorBucket", "Properties": {"VectorBucketName": "Upper_Case_Vec"}}} — an upper-case/underscored name that S3 Vectors must refuse.
3. Poll DescribeStacks until the stack leaves *_IN_PROGRESS; it reaches ROLLBACK_COMPLETE (correct).
4. DescribeStackEvents(StackName=<stack>) and confirm a CREATE_FAILED event exists for LogicalResourceId 'Vectors' (correct).
5. Call ListStackResources(StackName=<stack>) and drain the paginator. The emulator returns StackResourceSummaries = []; AWS returns one summary for logical id 'Vectors', ResourceType 'AWS::S3Vectors::VectorBucket', ResourceStatus 'DELETE_COMPLETE'.
6. Re-replay cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back to confirm the fix.
