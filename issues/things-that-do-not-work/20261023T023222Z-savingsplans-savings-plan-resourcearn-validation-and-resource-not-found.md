Issue-Source: trace-sweep-divergence
Issue-Trace: savingsplans-tagging-error-paths-unknown-resource
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: savingsplans-tagging-error-paths-unknown-resource
Issue-Shape-Id: com.amazonaws.savingsplans#AWSSavingsPlan
Issue-Key: savingsplans|ListTagsForResource|4xx->4xx|$.message
Issue-Key: savingsplans|TagResource|4xx->4xx|$.message
Issue-Key: savingsplans|UntagResource|4xx->4xx|$.message
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# Savings Plan resourceArn validation and resource-not-found errors diverge across tagging operations

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
All three tagging operations share two error-path defects: the resourceArn constraint violation is reported against the wrong member path and with a narrower, partition-blind pattern, and the resource-not-found error does not use AWS's request-derived message template. Statuses and error codes match throughout; the diverging parts are the constraint pattern (which also changes which ARNs are accepted) and two message templates whose rule is known, because both are built from the request.

Cause 1 of 5: ListTagsForResource with a malformed resourceArn — wrong member path and narrower pattern in the ValidationException.
  AWS: Smithy: ListTagsForResource.resourceArn is @required, targets SavingsPlanArn, and the checked-in model gives it @pattern ^arn:aws:[a-z]+:([a-z]{2}-[a-z]+-\d{1}|):(\d{12}):savingsplan\/([0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$; ValidationException is httpError 400. The message is AWS's standard constraint-violation template, built from the request — '<n> validation error detected: Value \'<value>\' at \'<memberPath>\' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>' — so the member path and the quoted pattern are the requirement, not the prose. Recorded behavior: the path is 'originalRequest.resourceArn' and the quoted pattern is partition-tolerant: ^arn:aws([-a-z]*)?:[a-z]+:([a-z]{2}-[a-z]+-\d{1}|):(\d{12}):savingsplan\/(...)$.
  Emulator: 400 with the path 'resourceArn' (no 'originalRequest.' prefix) and the checked-in model's narrower pattern ^arn:aws:[a-z]+:..., which is also what it enforces.
  Why: the quoted pattern is the contract the caller is told to satisfy, and the emulator's is strictly narrower than the live one — the 'aws([-a-z]*)?' partition group is what lets arn:aws-cn:... and arn:aws-us-gov:... through. Hypothesis: a caller using a China or GovCloud plan ARN is refused at the boundary validator with 400 where AWS would accept the call; the emulator's own state notes this deliberately (savings_plan_id_from_arn, state.rs:386-391, matches only the 'aws' partition because the model pattern starts ^arn:aws:). A caller that parses the error to locate the offending member also looks up the wrong path.
  Fix: the SavingsPlanArn boundary validator / the model pattern it is generated from, and the validation-error renderer's member-path prefix.

Cause 2 of 5: ListTagsForResource with a well-formed but unknown plan ARN — wrong not-found message template.
  AWS: Smithy: ListTagsForResource declares ResourceNotFoundException (client, httpError 404), 'The specified resource was not found.' The message is built from the request and names the ARN the caller sent: 'Resource with resourceArn: <arn> doesn\'t exist'.
  Emulator: 404 ResourceNotFoundException with 'no Savings Plan found with ARN \'<arn>\''.
  Why: the status and code are right, so the break is for callers that match on the message template AWS documents through its recordings; they classify the failure differently or log a message no AWS log would contain.
  Fix: the not-found error construction shared by the tagging handlers.

Cause 3 of 5: TagResource with a malformed resourceArn — identical to cause 1 (path 'resourceArn' instead of 'originalRequest.resourceArn', narrow pattern). Smithy: TagResource.resourceArn @required, same SavingsPlanArn pattern; ValidationException 400.

Cause 4 of 5: TagResource with an unknown plan ARN — identical to cause 2 (404 ResourceNotFoundException with the emulator's own wording instead of the request-derived 'Resource with resourceArn: <arn> doesn\'t exist').

Cause 5 of 5: UntagResource with a malformed resourceArn — identical to cause 1. Smithy: UntagResource.resourceArn @required, same SavingsPlanArn pattern; ValidationException 400.

Traces: savingsplans-tagging-error-paths-unknown-resource#task#1 (cause 1), #task#2 (cause 2), #task#3 (cause 3), #task#4 (cause 4), #task#5 (cause 5).

## Evidence
All five from fixture savingsplans-tagging-error-paths-unknown-resource; every status matched, only the message content diverged.

task#1 ListTagsForResource, 400 vs 400:
  expected "1 validation error detected: Value 'not-an-arn' at 'originalRequest.resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws([-a-z]*)?:[a-z]+:([a-z]{2}-[a-z]+-\d{1}|):(\d{12}):savingsplan\/([0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$"
  got      "1 validation error detected: Value 'not-an-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws:[a-z]+:([a-z]{2}-[a-z]+-\d{1}|):(\d{12}):savingsplan\/([0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$"
  -> two concrete deltas: member path missing the 'originalRequest.' prefix, and the partition group 'aws([-a-z]*)?' replaced by the literal 'aws'.
task#3 TagResource, 400 vs 400: same two deltas, same values.
task#5 UntagResource, 400 vs 400: same two deltas, same values.

task#2 ListTagsForResource, 404 vs 404:
  expected "Resource with resourceArn: arn:aws:savingsplans::000000000000:savingsplan/11111111-1111-1111-8111-111111111111 doesn't exist"
  got      "no Savings Plan found with ARN 'arn:aws:savingsplans::000000000000:savingsplan/11111111-1111-1111-8111-111111111111'"
task#4 TagResource, 404 vs 404: same expected/actual pair.

Supporting source: services/service-com-amazonaws-savingsplans-awssavingsplan/src/state.rs:386-391 documents that the boundary validator refuses aws-cn and aws-us-gov ARNs because the model pattern starts ^arn:aws:.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Causes 1/3/5: fixture savingsplans-tagging-error-paths-unknown-resource, steps task#1 (POST /ListTagsForResource), task#3 (POST /TagResource), task#5 (POST /UntagResource), each with resourceArn 'not-an-arn'; compare the 400 ValidationException message's member path and quoted pattern with the recording.
3. Causes 2/4: same fixture, steps task#2 (POST /ListTagsForResource) and task#4 (POST /TagResource) with the well-formed, unknown ARN arn:aws:savingsplans::000000000000:savingsplan/11111111-1111-1111-8111-111111111111; compare the 404 ResourceNotFoundException message template.
4. Partition check for cause 1: call POST /ListTagsForResource with an otherwise valid arn:aws-cn:savingsplans:... ARN — the emulator refuses it at the boundary validator, while the pattern AWS quotes in the recording admits it.
