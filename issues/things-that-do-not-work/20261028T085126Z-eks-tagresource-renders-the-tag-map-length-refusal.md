Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.tag-resource-contract-limits-rejected
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.tag-resource-contract-limits-rejected|run#21|AssertionError|assert msg == _validation_message(
Issue-Commit: 64f4c8770e2d08d8b2b967b6209f32bfb55376ee

# eks TagResource renders the tag-map length refusal with the wrong member path, value rendering and constraint sentence (1 cause)

> _Note: this issue was identified against the code base at commit `64f4c8770e2d08d8b2b967b6209f32bfb55376ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Replaying the behavioral suite against the EKS emulator (crate services/service-com-amazonaws-eks-awswesleyfrontend) shows ONE net-new root cause: TagResource correctly refuses a tag key longer than the modelled TagKey maximum with HTTP 400 BadRequestException, but the refusal MESSAGE is built by the generic scalar-member validator instead of the tagging front end's map-constraint renderer, so every part of the message except the leading sentence fragment diverges. This cause must be closed before the issue is complete. (No open issue covers it: 20261022T154613Z-eks-control-plane-operations-answer-501-notimplemented-or.md enumerates ten other control-plane causes and does not mention TagResource or tag-map limits; 20261027T224548Z-eks-2-net-new-root-causes-in-the.md is the add-on lifecycle.)

Cause 1 of 1 — the over-long-tag-key refusal message is rendered in the scalar-member form, not the map-constraint form.
AWS: Smithy (com-amazonaws-eks-awswesleyfrontend): TagResource input `tags` targets TagMap, @length min 1 max 50, key TagKey @length min 1 max 128, value TagValue @length min 0 max 256; modelled errors BadRequestException (httpError 400) and NotFoundException (404). The status and code are therefore modelled; the message TEXT is recorded behavior only, and the recording shows the EKS tagging front end answering 400 BadRequestException whose message string is itself a JSON document with each single quote backslash-escaped:
  {"message": "1 validation error detected: Value \'{<key>=<value>}\' at \'addedTags\' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]"}
In that template the FIXED text is: the `{"message": "..."}` envelope, `1 validation error detected: Value `, the escaped quotes around the rendered value, ` at \'addedTags\' failed to satisfy constraint: `, and the bracketed pair `Map keys must satisfy constraint: [Member must have length less than or equal to <max>, Member must have length greater than or equal to <min>]`. The VARIABLE parts are only: the rendered offending map ENTRY `{key=value}` taken from the request (the key AND its value, not the key alone), and <max>/<min>, which come from the model's TagKey length constraint (128/1). The same renderer carries the value branch, where the constraint clause is `Map value must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 0]` and the rendered entry is `{ok=<value>}`; the check never reached that leg because it aborted on the key leg, so the value leg is unverified by this replay (Hypothesis: it shares the same renderer and diverges identically).
Emulator: for tags={<129-char key>: "v"} on a taggable EKS ARN the emulator answers the right status/code (400 BadRequestException — the check's code assertion passed) but the message is:
  1 validation error detected: Value 'kkkk…k' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 128
Four divergences from AWS: (a) no `{"message": "..."}` JSON envelope and no backslash-escaped quotes — the bare sentence is returned; (b) the member path is `tags` (the input member name) where AWS names `addedTags`; (c) the rendered value is the bare offending key string where AWS renders the whole map entry `{key=value}`; (d) the constraint clause is the scalar-member form `Member must have length less than or equal to 128`, where AWS emits `Map keys must satisfy constraint: [...]` carrying BOTH the max and the min bound.
Why: a client (or a parity/policy test) that keys on the refusal message — parsing the embedded JSON document, reading the `addedTags` member path, or extracting the offending entry from `{key=value}` — gets a differently-shaped string from the emulator: JSON parsing of the message fails outright, the member path points at a member AWS never names, and the min bound is absent. Tests pinned to AWS's refusal text pass on AWS and fail on the emulator (and vice versa), and tooling that surfaces the parsed message to users shows a different diagnosis. The surrounding contract is otherwise honoured by the emulator (50 tags accepted and round-tripped, 51 refused with the exact "The resultant tag set must not have more than 50 user tags." sentence, and nothing partially applied), which makes this purely the map-constraint message renderer.
Fix: the TagResource handler's tag-map validation in the eks crate — the over-long key/value branches must use the tagging front end's map-constraint message renderer (JSON-document envelope with escaped quotes, member path `addedTags`, the `{key=value}` entry rendering, and the bracketed `Map keys must satisfy constraint: [max, min]` / `Map value must satisfy constraint: [max, min]` clauses derived from TagKey/TagValue @length) rather than the shared scalar-member length validator. Validation ordering and the refusal's atomicity are already correct and must be preserved.
Traces: eks.tag-resource-contract-limits-rejected.

## Evidence
Suite simparity-behavioral-ga-prod-v1: 33/110 passed; eks 30/89 (59 failed, all others already filed as unimplemented-stub clusters or owned by open issues).

[mismatch] eks.tag-resource-contract-limits-rejected — check_eks_tag_resource_contract_limits_rejected.py line 191, `assert msg == _validation_message(...)`:
AssertionError: 1 validation error detected: Value 'kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 128

Expected (AWS) for the same request: HTTP 400, code BadRequestException, message
{"message": "1 validation error detected: Value \'{kkk…k=v}\' at \'addedTags\' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]"}
(the 129-char key and its value `v` are the request's values filling the template; the limits 128/1 come from the model's TagKey @length).

Actual: HTTP 400, code BadRequestException (correct), message as quoted above — no JSON envelope/escaped quotes, member path `tags` instead of `addedTags`, bare key instead of the `{key=value}` entry, and the scalar `Member must have length less than or equal to 128` clause instead of `Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]`.

Passing controls in the same check, confirming the rest of the contract works: exactly 50 tags accepted and round-tripped through ListTagsForResource; UntagResource of all 50 keys left an empty map; 51 tags refused with BadRequestException "The resultant tag set must not have more than 50 user tags." and no partial application. The check aborted at the over-long-key leg, so the over-long-VALUE leg (TagValue max 256) and the at-maxima boundary control (128-char key with 256-char value) were never executed.

## Steps to Reproduce
1. Boot the emulator and point an EKS client at it.
2. Obtain a taggable EKS ARN without needing a control plane: eks register_cluster(name=<new>, connectorConfig={'roleArn': <an in-account IAM role trusted by eks-connector.amazonaws.com>, 'provider': 'OTHER'}) and take cluster['arn'].
3. Confirm list_tags_for_resource(resourceArn=arn)['tags'] == {}.
4. tag_resource(resourceArn=arn, tags={'k'*129: 'v'}) — a tag key one character over the modelled TagKey maximum of 128.
5. Expect HTTP 400, code BadRequestException, and message exactly:
   {"message": "1 validation error detected: Value \'{kkk…(129 k)…k=v}\' at \'addedTags\' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]"}
   Observe instead the bare sentence: 1 validation error detected: Value 'kkk…k' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 128
6. Confirm list_tags_for_resource still returns {} (the emulator already does this correctly).
7. Then exercise the leg the check never reached: tag_resource(resourceArn=arn, tags={'ok': 'v'*257}) must be refused with the same envelope but the clause `Map value must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 0]` and the rendered entry `{ok=vvv…v}`; and tag_resource(resourceArn=arn, tags={'k'*128: 'v'*256}) must be ACCEPTED and round-trip.
