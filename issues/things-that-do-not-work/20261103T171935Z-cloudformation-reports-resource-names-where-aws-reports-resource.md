Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-cfns3cnt-bucket-side-reverse-lookup
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-cfns3cnt-bucket-side-reverse-lookup cloudformation-cfns3cnt-template-summary-and-describe-route cloudformation-cfns3cnt-deleted-bucket-stack-handles cloudformation-cfns3cnt-reference-list-then-classify-variants cloudformation-cfns3cnt-update-adds-table-bucket-into-count cloudformation-cfnuf-change-set-preview-then-execute cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward cloudformation-sfn-owning-stack-reverse-lookup cloudformation-sfn-out-of-band-delete-drift
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DescribeStackResources|2xx->2xx|$.DescribeStackResourcesResponse.DescribeStackResourcesResult.StackResources.member[LogicalResourceId="Tab"].PhysicalResourceId
Issue-Key: cloudformation|ListStackResources|2xx->2xx|$.ListStackResourcesResponse.ListStackResourcesResult.StackResourceSummaries.member.PhysicalResourceId
Issue-Key: cloudformation|ListStackResources|2xx->2xx|$.ListStackResourcesResponse.ListStackResourcesResult.StackResourceSummaries.member[LogicalResourceId="Tab"].PhysicalResourceId
Issue-Key: cloudformation|DescribeChangeSet|2xx->2xx|$.DescribeChangeSetResponse.DescribeChangeSetResult.Changes.member.ResourceChange.Details.member
Issue-Key: cloudformation|DescribeChangeSet|2xx->2xx|$.DescribeChangeSetResponse.DescribeChangeSetResult.Changes.member.ResourceChange.PhysicalResourceId
Issue-Key: cloudformation|DescribeStackResource|2xx->2xx|$.DescribeStackResourceResponse.DescribeStackResourceResult.StackResourceDetail.PhysicalResourceId
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.Outputs.member.OutputValue
Issue-Key: cloudformation|DescribeStackResources|2xx->2xx|$.DescribeStackResourcesResponse.DescribeStackResourcesResult.StackResources.member.PhysicalResourceId
Issue-Related: 20261029T171940Z-cloudformation-change-set-previews-operation-events-and-the.md
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# CloudFormation reports resource names where AWS reports resource ARNs

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResources

## Description
For resource types whose AWS physical resource id is an ARN, the emulator stores and reports the resource's NAME instead. The same substitution leaks into stack Outputs resolved from those resources. This bundle carries 7 distinct causes. Related open issues 20261102T051310Z-cloudformation-s3-s3tables-s3vectors-cloudformation-aws-s3tables-tablebucket-physical-resource-id-ref-15f3b6.md, 20261102T101202Z-cloudformation-s3-s3tables-s3vectors-2-net-new-root.md and 20261102T101202Z-cloudformation-stepfunctions-3-net-new-root-causes-role.md describe neighbouring symptoms but own none of these replay keys (owned_by is empty for all seven), so each must be closed here.

Cause 1 of 7 - DescribeStackResources, AWS::S3Tables::TableBucket.
AWS: StackResource.PhysicalResourceId for a table bucket is the table-bucket ARN, arn:<partition>:s3tables:<region>:<account>:bucket/<bucketName>. Smithy: StackResource.PhysicalResourceId is documented as 'the name or unique identifier that corresponds to a physical instance ID of a resource supported by CloudFormation', and for S3 Tables that identifier is the ARN the S3 Tables API accepts.
Emulator: returns the bare bucket name.
Why: the physical id is the handle a caller feeds to the owning service; with a name instead of an ARN, S3 Tables refuses the follow-up call and every stack-to-resource reverse lookup breaks.
Fix: the AWS::S3Tables::TableBucket resource provisioner's physical-id assignment in CloudFormation state.
Traces: cloudformation-cfns3cnt-bucket-side-reverse-lookup#task#6 (cluster also covers #task#8 and cloudformation-cfns3cnt-template-summary-and-describe-route#task#6).

Cause 2 of 7 - ListStackResources, AWS::S3Tables::TableBucket on a stack whose bucket was deleted out of band: StackResourceSummaries[].PhysicalResourceId must be the table-bucket ARN; emulator returns the name.
Fix: same provisioner; ListStackResources' serializer reads the stored id.
Traces: cloudformation-cfns3cnt-deleted-bucket-stack-handles#task#15.

Cause 3 of 7 - ListStackResources, AWS::S3Tables::TableBucket on a live stack: same ARN-vs-name substitution.
Traces: cloudformation-cfns3cnt-reference-list-then-classify-variants#task#9.

Cause 4 of 7 - DescribeChangeSet, AWS::Lambda::Alias.
AWS: Changes[].ResourceChange.PhysicalResourceId for an alias is the alias ARN, arn:<partition>:lambda:<region>:<account>:function:<functionName>:<aliasName>. In the same change AWS emits ONE Details entry for the directly modified property (Evaluation 'Static', ChangeSource 'DirectModification', Target.Name the property, Target.RequiresRecreation 'Never').
Emulator: PhysicalResourceId is the alias name alone, and Details carries an extra second entry for a property the request did not change (Evaluation 'Dynamic', ChangeSource 'ResourceAttribute', CausingEntity the version resource).
Why: a change-set preview is read to decide whether to execute; a bare alias name is not resolvable by Lambda and the invented Details row makes the preview claim a modification AWS does not report.
Fix: the AWS::Lambda::Alias change-set differ and its physical-id resolution.
Traces: cloudformation-cfnuf-change-set-preview-then-execute#task#8.

Cause 5 of 7 - DescribeStackResource, AWS::Lambda::Alias.
AWS: StackResourceDetail.PhysicalResourceId is the alias ARN, and StackResourceDetail.Metadata is present (the empty-object literal when the template declares no resource Metadata).
Emulator: PhysicalResourceId is the alias name and Metadata is omitted entirely.
Why: the single-resource view is the detailed one; omitting Metadata makes a declared-metadata check indistinguishable from a resource with none.
Fix: DescribeStackResource's serializer plus the alias provisioner's physical-id assignment.
Traces: cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward#task#11 (cluster also covers cloudformation-sfn-owning-stack-reverse-lookup#task#9).

Cause 6 of 7 - DescribeStacks Outputs for AWS::StepFunctions::StateMachine.
AWS: an Output whose Value is a Ref/Fn::GetAtt of a state machine resolves to the state machine ARN, arn:<partition>:states:<region>:<account>:stateMachine:<name>.
Emulator: OutputValue is the state machine name.
Why: downstream Step Functions calls take an ARN; a name-valued output cannot be used and a consuming stack's import is wrong too.
Fix: the AWS::StepFunctions::StateMachine provisioner's physical id / Ref resolution feeding output evaluation.
Traces: cloudformation-sfn-out-of-band-delete-drift#task#5.

Cause 7 of 7 - DescribeStackResources for AWS::StepFunctions::StateMachine: PhysicalResourceId must be the state machine ARN; emulator returns the name.
Traces: cloudformation-sfn-out-of-band-delete-drift#task#6.

## Evidence
Replay of simparity-wire-aws-v1, 7 divergence clusters, all stub_path='' and owned_by=[].
Causes 1-3: expected PhysicalResourceId 'arn:aws:s3tables:<region>:<account>:bucket/<name>', got '<name>'.
Cause 4: expected ResourceChange.PhysicalResourceId 'arn:aws:lambda:<region>:<account>:function:<fn>:LATEST', got 'LATEST'; expected Details to be a single {Evaluation 'Static', ChangeSource 'DirectModification', Target{Attribute 'Properties', Name 'Description', RequiresRecreation 'Never'}}, got that entry plus a second {Target{Name 'FunctionVersion'}, Evaluation 'Dynamic', ChangeSource 'ResourceAttribute', CausingEntity '<version>.Version'}.
Cause 5: expected StackResourceDetail.PhysicalResourceId the alias ARN and Metadata '{}', got 'LATEST' and Metadata absent.
Cause 6: expected OutputValue 'arn:aws:states:<region>:<account>:stateMachine:<name>', got '<name>'.
Cause 7: expected StackResources[].PhysicalResourceId the state machine ARN, got '<name>'.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Cause 1: fixture cloudformation-cfns3cnt-bucket-side-reverse-lookup, step task#6 - create a stack with an AWS::S3Tables::TableBucket, then DescribeStackResources and read StackResources[LogicalResourceId='Tab'].PhysicalResourceId.
3. Causes 2-7: replay the fixture#step bindings in trace_ids (ListStackResources on the table-bucket stacks, DescribeChangeSet / DescribeStackResource on the Lambda-alias stack, DescribeStacks and DescribeStackResources on the Step Functions stack).
4. Compare each PhysicalResourceId / OutputValue and the change-set Details list against the recorded AWS response.
