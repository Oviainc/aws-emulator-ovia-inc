Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-data-automation-blueprint-tagging
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-data-automation-blueprint-tagging bedrock-data-automation-tagging-error-paths
Issue-Shape-Id: com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService
Issue-Key: bedrock-data-automation|ListTagsForResource|2xx->4xx|$.tags
Issue-Key: bedrock-data-automation|ListTagsForResource|2xx->4xx|status
Issue-Key: bedrock-data-automation|TagResource|2xx->4xx|$
Issue-Key: bedrock-data-automation|TagResource|2xx->4xx|status
Issue-Key: bedrock-data-automation|UntagResource|2xx->4xx|$
Issue-Key: bedrock-data-automation|UntagResource|2xx->4xx|status
Issue-Key: bedrock-data-automation|ListTagsForResource|4xx->4xx|$.message
Issue-Key: bedrock-data-automation|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-data-automation|ListTagsForResource|4xx->4xx|status
Issue-Key: bedrock-data-automation|TagResource|4xx->4xx|$.message
Issue-Key: bedrock-data-automation|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-data-automation|TagResource|4xx->4xx|status
Issue-Key: bedrock-data-automation|UntagResource|4xx->4xx|$.message
Issue-Key: bedrock-data-automation|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-data-automation|UntagResource|4xx->4xx|status
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Tagging operations reject every Bedrock Data Automation resource ARN

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
All three tagging operations validate resourceARN against the Bedrock CONTROL-PLANE resourceARN pattern (custom-model/guardrail/inference-profile/...), which no Bedrock Data Automation ARN can ever match. Every tagging call is therefore refused 400 ValidationException, whether the resource exists or not -- the tagging surface of this service is entirely unreachable, and the modelled ResourceNotFoundException path is unreachable with it. Six net-new causes, one per diverging operation/path.

The shared root fault: the pattern being applied is the one from the neighbouring bedrock control-plane model, not Smithy's TaggableResourceArn for THIS service, whose pattern is ^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-z0-9-]*:[0-9]{12}:(blueprint|data-automation-project|blueprint-optimization-invocation|data-automation-library|data-automation-library-ingestion-job)/[a-zA-Z0-9-]{12,36}$ with @length(20..1011). A blueprint ARN of the form arn:aws:bedrock:<region>:<account>:blueprint/<12-36 id> satisfies it. (No open issue covers this; the similarly named 20261026T030941Z-bedrock-data-automation-runtime-tagresource-validation and 20261029T113856Z-bedrock-data-automation-runtime-listtagsforresource-maintainability are for the separate bedrock-data-automation-runtime service.)

=== Cause 1 of 6: ListTagsForResource refuses a valid blueprint ARN ===
AWS: Smithy: ListTagsForResourceRequest.resourceARN targets TaggableResourceArn (@length 20..1011, pattern above), ListTagsForResource -> ListTagsForResourceResponse.tags, errors ResourceNotFoundException(404). The recorded response for a blueprint ARN that exists and carries tags is 200 with the tag list the resource holds.
Emulator: status 400 with __type ValidationException and the message "1 validation error detected: Value '<resourceARN>' at 'resourceARN' failed to satisfy constraint: Member must satisfy regular expression pattern: (^[a-zA-Z0-9][a-zA-Z0-9\\-]*$)|(^arn:aws(-[^:]+)?:bedrock:...:custom-model/...)|..." -- the bedrock control-plane alternation, which contains no blueprint or data-automation-project alternative at all. $.tags is absent.
Why: a client cannot read the tags of any Bedrock Data Automation resource. Cost allocation, attribute-based access control and any tag-driven inventory pass against this service fails closed with a validation error that names a pattern the SDK's own model does not contain, so the client concludes its request was malformed rather than that the emulator is wrong.
Fix: ListTagsForResource's input validation -- validate resourceARN against this service's TaggableResourceArn, not the bedrock control-plane ARN pattern.
Traces: bedrock-data-automation-blueprint-tagging#task#3 (also #task#5, #task#7).

=== Cause 2 of 6: TagResource refuses a valid blueprint ARN ===
AWS: Smithy: TagResourceRequest.resourceARN -> TaggableResourceArn (same pattern/length), tags -> TagList (@length 0..200), both required; TagResource -> TagResourceResponse (empty body), errors ResourceNotFoundException(404), ServiceQuotaExceededException. The recorded response for tagging an existing blueprint is 200 with an empty JSON object body.
Emulator: status 400 with __type ValidationException and the same control-plane-pattern message, instead of the empty 200 body.
Why: no Bedrock Data Automation resource can be tagged at all, so every tag-on-create-then-tag workflow, every tag-based access policy and every subsequent ListTagsForResource assertion fails.
Fix: TagResource's input validation -- same TaggableResourceArn source as Cause 1.
Traces: bedrock-data-automation-blueprint-tagging#task#4.

=== Cause 3 of 6: UntagResource refuses a valid blueprint ARN ===
AWS: Smithy: UntagResource -> UntagResourceResponse (empty body), resourceARN -> TaggableResourceArn, errors ResourceNotFoundException(404). The recorded response for untagging an existing blueprint is 200 with an empty JSON object body.
Emulator: status 400 with __type ValidationException and the same control-plane-pattern message.
Why: a tag can never be removed, so a client cannot undo a tag it should not have set -- and because Cause 2 means it could never set one, the two together make the tag state of every resource permanently unmanageable.
Fix: UntagResource's input validation -- same TaggableResourceArn source as Cause 1.
Traces: bedrock-data-automation-blueprint-tagging#task#6.

=== Cause 4 of 6: ListTagsForResource on a non-existent resource answers 400 ValidationException, not 404 ResourceNotFoundException ===
AWS: Smithy: ListTagsForResource errors include ResourceNotFoundException with @httpError(404) and a single `message` member; its doc is "This exception is thrown when a resource referenced by the operation does not exist". The recorded refusal is HTTP 404 with x-amzn-errortype naming ResourceNotFoundException and the message "Blueprint <resourceARN> does not exist" -- fixed text "Blueprint " + " does not exist" around the resourceARN taken verbatim from the request.
Emulator: status 400, x-amzn-errortype "ValidationException", and the control-plane-pattern validation message -- so the ARN is rejected as malformed before existence is ever considered.
Why: a client distinguishes "this resource is gone" (404, retire the record) from "my request is malformed" (400, do not retry) by status and error type. Here it gets the latter for the former, so a reconcile loop treats a deleted resource as a client bug and stops instead of cleaning up.
Fix: ListTagsForResource's input validation (accept the service's own ARN form) plus the handler's existence check, which must then raise ResourceNotFoundException with the "Blueprint <arn> does not exist" wording.
Traces: bedrock-data-automation-tagging-error-paths#task#1.

=== Cause 5 of 6: TagResource on a non-existent resource answers 400 ValidationException, not 404 ResourceNotFoundException ===
AWS: as Cause 4, for TagResource (which also models ResourceNotFoundException @httpError(404)). The recorded refusal is 404, x-amzn-errortype ResourceNotFoundException, message "Blueprint <resourceARN> does not exist".
Emulator: status 400, x-amzn-errortype "ValidationException", control-plane-pattern message.
Why: tagging a resource that was concurrently deleted must be reported as a missing resource so the caller can drop it; reported as a malformed request, the caller instead flags its own ARN construction as broken.
Fix: TagResource's input validation plus its existence check and ResourceNotFoundException wording.
Traces: bedrock-data-automation-tagging-error-paths#task#2.

=== Cause 6 of 6: UntagResource on a non-existent resource answers 400 ValidationException, not 404 ResourceNotFoundException ===
AWS: as Cause 4, for UntagResource. The recorded refusal is 404, x-amzn-errortype ResourceNotFoundException, message "Blueprint <resourceARN> does not exist".
Emulator: status 400, x-amzn-errortype "ValidationException", control-plane-pattern message.
Why: same as Cause 5 -- an untag during a teardown race is misreported, so idempotent cleanup code that swallows 404 aborts on the 400 instead.
Fix: UntagResource's input validation plus its existence check and ResourceNotFoundException wording.
Traces: bedrock-data-automation-tagging-error-paths#task#3.

## Evidence
Cause 1 (bedrock-data-automation-blueprint-tagging#task#3, ListTagsForResource): expected 200, got 400; $.tags expected the resource's tag list (in the recording [{'key': 'team', 'value': 'data-eng'}]), got None. 3 steps in the cluster (also #task#5, #task#7).
Cause 2 (bedrock-data-automation-blueprint-tagging#task#4, TagResource): expected 200 with body {}, got 400 with {'__type': 'ValidationException', 'message': "1 validation error detected: Value '<blueprint arn>' at 'resourceARN' failed to satisfy constraint: Member must satisfy regular expression pattern: (^[a-zA-Z0-9][a-zA-Z0-9\\-]*$)|(^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model/(imported)/[a-z0-9]{12}$)|(^arn:aws(-[^:]+)?:bedrock:...(:guardrail/[a-z0-9]+$)|(:automated-reasoning-policy/...)|(:(inference-profile|application-inference-profile)/...)|(:(provisioned-model|model-invocation-job|...)/[a-z0-9]{12}$)))"}.
Cause 3 (bedrock-data-automation-blueprint-tagging#task#6, UntagResource): expected 200 with body {}, got 400 with the same ValidationException body.
Cause 4 (bedrock-data-automation-tagging-error-paths#task#1, ListTagsForResource): expected 404, got 400; $.message expected 'Blueprint <resourceARN> does not exist' got the control-plane-pattern validation text; $headers.x-amzn-errortype expected ResourceNotFoundException (recorded as 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.keystone.build/'), got 'ValidationException'.
Cause 5 (bedrock-data-automation-tagging-error-paths#task#2, TagResource): expected 404, got 400; same message and x-amzn-errortype divergence.
Cause 6 (bedrock-data-automation-tagging-error-paths#task#3, UntagResource): expected 404, got 400; same message and x-amzn-errortype divergence.
Model citations: Smithy TagResourceRequest.resourceARN / ListTagsForResourceRequest.resourceARN -> TaggableResourceArn, @length(min 20, max 1011), pattern ^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-z0-9-]*:[0-9]{12}:(blueprint|data-automation-project|blueprint-optimization-invocation|data-automation-library|data-automation-library-ingestion-job)/[a-zA-Z0-9-]{12,36}$; ResourceNotFoundException errorKind client, @httpError(404), member message -> NonBlankString; TagResource/UntagResource/ListTagsForResource all model ResourceNotFoundException; protocol restJson1. (The specific ARNs, account id, tag key/value and the coral suffix on the recorded errortype header are this run's values, not requirements; the requirements are the 200/404 statuses, the ResourceNotFoundException type and the 'Blueprint <arn> does not exist' wording with the request's resourceARN substituted.)

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator (replay_wire_corpus).
2. Cause 1: fixture bedrock-data-automation-blueprint-tagging, step task#3 -- ListTagsForResource on the blueprint ARN the fixture tagged; expect 200 with the tag list.
3. Cause 2: same fixture, step task#4 -- TagResource on that blueprint ARN; expect 200 with an empty body.
4. Cause 3: same fixture, step task#6 -- UntagResource on that blueprint ARN; expect 200 with an empty body.
5. Cause 4: fixture bedrock-data-automation-tagging-error-paths, step task#1 -- ListTagsForResource on a blueprint ARN that does not exist; expect 404 ResourceNotFoundException with 'Blueprint <arn> does not exist'.
6. Cause 5: same fixture, step task#2 -- TagResource on that ARN; same expectation.
7. Cause 6: same fixture, step task#3 -- UntagResource on that ARN; same expectation.
