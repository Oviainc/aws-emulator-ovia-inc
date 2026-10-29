Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-resource-policy-and-tagging-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-resource-policy-and-tagging-errors
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|DescribeResourcePolicy|4xx->4xx|status
Issue-Key: comprehend|PutResourcePolicy|4xx->4xx|$.Message
Issue-Key: comprehend|PutResourcePolicy|4xx->4xx|$.__type
Issue-Key: comprehend|PutResourcePolicy|4xx->4xx|status
Issue-Key: comprehend|DeleteResourcePolicy|4xx->4xx|status
Issue-Key: comprehend|ListTagsForResource|4xx->4xx|status
Issue-Key: comprehend|TagResource|4xx->4xx|status
Issue-Key: comprehend|UntagResource|4xx->4xx|status
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# Comprehend resource-policy and tagging refusals answer 404 and resolve the resource before validating the policy

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
Six net-new root causes in the resource-policy and tagging plane. Five are the same status defect as the model/endpoint plane (see the companion issue on DescribeEndpoint/DescribeDocumentClassifier not-found status) but in operations that issue does not name and whose steps it does not replay; the sixth is an ordering defect in PutResourcePolicy that changes the error CLASS, not only its status.

Cause 1 of 6: DescribeResourcePolicy for a model ARN with no policy / no such model.
  AWS: Smithy: DescribeResourcePolicy errors InvalidRequestException(400) / ResourceNotFoundException; recorded behavior shows the live service answering HTTP 400 for this refusal.
  Emulator: HTTP 404.
  Why: a client branching on the 4xx status treats a modeled client error as an endpoint/routing fault and does not surface the refusal.
  Fix: the DescribeResourcePolicy handler's not-found path and the shared ResourceNotFoundException status mapping.
  Traces: comprehend-resource-policy-and-tagging-errors#task#1.

Cause 2 of 6: PutResourcePolicy with a policy document naming an action Comprehend does not support, against a model ARN that does not exist. AWS validates the POLICY DOCUMENT before resolving the resource, so the request is refused as an invalid request, not as a missing resource.
  AWS: Smithy: PutResourcePolicy input ResourcePolicy @required (Policy, length 1..20000) and ResourceArn @required; errors InvalidRequestException(400) and ResourceNotFoundException. Recorded behavior: HTTP 400, __type 'InvalidRequestException', Message 'POLICY_INVALID_INPUT: The provided policy uses unsupported action.' — the reason prefix 'POLICY_INVALID_INPUT: ' and the sentence are fixed text; nothing from the request is interpolated.
  Emulator: HTTP 404, __type 'ResourceNotFoundException', Message 'RESOURCE_NOT_FOUND: Could not find specified resource.' — it resolves the ARN first and never reaches the policy check, so a malformed policy against a real model would also go unreported.
  Why: a client that fixes the ARN and retries hits the same rejection with no hint that its policy document is the problem; the error class it catches (InvalidRequestException) is never thrown.
  Fix: the PutResourcePolicy handler — run policy-document validation ahead of resource resolution — plus its InvalidRequestException wording.
  Traces: comprehend-resource-policy-and-tagging-errors#task#2.

Cause 3 of 6: DeleteResourcePolicy for a model ARN that does not exist.
  AWS: Smithy: DeleteResourcePolicy errors InvalidRequestException / ResourceNotFoundException; recorded behavior only — HTTP 400.
  Emulator: HTTP 404.
  Why: as cause 1.
  Fix: the DeleteResourcePolicy handler's not-found path.
  Traces: comprehend-resource-policy-and-tagging-errors#task#3.

Cause 4 of 6: ListTagsForResource for an ARN that resolves to nothing.
  AWS: Smithy: ListTagsForResource errors InvalidRequestException(400) / ResourceNotFoundException / TooManyRequestsException; recorded behavior only — HTTP 400.
  Emulator: HTTP 404.
  Why: as cause 1, on the read side of tagging.
  Fix: the ListTagsForResource handler's not-found path.
  Traces: comprehend-resource-policy-and-tagging-errors#task#4.

Cause 5 of 6: TagResource against an ARN that resolves to nothing.
  AWS: Smithy: TagResource input ResourceArn @required (ComprehendArn), Tags @required; errors ResourceNotFoundException / InvalidRequestException(400) / ConcurrentModificationException / TooManyTagsException; recorded behavior only — HTTP 400.
  Emulator: HTTP 404.
  Why: as cause 1, on the write side of tagging.
  Fix: the TagResource handler's not-found path.
  Traces: comprehend-resource-policy-and-tagging-errors#task#5.

Cause 6 of 6: UntagResource against an ARN that resolves to nothing.
  AWS: Smithy: UntagResource errors ResourceNotFoundException / InvalidRequestException(400) / ConcurrentModificationException; recorded behavior only — HTTP 400.
  Emulator: HTTP 404.
  Why: as cause 1.
  Fix: the UntagResource handler's not-found path.
  Traces: comprehend-resource-policy-and-tagging-errors#task#6.

## Evidence
Cause 1: DescribeResourcePolicy — status: expected 400, got 404.
Cause 2: PutResourcePolicy — status: expected 400, got 404; $.__type expected 'InvalidRequestException', got 'ResourceNotFoundException'; $.Message expected 'POLICY_INVALID_INPUT: The provided policy uses unsupported action.', got 'RESOURCE_NOT_FOUND: Could not find specified resource.'
Cause 3: DeleteResourcePolicy — status: expected 400, got 404.
Cause 4: ListTagsForResource — status: expected 400, got 404.
Cause 5: TagResource — status: expected 400, got 404.
Cause 6: UntagResource — status: expected 400, got 404.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Fixture comprehend-resource-policy-and-tagging-errors: step task#1 (DescribeResourcePolicy), task#2 (PutResourcePolicy with an unsupported-action policy document against a nonexistent model ARN), task#3 (DeleteResourcePolicy), task#4 (ListTagsForResource), task#5 (TagResource), task#6 (UntagResource).
3. Each answers HTTP 404 where the recording has 400; task#2 additionally answers ResourceNotFoundException where the recording has InvalidRequestException with the POLICY_INVALID_INPUT message.
