Issue-Source: trace-sweep-divergence
Issue-Trace: glue-resource-policy-and-tagging
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-resource-policy-and-tagging glue-schema-create-with-tags glue-session-description-and-tags-round-trip glue-workflow-definition-and glue-account-wide-readonly-settings glue-resource-policies-listing glue-resource-policy-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetTags|2xx->4xx|$.Tags
Issue-Key: glue|GetTags|2xx->4xx|status
Issue-Key: glue|TagResource|2xx->4xx|$
Issue-Key: glue|TagResource|2xx->4xx|status
Issue-Key: glue|UntagResource|2xx->4xx|$
Issue-Key: glue|UntagResource|2xx->4xx|status
Issue-Key: glue|GetTags|4xx->4xx|$.Message
Issue-Key: glue|GetTags|4xx->4xx|$.__type
Issue-Key: glue|GetResourcePolicies|2xx->2xx|$.NextToken
Issue-Key: glue|PutResourcePolicy|2xx->2xx|$.PolicyHash
Issue-Key: glue|GetResourcePolicy|2xx->2xx|$.PolicyInJson
Issue-Key: glue|PutResourcePolicy|4xx->4xx|$.Message
Issue-Commit: c912e232849d0cf345219eaaac304bd62b7f7616

# Glue tagging rejects most taggable ARNs and resource-policy responses diverge

> _Note: this issue was identified against the code base at commit `c912e232849d0cf345219eaaac304bd62b7f7616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTags

## Description
This issue bundles 8 net-new causes on the Glue tag and resource-policy surface. Causes 1-4 share one root cause: the emulator's tagging handlers accept only a hard-coded set of Glue resource kinds and refuse every other Glue ARN with InvalidInputException, so tagging a registry, schema, session or workflow fails outright. Causes 5-8 are the resource-policy family's response/refusal divergences. HIGH because causes 1-3 turn successful AWS calls into 400s.

Cause 1 of 8: GetTags refuses ARNs outside a hard-coded resource whitelist.
AWS: Smithy: GetTags input ResourceArn is a GlueResourceArn constrained only by pattern '^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:' (length 1..10240) — the model places NO restriction on which Glue resource kind the ARN names, and real Glue returns the tags of a registry, schema, session or workflow. HTTP 200 with Tags as the map previously set.
Emulator: HTTP 400 with __type InvalidInputException and Message 'ResourceArn must be the ARN of a taggable Glue resource naming one of connection, crawler, database, job; got "<arn>"', and Tags absent.
Why: any client tagging a registry/schema/session/workflow — the documented behavior — breaks, and tag round-trips are impossible for those kinds.
Fix: GetTags validation (drop the resource-kind whitelist; resolve tags per Glue resource kind) and the Glue tag state keyed by ARN.
Traces: glue-resource-policy-and-tagging#task#6 (cluster also covers glue-resource-policy-and-tagging#task#8, glue-schema-create-with-tags#task#3, glue-session-description-and-tags-round-trip#task#5, glue-workflow-definition-and#task#2, #task#4, #task#6).

Cause 2 of 8: TagResource refuses the same ARNs.
AWS: Smithy: TagResource input ResourceArn (GlueResourceArn, same pattern) and required TagsToAdd; tagging a registry succeeds with HTTP 200 and an empty body.
Emulator: HTTP 400 / InvalidInputException with the same whitelist message.
Why: the tags can never be set, so every downstream tag assertion fails.
Fix: TagResource validation/handler (same whitelist).
Traces: glue-resource-policy-and-tagging#task#5 (also glue-workflow-definition-and#task#3).

Cause 3 of 8: UntagResource refuses the same ARNs.
AWS: HTTP 200, empty body.
Emulator: HTTP 400 / InvalidInputException with the same whitelist message.
Why: tags cannot be removed.
Fix: UntagResource validation/handler.
Traces: glue-resource-policy-and-tagging#task#7 (also glue-workflow-definition-and#task#5).

Cause 4 of 8: GetTags for a resource that does not exist reports the wrong error.
AWS: Smithy: GetTags error EntityNotFoundException (client, httpError 400). For an ARN naming a workflow that never existed: __type EntityNotFoundException, Message the fixed text 'Entity not found'.
Emulator: __type InvalidInputException with the whitelist message.
Why: a client distinguishing 'resource gone' from 'bad request' branches wrongly.
Fix: GetTags error classification once the whitelist is removed (resolve the ARN, then refuse missing resources with EntityNotFoundException).
Traces: glue-workflow-definition-and#task#7 (also #task#9).

Cause 5 of 8: GetResourcePolicies omits NextToken.
AWS: Smithy: GetResourcePolicies is @paginated (inputToken NextToken, outputToken NextToken, items GetResourcePoliciesResponseList) and the recorded response carries a NextToken.
Emulator: NextToken absent.
Why: a paginating client (every AWS SDK paginator) stops after the first page and cannot resume; a client asserting the member's presence fails.
Fix: GetResourcePolicies handler's pagination (emit the continuation token).
Traces: glue-account-wide-readonly-settings#task#2 (also glue-resource-policies-listing#task#1, glue-resource-policy-lifecycle#task#4).

Cause 6 of 8: PutResourcePolicy returns a hash that is not AWS's PolicyHash shape.
AWS: Smithy: PutResourcePolicy output PolicyHash is a HashString (length 1..255). Recorded behavior: AWS returns an opaque hash token whose shape the fixture asserts.
Emulator: returns a 64-character lowercase hex SHA-256 digest of the policy document, which fails the recorded shape assertion.
Why: a client that stores the hash and replays it as PolicyHashCondition against real Glue (or validates its form) is working with a token AWS would not mint.
Fix: PutResourcePolicy handler's policy-hash generation.
Traces: glue-resource-policy-and-tagging#task#9.

Cause 7 of 8: GetResourcePolicy returns a re-serialized policy document.
AWS: PolicyInJson is returned in Glue's own pretty-printed form — the policy is re-emitted with newlines and ' : ' between key and value, not the compact form a client submitted and not a byte-for-byte echo.
Emulator: returns a compact single-line JSON rendering.
Why: a client that diffs the stored policy against what Glue reports (or asserts on the document it reads back) sees a spurious difference on every read.
Fix: GetResourcePolicy serializer (render PolicyInJson the way Glue does).
Traces: glue-resource-policy-and-tagging#task#10 (also glue-resource-policy-lifecycle#task#3).

Cause 8 of 8: PutResourcePolicy with a non-JSON policy returns an emulator-authored message.
AWS: Smithy: PutResourcePolicy error InvalidInputException (client, 400); PolicyInJson is a required PolicyJsonString with length min 2. For a body that is not JSON, AWS surfaces its parser's own text: fixed prefix 'Invalid input policy: Unexpected IOException: Unrecognized token ' then the offending token in quotes, then ": was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')" and a ' at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: <line>, column: <column>]' clause — the token and the line/column come from the submitted document.
Emulator: Message 'PolicyInJson must be a policy document in JSON format.'
Why: message assertions fail, and the client loses the position of the parse error AWS reports.
Fix: PutResourcePolicy validation message (surface the parse failure with token and position).
Traces: glue-resource-policy-and-tagging#task#11.

## Evidence
Replay of simparity-wire-aws-v1:
1. GetTags: status expected 200, got 400; $.Tags expected {'sim2real-key1': 'value1', 'sim2real-key2': 'value2'}, got None (7 affected steps).
2. TagResource: status expected 200, got 400; expected {}, got {'__type': 'InvalidInputException', 'Message': 'ResourceArn must be the ARN of a taggable Glue resource naming one of connection, crawler, database, job; got "arn:aws:glue:us-west-2:000000000000:registry/..."'}.
3. UntagResource: status expected 200, got 400; same InvalidInputException body.
4. GetTags 400/400: $.__type expected 'EntityNotFoundException' got 'InvalidInputException'; $.Message expected 'Entity not found' got the whitelist message for a workflow ARN.
5. GetResourcePolicies 200/200: $.NextToken missing (expected a token, got None).
6. PutResourcePolicy 200/200: $.PolicyHash expected '<shaped-not-asserted>' (shape assertion) got 'b2bf8cdac24fdbd2b313aea08d348101651059247d66ff1343f5fd54e34d70f8'.
7. GetResourcePolicy 200/200: $.PolicyInJson expected '{\n  "Version" : "2012-10-17",\n  "Statement" : [ { ... } ]\n}' got '{"Version": "2012-10-17", "Statement": [{...}]}'.
8. PutResourcePolicy 400/400: $.Message expected "Invalid input policy: Unexpected IOException: Unrecognized token 'not': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1]" got 'PolicyInJson must be a policy document in JSON format.'
(ARNs, hashes and the offending token above are that run's values.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Re-replay: glue-resource-policy-and-tagging#task#6, #task#5, #task#7, #task#9, #task#10, #task#11, glue-workflow-definition-and#task#7, glue-account-wide-readonly-settings#task#2.
3. The tagging steps TagResource/GetTags/UntagResource against a Glue registry (and GetTags against an absent workflow); the policy steps Put/Get a catalog resource policy, list policies, and Put a non-JSON policy.
