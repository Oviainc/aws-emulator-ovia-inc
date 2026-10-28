Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.tag-resource-contract-limits-rejected
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.tag-resource-contract-limits-rejected|run#15|AssertionError|assert code == "BadRequestException", (
Issue-Commit: 293416f0e23279c4c36b6e93c4ddddc54811a5e3

# EKS TagResource refuses an over-limit tag map with the wrong error class and message: unmodeled InvalidParameterException + generic constraint text instead of BadRequestException / "The resultant tag set must not have more than 50 user tags." (1 cause)

> _Note: this issue was identified against the code base at commit `293416f0e23279c4c36b6e93c4ddddc54811a5e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: the TagResource tag-contract-limit refusal is reported in the wrong error vocabulary.

AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json) models TagResource with input `tags` -> TagMap, `@length min 1 max 50`, key TagKey `@length 1..128`, value TagValue `@length 0..256`, and declares EXACTLY TWO errors: BadRequestException (HTTP 400, "the request contains a semantic error") and NotFoundException (HTTP 404). InvalidParameterException is NOT in TagResource's error set at all. Recorded behavior for a TagMap one entry over the modelled maximum: HTTP 400, error code `BadRequestException`, message `The resultant tag set must not have more than 50 user tags.` In that message the only variable part is the limit (50, the TagMap `@length` max); the rest is fixed text, and notably it echoes NO request-derived value -- not the submitted count, not the resourceArn. The refusal is also total: ListTagsForResource is unchanged afterwards, and the boundary value (exactly 50 tags) is accepted.

Emulator: the 51-entry map never reaches the handler. The shared shape-constraint layer refuses it first, answering error code `InvalidParameterException` with the generic message `1 validation error detected: Value '51 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50`. That code is not a modelled error of this operation, and the message is the generic constraint-report form rather than EKS's tagging sentence. Independently, the handler's OWN accumulated-tag ceiling (services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/tag_resource.rs lines 176-198) would also diverge if it were reached: it builds `Tagging <resourceArn> would give it <N> tags, more than the limit of 50 tags per resource`, which echoes the ARN and a request-derived count that AWS's sentence does not contain. Same root cause -- the identity of the over-limit refusal (class + wording) is wrong on both paths.

Hypothesis: the same generic constraint layer, ordered ahead of the handler, will mis-class the other two modelled tag limits the check goes on to exercise (a 129-character TagKey and a 257-character TagValue, both of which AWS refuses as BadRequestException whose message is a JSON document embedded in the message string, `{"message": "1 validation error detected: Value \'{<key>=<value>}\' at \'addedTags\' failed to satisfy constraint: Map keys must satisfy constraint: [...]"}`). The check aborts at the first limit, so those two are NOT independently observed here and are reported as part of this one cause, not as separate ones.

Why: a client catching the operation's modelled errors cannot catch InvalidParameterException for TagResource -- generated SDK models have no such variant on this operation, so a tag-ceiling refusal surfaces as an unmodelled/generic error and tag-trimming retry logic keyed on BadRequestException never fires. Code that matches on AWS's message text ("The resultant tag set must not have more than 50 user tags.") also never matches.

Fix: the TagResource validation path for this operation -- the generic TagMap `@length` constraint rejection must be answered in this operation's own error channel (BadRequestException, 400) with EKS's tagging sentence, and the handler's accumulated-tag ceiling message must be restated in the same fixed wording instead of echoing the ARN and the submitted count.

Traces: eks.tag-resource-contract-limits-rejected.

## Evidence
Failing check eks.tag-resource-contract-limits-rejected [mismatch]:

  AssertionError: TagResource with 51 tags (TagMap max is 50) -> InvalidParameterException: 1 validation error detected: Value '51 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50; expected BadRequestException

Expected (real AWS, us-west-2): code `BadRequestException` (HTTP 400), message exactly `The resultant tag set must not have more than 50 user tags.` -- fixed text with the limit (50) as its only variable part.
Actual (emulator): code `InvalidParameterException`, message `1 validation error detected: Value '51 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50` -- generic constraint-report form, echoing a request-derived entry count AWS does not report.

Model: TagResource errors = [BadRequestException (400), NotFoundException (404)]; TagMap @length {min 1, max 50}; TagKey @length {1,128}; TagValue @length {0,256}.
Handler: services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/tag_resource.rs:176-198 holds a second, differently-worded ceiling message that is unreachable behind the constraint layer.

The check's controls passed up to this point: exactly 50 tags round-trips and untags cleanly, so the boundary-accept side is correct; only the boundary+1 refusal diverges.

## Steps to Reproduce
1. RegisterCluster a connected cluster (no control plane, no EC2 needed) and take `cluster.arn`.
2. Confirm ListTagsForResource(resourceArn=arn) returns an empty tag map.
3. Call TagResource(resourceArn=arn, tags={50 distinct keys}) -- accepted today, correct. UntagResource all 50 keys.
4. Call TagResource(resourceArn=arn, tags={51 distinct keys}).
   Expected: HTTP 400 BadRequestException, message `The resultant tag set must not have more than 50 user tags.`, and ListTagsForResource still empty.
   Actual: InvalidParameterException with message `1 validation error detected: Value '51 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50`.
5. Replay: eks.tag-resource-contract-limits-rejected (after the fix the same check continues on to the 129-char TagKey and 257-char TagValue limits, which must likewise answer BadRequestException).
