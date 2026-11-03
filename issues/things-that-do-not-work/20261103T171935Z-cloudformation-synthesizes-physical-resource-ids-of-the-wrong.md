Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-dcsr-stackid-reference-route
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-dcsr-stackid-reference-route cloudformation-dcsr-wrong-region-stackid cloudformation-list-summaries-key-set-and-count cloudformation-tsrb-list-stack-resources-bad-stack-references cloudformation-tsrb-deleted-stack-name-vs-stack-id cloudformation-tsrb-describe-resources-filter-mistakes cloudformation-tsrb-describe-truncates-then-list-paginates cloudformation-tsrb-template-summary-type-route
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|ListStackResources|2xx->2xx|$.ListStackResourcesResponse.ListStackResourcesResult.StackResourceSummaries.member[*].PhysicalResourceId
Issue-Key: cloudformation|DescribeStackResources|2xx->2xx|$.DescribeStackResourcesResponse.DescribeStackResourcesResult.StackResources.member[*].PhysicalResourceId
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# CloudFormation synthesizes physical resource ids of the wrong form for WaitConditionHandle and CDK metadata

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStackResources

## Description
For resource types that have no name-shaped physical id, the emulator falls back to one synthesized template, '<stackName>-<logicalId>-<hex suffix>', for every type. AWS's physical id for these types has a different form, so the recorded shape assertion fails on every resource row. This bundle carries 7 distinct causes (one per diverging operation/step). Related open issue 20261029T214321Z-cloudformation-aws-cloudformation-waitconditionhandle-ref-returns-the-logical.md covers the Ref value for WaitConditionHandle and 20261102T090835Z-cloudformation-dynamodb-lambda-logs-s3-cloudformation-rejects-the-aws-cdk-metadata-resource-type.md covers AWS::CDK::Metadata acceptance; neither owns these replay keys (owned_by is empty for all seven), so each must be closed here.

Cause 1 of 7 - ListStackResources, AWS::CDK::Metadata.
AWS: recorded behavior only - the fixture asserts the SHAPE of StackResourceSummaries[].PhysicalResourceId, and AWS's value for AWS::CDK::Metadata is an opaque short token, not a stack/logical-id-derived name. Smithy: StackResource.PhysicalResourceId is only documented as 'the name or unique identifier that corresponds to a physical instance ID', so the form is per resource type and comes from the recording.
Emulator: emits '<stackName>-<logicalId>-<12 hex chars>', which does not match the recorded shape.
Why: a caller that parses or round-trips a physical id (and `cdk diff`, which reads the metadata resource) is handed an id of a form AWS never produces.
Fix: the per-resource-type physical-id generator in the CloudFormation provisioner; the generic '<stack>-<logical>-<hex>' fallback must not be used for types with their own id form.
Traces: cloudformation-dcsr-stackid-reference-route#task#4 (cluster also covers cloudformation-dcsr-wrong-region-stackid#task#7, cloudformation-list-summaries-key-set-and-count#task#5, cloudformation-tsrb-list-stack-resources-bad-stack-references#task#7 and #task#8).

Cause 2 of 7 - DescribeStackResources, AWS::CDK::Metadata: same synthesized id in StackResources[].PhysicalResourceId.
Fix: same generator, read by DescribeStackResources' serializer.
Traces: cloudformation-dcsr-stackid-reference-route#task#5.

Cause 3 of 7 - ListStackResources, AWS::CloudFormation::WaitConditionHandle on a deleted stack read by name and by id.
AWS: a wait condition handle's physical id is the presigned callback URL CloudFormation issues for it (an https URL under the region's cloudformation-waitcondition bucket), not a name.
Emulator: '<stackName>-<logicalId>-<hex>'.
Why: the physical id IS the signal endpoint for this type, so a name-shaped id makes every signal path unusable and the stale-stack reverse lookup wrong.
Fix: the AWS::CloudFormation::WaitConditionHandle provisioner's physical-id assignment.
Traces: cloudformation-tsrb-deleted-stack-name-vs-stack-id#task#4.

Cause 4 of 7 - DescribeStackResources, WaitConditionHandle, with logical/physical filters: same synthesized id.
Traces: cloudformation-tsrb-describe-resources-filter-mistakes#task#6.

Cause 5 of 7 - DescribeStackResources on a 100-resource WaitConditionHandle stack: every row carries the synthesized id.
Traces: cloudformation-tsrb-describe-truncates-then-list-paginates#task#4.

Cause 6 of 7 - ListStackResources on that same stack, first page: every row carries the synthesized id.
Traces: cloudformation-tsrb-describe-truncates-then-list-paginates#task#5.

Cause 7 of 7 - ListStackResources on that same stack, continuation page: every row carries the synthesized id.
Traces: cloudformation-tsrb-describe-truncates-then-list-paginates#task#7.

## Evidence
Replay of simparity-wire-aws-v1, 7 divergence clusters, all stub_path='' and owned_by=[].
Every cluster reports PhysicalResourceId shape mismatches of the form: expected '<shaped-not-asserted>' (the recorded AWS id form), got '<stackName>-<logicalId>-<12 hex>' - e.g. a CDK metadata row and WaitConditionHandle rows numbered Handle000..Handle102 across the 100-resource stack's describe page, first list page and continuation page.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Cause 1: fixture cloudformation-dcsr-stackid-reference-route, step task#4 - deploy a CDK-synthesized stack and call ListStackResources, then inspect StackResourceSummaries[0].PhysicalResourceId.
3. Causes 2-7: replay the fixture#step bindings in trace_ids (DescribeStackResources on the same stack, and the WaitConditionHandle stacks' describe/list pages).
4. Compare each PhysicalResourceId's form against the recorded AWS value's form.
