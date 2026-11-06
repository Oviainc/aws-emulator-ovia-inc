Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.get-resource-policy-without-policy-fails kinesis.resource-policy-put-get-delete kinesis.resource-policy-large-json-round-trip
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|GetResourcePolicy|ResourceNotFoundException
Issue-Commit: 954781cef49baf7406aef764139782df619c6255

# kinesis GetResourcePolicy refuses a stream that has no resource policy with 400 ResourceNotFoundException instead of answering 200 with Policy "{}"

> _Note: this issue was identified against the code base at commit `954781cef49baf7406aef764139782df619c6255`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
REOPENS 20261027T011654Z-kinesis-getresourcepolicy-stub.md: its fix left the call refused.

One root cause (Cause 1 of 1), grounded in three failing behavioral checks of the `kinesis` service: kinesis.get-resource-policy-without-policy-fails, kinesis.resource-policy-put-get-delete, kinesis.resource-policy-large-json-round-trip. The GetResourcePolicy scaffold stub was replaced by a handler, but that handler now treats "this resource has no policy attached" as "not found" and refuses instead of succeeding, so every check that reads the policy of a stream in its no-policy state still dies on the call.

=== Cause 1 of 1: the absence of a resource policy is reported as a not-found refusal instead of an empty policy document ===
Failing checks: kinesis.get-resource-policy-without-policy-fails, kinesis.resource-policy-put-get-delete, kinesis.resource-policy-large-json-round-trip.
AWS: Smithy models GetResourcePolicy input { ResourceARN (required, @length 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$), StreamId } and output { Policy (required) }; its modeled errors are AccessDeniedException, InvalidArgumentException, LimitExceededException, ResourceInUseException and ResourceNotFoundException (all HTTP 400). Recorded behavior: GetResourcePolicy on a stream that EXISTS but has no policy attached is a SUCCESS — HTTP 200 whose required `Policy` member is the empty JSON object document `{}` — and the modeled ResourceNotFoundException is reserved for a ResourceARN that names a resource which does not exist at all (the not-found is about the STREAM, not about the absence of a policy). The same holds after DeleteResourcePolicy: the policy reads back as `{}`, not as a refusal.
Emulator: the call is refused with HTTP 400, error code `ResourceNotFoundException` and the message `Policy for resource <resource-arn> under account <account-id> not found.` — fixed text `Policy for resource `, ` under account ` and ` not found.`, variable parts the ResourceARN from the request and the caller's 12-digit account id. The refusal happens on an ACTIVE stream the same client just created and described, so the stream plainly exists; only its policy does not. kinesis.get-resource-policy-without-policy-fails dies on its first read, and kinesis.resource-policy-put-get-delete and kinesis.resource-policy-large-json-round-trip die when they re-read the policy after DeleteResourcePolicy — none of their real assertions (byte-for-byte unicode/whitespace round-trip of a Condition value, put/get/delete lifecycle, not-found for a genuinely missing stream) are graded at all.
Why: "read the current policy, mutate it, write it back" is the normal way resource policies are managed, and the first read of a never-configured resource is the common case. Against the emulator that read raises ResourceNotFoundException, so such code aborts rather than starting from an empty document; code that distinguishes "resource gone" from "no policy yet" by error code takes the wrong branch, and the post-delete state is unobservable.
Fix: the GetResourcePolicy handler in the kinesis service crate (services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/get_resource_policy.rs) must resolve the ResourceARN against stream/channel state and, when the resource exists, return 200 with Policy = "{}" whenever no policy is stored, raising ResourceNotFoundException only when the ARN resolves to no resource.
Traces: kinesis.get-resource-policy-without-policy-fails, kinesis.resource-policy-put-get-delete, kinesis.resource-policy-large-json-round-trip.

## Evidence
All evidence is from the replayed behavioral suite (simparity-behavioral-aws-v1), 148/284 passed.

Expected: get_resource_policy(ResourceARN=<arn of an existing ACTIVE stream with no policy>) -> HTTP 200 with Policy parsing to {}.
Actual (kinesis.get-resource-policy-without-policy-fails):
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetResourcePolicy operation: Policy for resource arn:aws:kinesis:us-west-2:000000000000:stream/sim2real-kinesis-9cc267aab6b0 under account 000000000000 not found.

Actual (kinesis.resource-policy-put-get-delete), the same refusal on the same operation:
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetResourcePolicy operation: Policy for resource arn:aws:kinesis:us-west-2:000000000000:stream/sim2real-behav-kds-7dcc45cfcf61 under account 000000000000 not found.

Actual (kinesis.resource-policy-large-json-round-trip), again the same:
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetResourcePolicy operation: Policy for resource arn:aws:kinesis:us-west-2:000000000000:stream/sim2real-behav-respolicy-e2e37f84af70 under account 000000000000 not found.

The stream names and the account id above are per-run values; the requirement is the status/code/shape: 200 with Policy "{}" for an existing resource with no policy, and ResourceNotFoundException only for an ARN that resolves to no resource.

## Steps to Reproduce
1. kinesis.create_stream(StreamName=<n>, ShardCount=1) and poll describe_stream_summary until StreamStatus == ACTIVE; take StreamARN from the summary.
2. kinesis.get_resource_policy(ResourceARN=<StreamARN>) -> observe HTTP 400 ResourceNotFoundException "Policy for resource <arn> under account <account> not found." instead of HTTP 200 with Policy == "{}".
3. Contrast: kinesis.get_resource_policy(ResourceARN=<same ARN prefix>/<a stream name that was never created>) -> this one SHOULD be ResourceNotFoundException; the emulator makes both cases indistinguishable.
4. Round-trip form: put_resource_policy(ResourceARN=<StreamARN>, Policy=<a valid policy>), get_resource_policy (succeeds), delete_resource_policy, then get_resource_policy again -> expected 200 with "{}", observed the same 400 ResourceNotFoundException.
