Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.deploy-cfn-stream-tags-applied
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|ListTagsForResource|ResourceNotFoundException
Issue-Commit: af62a377edfa6e5f87943800a55de7e350a7269a

# kinesis: a CloudFormation-deployed AWS::Kinesis::Stream is still never created, so ListTagsForResource on the stack's stream ARN answers ResourceNotFoundException instead of the CFN-declared tags

> _Note: this issue was identified against the code base at commit `af62a377edfa6e5f87943800a55de7e350a7269a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
One net-new root cause in the `kinesis` service (Cause 1 of 1), grounded in the failing behavioral check kinesis.deploy-cfn-stream-tags-applied.

=== Cause 1 of 1: the CloudFormation provisioner for AWS::Kinesis::Stream registers no stream, so the Tags property is unobservable through ListTagsForResource ===
Failing check: kinesis.deploy-cfn-stream-tags-applied.

AWS: a stack that declares `Type: AWS::Kinesis::Stream` with `ShardCount` and a `Tags` list only reaches CREATE_COMPLETE once the stream exists; `Ref` resolves to the stream name and `!GetAtt Stream.Arn` to that stream's ARN. kinesis ListTagsForResource called with that ARN then answers HTTP 200 with a `Tags` list that contains every tag the template declared (Smithy: ListTagsForResource input ResourceARN is required, @length 1..2048, pattern `^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$`; output member `Tags` is a TagList of 0..200 entries; modeled errors AccessDenied/InvalidArgument/LimitExceeded/ResourceInUse/ResourceNotFound, all HTTP 400). After DeleteStack the same call is refused HTTP 400 `ResourceNotFoundException` with message `Stream <stream-name> under account <account-id> not found.` — fixed text `Stream `, ` under account `, ` not found.`, with the stream name taken from the request's ARN and the account id from the caller's account. The AWS::Kinesis::Stream resource contract itself is in the CloudFormation resource-type registry, not the kinesis Smithy model, so for the provisioning half this is recorded behavior only.

Emulator: the stack reaches CREATE_COMPLETE and returns both Outputs (StreamName and a well-formed StreamArn), but no stream is registered in kinesis state. The very first call against the stack's own output is refused HTTP 400 `ResourceNotFoundException`: `Stream sim2real-behav-kinesis-cfntags-<id>-Sim2RealStream-<id> under account 000000000000 not found.` — i.e. the refusal wording is now AWS's, but it is produced for a stream the stack claimed to create. The check therefore dies on its first assertion and neither the tag-propagation contract nor the post-DeleteStack refusal is graded.

Why: infrastructure-as-code is the primary way streams and their tags are created. A client gets CREATE_COMPLETE plus a usable-looking ARN, and then every control-plane and data-plane call against that ARN behaves as if the stream never existed — tag-based discovery, cost allocation and tag-driven cleanup all see nothing, with no failure signal anywhere on the stack.

Fix: the CloudFormation resource provisioner for `AWS::Kinesis::Stream` — the resource-type handler in the cloudformation service crate that executes a stack's resources — must create the stream in kinesis state (name, shard count, stream mode, retention, encryption) AND apply the resource's `Tags` property to that stream's tag set before the stack is reported CREATE_COMPLETE, derive `Ref`/`GetAtt Arn` from the created stream, and delete it on stack deletion. The kinesis CreateStream / TagResource / ListTagsForResource handlers answer correctly when called directly, so no change is needed in them.

Relationship to an already-filed issue: open issue 20261027T011654Z-kinesis-9-net-new-root-causes-cloudformation-aws.md describes this same provisioning no-op as its Cause 1, but its Issue-Trace-Ids carry only kinesis.deploy-cfn-kinesis-stream-put-get-records, kinesis.deploy-cfn-stream-encryption-enforced, kinesis.deploy-cfn-stream-put-get-records-round-trip and kinesis.deploy-kinesis-stream-cfn-behavioral-put-get — the tag-propagation check below is absent from it, so a fixer closing that issue would never re-replay it and the Tags half of the resource contract would stay unverified. This issue exists to carry that replay key; both should be closed by the same single fix.

## Evidence
From the replayed behavioral suite (simparity-behavioral-ga-prod-v1), 101/224 passed.

[error] kinesis.deploy-cfn-stream-tags-applied
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the ListTagsForResource operation: Stream sim2real-behav-kinesis-cfntags-ab2fe08d966c-Sim2RealStream-47BE84391B8D under account 000000000000 not found.

Expected: HTTP 200 with Tags containing {"sim2real-behav-check": "cfn-tags-applied"} — the tag the template declared on the AWS::Kinesis::Stream resource — for the ARN the stack published as its StreamArn output, after the stack reported CREATE_COMPLETE.
Actual: HTTP 400 ResourceNotFoundException for that same ARN; the stream named by the stack's own outputs does not exist in kinesis state. (The refusal's wording itself matches AWS's template `Stream <name> under account <account> not found.`; the divergence is that it is returned at all, for a live stack's resource.)
Not graded as a consequence: the post-DeleteStack arm, which asserts the identical ResourceNotFoundException and message once the stack is gone.

## Steps to Reproduce
1. cloudformation.create_stack with a template declaring:
     Resources:
       Sim2RealStream:
         Type: AWS::Kinesis::Stream
         Properties:
           ShardCount: 1
           Tags:
             - Key: sim2real-behav-check
               Value: cfn-tags-applied
     Outputs: StreamName (!Ref Sim2RealStream) and StreamArn (!GetAtt Sim2RealStream.Arn)
2. Wait for stack_create_complete and read both Outputs — the stack reaches CREATE_COMPLETE and both outputs are populated.
3. kinesis.list_tags_for_resource(ResourceARN=<StreamArn output>) -> HTTP 400 ResourceNotFoundException "Stream <name> under account <account> not found." instead of HTTP 200 with Tags=[{Key: sim2real-behav-check, Value: cfn-tags-applied}].
4. (Blocked by step 3 today) cloudformation.delete_stack, wait for stack_delete_complete, then kinesis.list_tags_for_resource(ResourceARN=<StreamArn>) -> must be ResourceNotFoundException with message "Stream <name> under account <account> not found."
