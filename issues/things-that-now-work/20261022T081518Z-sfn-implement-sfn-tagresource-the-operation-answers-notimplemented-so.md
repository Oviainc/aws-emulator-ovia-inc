Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resourcegroupstaggingapi__stepfunctions.state-machine-tags-visible-through-get-resources
Issue-Key: resourcegroupstaggingapi__stepfunctions|TagResource|NotImplemented
Issue-Commit: 5ef273b5ec187d9b55db2ccfdad00816d8c5c39a

# Implement sfn TagResource: the operation answers NotImplemented, so no state machine can be tagged natively

> _Note: this issue was identified against the code base at commit `5ef273b5ec187d9b55db2ccfdad00816d8c5c39a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: Step Functions' own TagResource is a scaffold stub.

AWS: TagResource is a bound Step Functions operation (awsJson1.0, X-Amz-Target AWSStepFunctions.TagResource) that adds a tag list to a state-machine or activity ARN and returns an empty 200 body; its modeled failures are the service's own client errors (ResourceNotFound, InvalidArn, TooManyTags). NotImplemented is not a Step Functions error code and no Step Functions operation can answer it. Tags written here are also what ListTagsForResource and the cross-service tagging index must report.
Emulator: "An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator" — a 501-class refusal from the generated scaffold rather than a tag write. The state machine and its IAM role are created successfully first; only the tag write is refused.
Why: a caller cannot tag a state machine at all, so neither Step Functions' ListTagsForResource nor tagging-API discovery can ever see a state-machine tag, and any ownership/cost-allocation or type-scoped discovery flow over Step Functions resources is unusable. The refusal is a server-class error no retry policy resolves.
Fix: the TagResource handler in the sfn service crate (the scaffolded ops/tag_resource stub) plus the state-machine tag state it must write, so ListTagsForResource and the tagging index both observe it. The report did not mark this failure as already filed, and no open issue names sfn TagResource. Note the failing check's id is prefixed resource-groups-tagging-api__sfn, but the refused call is Step Functions' own TagResource, so the defect belongs to sfn.
Traces: resource-groups-tagging-api__sfn.state-machine-tags-visible-through-get-resources (dies on sfn.tag_resource before any GetResources assertion runs).

## Evidence
Expected: tag_resource(resourceArn=<stateMachine arn>, tags=[{'key':..,'value':..}]) returns an empty 200 and the tag becomes visible through sfn ListTagsForResource and through ResourceGroupsTaggingAPI GetResources (including ResourceTypeFilters=['states:stateMachine']).
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator.

## Steps to Reproduce
1. Boot the emulator and point botocore 'stepfunctions' and 'iam' clients at it.
2. Create an IAM role trusted by states.amazonaws.com and a STANDARD state machine with a single Pass state.
3. Call tag_resource(resourceArn=<stateMachineArn>, tags=[{'key':'k','value':'v'}]).
4. Observe the NotImplemented refusal instead of an empty 200.
