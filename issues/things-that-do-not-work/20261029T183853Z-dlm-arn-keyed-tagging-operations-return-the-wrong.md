Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dlm.tag-resource-on-nonexistent-policy-not-found dlm.tag-resource-reserved-aws-prefix-key-rejected
Issue-Shape-Id: com.amazonaws.dlm#dlm_20180112
Issue-Key: dlm.tag-resource-on-nonexistent-policy-not-found|run#8|AssertionError|assert err["Code"] == "NotFoundException", f"code={err['Code']!r}"
Issue-Key: dlm.tag-resource-reserved-aws-prefix-key-rejected|run#6|AssertionError|assert err["Code"] == "BadRequestException", f"code={err['Code']!r}"
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# DLM ARN-keyed tagging operations return the wrong error codes: ResourceNotFoundException instead of NotFoundException for a gone policy ARN, and InvalidRequestException instead of BadRequestException for a reserved aws: tag key

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Two net-new root causes in the dlm crate's ARN-keyed tagging path (TagResource / UntagResource / ListTagsForResource). In both the emulator refuses the call, but with a different error code (and message) than AWS, so a client cannot classify the refusal.

Cause 1 of 2: not-found on an ARN-keyed tagging operation. Failing check: dlm.tag-resource-on-nonexistent-policy-not-found.
AWS: Smithy (services/service-com-amazonaws-dlm-dlm-20180112): TagResource/UntagResource/ListTagsForResource take ResourceArn as an @httpLabel PolicyArn (pattern ^arn:aws(-[a-z]{1,3}){0,2}:dlm:[A-Za-z0-9_/.-]{0,63}:\\d+:policy/[0-9A-Za-z_-]{1,128}$) and model errors InternalServerException(500), InvalidRequestException(400), ResourceNotFoundException(404). The not-found code these ARN-keyed operations actually return is NOT in the model, so: recorded behavior only — AWS answers a well-formed but nonexistent policy ARN with error code 'NotFoundException' and the message 'Resource was not found'. That message is entirely fixed text: no request value, id or timestamp appears in it. The code is empirically distinct from the modeled ResourceNotFoundException used by the PolicyId-keyed operations, i.e. the ARN-keyed lookup path has its own not-found classification.
Emulator: returns code 'ResourceNotFoundException' for all three ARN-keyed operations against a deleted policy's ARN.
Why: a client with error handling keyed on NotFoundException (the code DLM actually emits for ARN-keyed tagging, which is what SDK error-matching and retry/except blocks are written against) does not match the emulator's ResourceNotFoundException, so a benign "the policy is gone" condition escapes as an unhandled error, while code written for the PolicyId-keyed path matches where AWS would not have matched. The distinction between the two codes is observable and load-bearing.
Fix: the resource-lookup/not-found path shared by the TagResource, UntagResource and ListTagsForResource handlers in the dlm service crate (the ARN->policy resolution step, as opposed to the PolicyId-keyed resolution) must raise NotFoundException with the message 'Resource was not found'.
Traces: dlm.tag-resource-on-nonexistent-policy-not-found — the check asserts the same code and message for TagResource, ListTagsForResource and UntagResource on a service-issued ARN of a policy that was created and then deleted; its positive control (tagging and reading tags on a live policy) passes, so only the not-found classification diverges.

Cause 2 of 2: reserved 'aws:' tag-key rejection class. Failing check: dlm.tag-resource-reserved-aws-prefix-key-rejected.
AWS: Smithy: TagResource.Tags targets TagMap, whose key TagKey is @length min 1 max 128 with @pattern ^(?!aws:)[a-zA-Z+-=._:/]+$ — the reserved aws: prefix is a shape-level constraint. The code DLM returns for violating it is not in the operation's modeled error list, so: recorded behavior only — AWS refuses it with error code 'BadRequestException' and the message 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)'. That message is entirely fixed text (no request value interpolated), and the code is empirically distinct from the InvalidRequestException DLM uses for semantic/business-rule failures: a constraint-pattern violation is classified separately from a domain rule violation.
Emulator: returns code 'InvalidRequestException' (the domain-rule code) instead of 'BadRequestException'; the check fails at the code assertion before the message is compared, so the message is unverified and must be set to AWS's text as part of the fix.
Why: a client distinguishing malformed input (BadRequestException — fix the request, never retry) from a business-rule refusal (InvalidRequestException) mis-routes this failure: the reserved-prefix rejection is reported in the same class as, e.g., an un-updatable parameter, so input-validation errors and policy-semantics errors become indistinguishable at the client.
Fix: the TagResource handler's tag-key validation in the dlm service crate (the TagKey pattern/reserved-prefix check) must raise BadRequestException with AWS's message, separately from the handler's InvalidRequestException domain refusals; the same classification applies wherever tag keys are validated on this path.
Traces: dlm.tag-resource-reserved-aws-prefix-key-rejected — the check also confirms the reserved tag is not attached and that a normal tag key on the same policy succeeds; both of those pass, so only the refusal's class diverges.

Neither cause is covered by the open dlm tagging issue 20261029T180848Z-dlm-untagresource-wrong-behavior.md, which is about UntagResource advancing DateModified for keys the policy never held.

## Evidence
dlm.tag-resource-on-nonexistent-policy-not-found (ARN obtained by creating a real policy, reading Policy.PolicyArn, then deleting the policy):
  TagResource(ResourceArn=<gone policy ARN>, Tags={k: v})
  expected: Code='NotFoundException', Message='Resource was not found'
  actual:   Code='ResourceNotFoundException'
  AssertionError: code='ResourceNotFoundException'
  (the check asserts the identical code/message for ListTagsForResource(ResourceArn=<gone ARN>) and UntagResource(ResourceArn=<gone ARN>, TagKeys=['k']) — same shared lookup path)

dlm.tag-resource-reserved-aws-prefix-key-rejected:
  TagResource(ResourceArn=<live policy ARN>, Tags={'aws:reserved': 'nope'})
  expected: Code='BadRequestException', Message='Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)'
  actual:   Code='InvalidRequestException'
  AssertionError: code='InvalidRequestException'

In both cases the call IS refused; the divergence is the error code (and, for Cause 1, the message), which is what SDK error matching keys on.

## Steps to Reproduce
Cause 1:
1. Create an IAM role DLM can assume; create a custom EBS_SNAPSHOT_MANAGEMENT policy with one schedule.
2. Read its ARN via GetLifecyclePolicy(PolicyId=...).Policy.PolicyArn, then DeleteLifecyclePolicy(PolicyId=...).
3. Call TagResource(ResourceArn=<that ARN>, Tags={'k':'v'}); repeat with ListTagsForResource(ResourceArn=<that ARN>) and UntagResource(ResourceArn=<that ARN>, TagKeys=['k']).
4. Observe Code='ResourceNotFoundException' where AWS returns Code='NotFoundException', Message='Resource was not found'.

Cause 2:
1. Create a custom EBS_SNAPSHOT_MANAGEMENT policy and read its PolicyArn.
2. Call TagResource(ResourceArn=<that ARN>, Tags={'aws:reserved': 'nope'}).
3. Observe Code='InvalidRequestException' where AWS returns Code='BadRequestException' with message 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)'.

Replay: run_behavioral_suite and re-run the two check ids in trace_ids.
