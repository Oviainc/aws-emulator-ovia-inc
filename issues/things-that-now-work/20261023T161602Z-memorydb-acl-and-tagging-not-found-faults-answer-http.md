Issue-Source: trace-sweep-divergence
Issue-Trace: memorydb-acl-lifecycle-and-state-machine
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: memorydb-acl-lifecycle-and-state-machine memorydb-default-user-and-open-access-guards memorydb-arn-validation-by-resource-type memorydb-resource-tagging-and-arn-validation
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|DescribeACLs|4xx->4xx|$.message
Issue-Key: memorydb|DescribeACLs|4xx->4xx|status
Issue-Key: memorydb|DescribeACLs|4xx->4xx|$
Issue-Key: memorydb|UntagResource|4xx->4xx|status
Issue-Key: memorydb|ListTags|4xx->4xx|$.message
Issue-Key: memorydb|ListTags|4xx->4xx|status
Issue-Key: memorydb|TagResource|4xx->4xx|$.message
Issue-Key: memorydb|TagResource|4xx->4xx|status
Issue-Key: memorydb|UntagResource|4xx->4xx|$.message
Issue-Commit: 205916506edbd7602d3640584e73a4ccd0a8f366

# ACL and tagging not-found faults answer HTTP 404 instead of 400

> _Note: this issue was identified against the code base at commit `205916506edbd7602d3640584e73a4ccd0a8f366`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeACLs

## Description
Six causes, all one shared fault: on the ACL and resource-tagging operations the emulator maps the MemoryDB *NotFoundFault family to HTTP 404, while the recorded AWS endpoint answers HTTP 400 with the same __type. The MemoryDB Smithy model does declare @httpError 404 on these fault shapes (ACLNotFoundFault @httpError 404, UserNotFoundFault @httpError 404, both with an inert awsQueryError), but the recording shows the live awsJson1_1 endpoint answering 400 — the recorded wire status is the requirement, so the per-fault status mapping must not be taken from @httpError for this family. All six causes must be closed before submitting.

Cause 1 of 6: DescribeACLs on a missing ACL.
AWS: recorded behavior — HTTP 400, __type 'ACLNotFoundFault' (Smithy: ACLNotFoundFault, errorKind client, doc 'The specified ACL does not exist.', @httpError 404 not reflected on the wire).
Emulator: HTTP 404 with __type ACLNotFoundFault. Message also differs only by a trailing period ('ACL <name> not found' vs 'ACL <name> not found.'); that text difference alone is an example, the status is the defect.
Why: a client that branches on the HTTP status (404 treated as a routing/endpoint failure, 400 as a client fault) misclassifies a missing-ACL error, and SDK retry/error-shape classification keyed on status diverges.
Fix: the memorydb error serializer's fault-to-HTTP-status mapping for the NotFound family.
Traces: memorydb-acl-lifecycle-and-state-machine#task#7.

Cause 2 of 6: DescribeACLs polled during ACL teardown. Same fault surfaced in a poll loop: AWS answers 400 ACLNotFoundFault; the emulator answers 404, so the fixture's poll for 400 never satisfies and caps at 60s after 13 attempts. Why: a client polling for deletion-completion by waiting for the documented 400 fault never terminates. Fix: same status mapping. Traces: memorydb-acl-lifecycle-and-state-machine#teardown#12.

Cause 3 of 6: UntagResource against an ARN whose resource does not exist. AWS: HTTP 400 (Smithy UntagResource errors include ACLNotFoundFault/UserNotFoundFault/ClusterNotFoundFault, all recorded at 400). Emulator: HTTP 404. Fix: same mapping, reached through the UntagResource handler. Traces: memorydb-arn-validation-by-resource-type#task#5.

Cause 4 of 6: ListTags against an ARN naming a non-existent user. AWS: HTTP 400 with the not-found fault and a message of the form '<name> is either not present or not available'. Emulator: HTTP 404 (message 'User <name> not found.' — example only). Fix: same mapping, reached through ListTags. Traces: memorydb-arn-validation-by-resource-type#task#10.

Cause 5 of 6: TagResource against an ARN naming a non-existent user. AWS: HTTP 400. Emulator: HTTP 404. Fix: same mapping, reached through TagResource. Traces: memorydb-arn-validation-by-resource-type#task#20.

Cause 6 of 6: UntagResource against an ARN naming a non-existent ACL. AWS: HTTP 400. Emulator: HTTP 404. Fix: same mapping. Traces: memorydb-arn-validation-by-resource-type#task#21.

## Evidence
Cause 1: expected status 400, got 404 (fixture memorydb-acl-lifecycle-and-state-machine, step task#7, DescribeACLs). Cause 2: expected 400, got 404; poll capped at 60s, 'status 404 not in [400] after 13 attempt(s)' (teardown#12). Cause 3: expected 400, got 404 (memorydb-arn-validation-by-resource-type task#5, UntagResource). Cause 4: expected 400, got 404 (task#10, ListTags). Cause 5: expected 400, got 404 (task#20, TagResource). Cause 6: expected 400, got 404 (task#21, UntagResource). Smithy citation: ACLNotFoundFault/UserNotFoundFault @httpError 404 with inert awsQueryError, contradicted by the recorded 400 on the awsJson1_1 wire.

## Steps to Reproduce
Replay the wire corpus (trace set simparity-wire-aws-v1) and compare: (1) DescribeACLs with ACLName naming an ACL that does not exist — expect HTTP 400 __type ACLNotFoundFault, emulator returns 404; (2) the same call in a delete-completion poll loop; (3) UntagResource/TagResource/ListTags with a well-formed MemoryDB ARN whose resource (user or ACL) does not exist — expect HTTP 400 with the corresponding NotFound __type, emulator returns 404. Fixtures/steps: memorydb-acl-lifecycle-and-state-machine#task#7, memorydb-acl-lifecycle-and-state-machine#teardown#12, memorydb-arn-validation-by-resource-type#task#5, #task#10, #task#20, #task#21.
