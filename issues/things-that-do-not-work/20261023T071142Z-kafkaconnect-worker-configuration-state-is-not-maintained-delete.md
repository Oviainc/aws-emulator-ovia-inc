Issue-Source: trace-sweep-divergence
Issue-Trace: kafkaconnect-list-filters-and-pagination
Issue-Step: teardown#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafkaconnect-list-filters-and-pagination kafkaconnect-worker-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.kafkaconnect#KafkaConnect
Issue-Key: kafkaconnect|DescribeWorkerConfiguration|4xx->2xx|$
Issue-Key: kafkaconnect|DescribeWorkerConfiguration|4xx->2xx|$.message
Issue-Key: kafkaconnect|DescribeWorkerConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: kafkaconnect|DescribeWorkerConfiguration|4xx->2xx|status
Issue-Key: kafkaconnect|ListTagsForResource|2xx->2xx|$.tags.env
Issue-Key: kafkaconnect|ListTagsForResource|2xx->2xx|$.tags.team
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# kafkaconnect worker-configuration state is not maintained: delete is ignored and tags are never stored

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeWorkerConfiguration

## Description
Three causes, all in the kafkaconnect worker-configuration state store: mutations applied to a worker configuration (deletion, tagging, untagging) are not reflected in later reads.

Cause 1 of 3: after DeleteWorkerConfiguration, DescribeWorkerConfiguration keeps returning the deleted configuration instead of 404 NotFoundException.
AWS: DELETE /v1/worker-configurations/{workerConfigurationArn} is idempotent and returns the arn plus a WorkerConfigurationState; GET /v1/worker-configurations/{workerConfigurationArn} models NotFoundException with httpError 404 (Smithy: DeleteWorkerConfiguration, DescribeWorkerConfiguration errors include NotFoundException=404). The recording shows the configuration converging to deleted, after which Describe answers HTTP 404 with errortype NotFoundException and a `message` member (recorded example: 'Invalid parameter workerConfigurationArn: The requested resource doesn't exist.'). The 404 status and the NotFoundException error code are the requirements; the sentence is an example.
Emulator: Describe keeps answering HTTP 200 with the full configuration (workerConfigurationState=ACTIVE, latestRevision, propertiesFileContent) for the whole 60s poll window — the replay polled 21 times and never saw an error code, so the delete never took effect in state.
Why: any client that deletes a worker configuration and waits for it to disappear (teardown, idempotent re-provisioning, name reuse) hangs or loops forever, and code branching on NotFoundException is never reached. Names also stay permanently occupied.
Fix: DeleteWorkerConfiguration's handler/state — the delete must remove the configuration (or transition it to DELETING and then out of the store) so Describe serializes NotFoundException with HTTP 404.
Traces: kafkaconnect-list-filters-and-pagination#teardown#9 (cluster also covers kafkaconnect-list-filters-and-pagination#teardown#10 and kafkaconnect-worker-configuration-lifecycle#teardown#9).

Cause 2 of 3: ListTagsForResource returns no tags for a worker configuration created with tags.
AWS: CreateWorkerConfiguration accepts a `tags` map (Smithy: CreateWorkerConfiguration.tags -> Tags, map, length 0..200) and GET /v1/tags/{resourceArn} returns them in `tags` (Smithy: ListTagsForResource -> ListTagsForResourceResponse.tags -> Tags). The recording shows the create-time tags echoed back.
Emulator: 200 with the tag keys absent — $.tags.env and $.tags.team both missing.
Why: tag-based discovery, cost allocation and tag-driven authorization all read back empty, so a client cannot confirm the tags it just set and any assertion on them fails.
Fix: CreateWorkerConfiguration's state must persist the input `tags` against the resource ARN, and ListTagsForResource must read them back.
Traces: kafkaconnect-worker-configuration-lifecycle#task#5.

Cause 3 of 3: after tagging/untagging, ListTagsForResource still omits a tag that must remain.
AWS: ListTagsForResource returns exactly the tags currently attached to the ARN (Smithy: ListTagsForResourceResponse.tags); the recording shows the remaining key still present after the untag of the other key.
Emulator: 200 with $.tags.env missing.
Why: untagging appears to clear unrelated keys (and tag writes are not durable at all), so a client's view of the resource's tag set is wrong after any tag mutation.
Fix: same tag state — TagResource/UntagResource must apply per-key against the stored tag map rather than discarding it, and ListTagsForResource must serialize the surviving keys.
Traces: kafkaconnect-worker-configuration-lifecycle#task#7.

## Evidence
Cause 1 — kafkaconnect-list-filters-and-pagination#teardown#9, DescribeWorkerConfiguration: expected status 404, got 200. 'poll: error code None not in ["NotFoundException"] after 21 attempt(s)', poll capped at 60s. Expected body {'message': 'Invalid parameter workerConfigurationArn: The requested resource doesn't exist.'}; got the live configuration {'workerConfigurationState': 'ACTIVE', 'name': 'sim2real-probe-kafkaconnect-wclist-...-a', 'latestRevision': {...}, ...}. $.message missing; $headers.x-amzn-errortype missing. Same cluster: #teardown#10 and kafkaconnect-worker-configuration-lifecycle#teardown#9.

Cause 2 — kafkaconnect-worker-configuration-lifecycle#task#5, ListTagsForResource: status 200 both sides; $.tags.env missing (expected 'probe', got None); $.tags.team missing (expected 'data-eng', got None).

Cause 3 — kafkaconnect-worker-configuration-lifecycle#task#7, ListTagsForResource: status 200 both sides; $.tags.env missing (expected 'probe', got None).

(Tag keys/values, names and ARNs are the recording's examples; the requirement is that the stored tag set is returned.)

## Steps to Reproduce
1. Boot the emulator and replay fixtures kafkaconnect-list-filters-and-pagination and kafkaconnect-worker-configuration-lifecycle.
2. Cause 1: create a worker configuration, DELETE /v1/worker-configurations/{arn}, then poll GET /v1/worker-configurations/{arn} (kafkaconnect-list-filters-and-pagination#teardown#9). Observe 200 with the configuration still ACTIVE for the full poll window instead of 404 NotFoundException.
3. Cause 2: POST /v1/worker-configurations with tags {env, team}, then GET /v1/tags/{arn} (lifecycle#task#5). Observe an empty tags map.
4. Cause 3: after tagging/untagging the configuration, GET /v1/tags/{arn} again (lifecycle#task#7). Observe the key that should remain (env) absent.
