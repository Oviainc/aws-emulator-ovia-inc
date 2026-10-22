Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-guardrail-full-policy-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-guardrail-full-policy-lifecycle bedrock-delete-resource-policy-missing-not-notfound bedrock-guardrail-lifecycle-and-versioning
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|GetGuardrail|2xx->2xx|$.contentPolicy.tier
Issue-Key: bedrock|GetGuardrail|2xx->2xx|$.createdAt
Issue-Key: bedrock|GetGuardrail|2xx->2xx|$.failureRecommendations
Issue-Key: bedrock|GetGuardrail|2xx->2xx|$.status
Issue-Key: bedrock|GetGuardrail|2xx->2xx|$.statusReasons
Issue-Key: bedrock|GetGuardrail|2xx->2xx|$.topicPolicy.tier
Issue-Key: bedrock|GetGuardrail|2xx->2xx|$.topicPolicy.topics[*].examples
Issue-Key: bedrock|GetGuardrail|2xx->2xx|$.updatedAt
Issue-Key: bedrock|GetGuardrail|2xx->2xx|$.wordPolicy.managedWordLists
Issue-Key: bedrock|CreateGuardrail|2xx->2xx|$.createdAt
Issue-Key: bedrock|ListGuardrails|2xx->2xx|$.guardrails[arn="arn
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Bedrock guardrail reads return epoch timestamps, omit empty collections and never reach READY

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGuardrail

## Description
Six causes in the guardrail response path, sharing three faults: timestamps are serialized as epoch numbers instead of date-time strings, empty collections and policy tier blocks are omitted instead of emitted, and a created/updated guardrail never settles to READY.

Fault A - timestamp serialization. AWS: Smithy GetGuardrail.createdAt and .updatedAt target Timestamp with timestampFormat date-time (both @required); CreateGuardrail.createdAt and the ListGuardrails summary carry the same Timestamp. So the wire value is an ISO-8601 date-time string. Emulator: a bare epoch float (e.g. 1792691069.7873337). Why: an AWS SDK deserializing a restJson1 date-time member gets a number where a string is modeled - either a parse failure or a nonsense instant - so any client that reads guardrail creation time breaks, and the fixture's shape assertion fails. Fix: the guardrail serializer's timestamp emission (it must honour the member's timestampFormat).

Fault B - omitted collections and tier blocks. AWS: GetGuardrail returns statusReasons and failureRecommendations (modeled lists), wordPolicy.managedWordLists (modeled list), topicPolicy.topics[].examples, and a tier block on contentPolicy and topicPolicy ({'tierName': 'CLASSIC'}); the recordings show these present as [] / the CLASSIC tier even when nothing was configured. Emulator: the members are absent from the body. Why: a client that iterates statusReasons or reads contentPolicy.tier.tierName to decide which guardrail tier it is on hits a null and either crashes or mis-reports the tier. Fix: GetGuardrail's response construction/serializer plus the guardrail state defaults (tier defaulting to CLASSIC when the create request omitted it).

Fault C - lifecycle status never converges. AWS: GuardrailStatus enum CREATING/UPDATING/VERSIONING/READY/FAILED/DELETING; the recordings show the guardrail already READY on the read that follows CreateGuardrail (202) and on the read that follows UpdateGuardrail. Emulator: still CREATING after create and UPDATING after update, permanently. Why: any client (or test) that polls for READY before versioning or invoking the guardrail loops forever; the whole guardrail lifecycle is unusable. Fix: the guardrail state machine - create/update must transition the stored guardrail to READY (the emulator has no asynchronous work to wait on).

Cause 1 of 6: GetGuardrail after CreateGuardrail - all three faults at once (status CREATING not READY; contentPolicy.tier, topicPolicy.tier, topicPolicy.topics[0].examples, statusReasons, failureRecommendations missing; createdAt/updatedAt epoch).
Traces: bedrock-guardrail-full-policy-lifecycle#task#2.

Cause 2 of 6: GetGuardrail in the resource-policy fixture - statusReasons, failureRecommendations and wordPolicy.managedWordLists missing; createdAt/updatedAt epoch (status matched here).
Traces: bedrock-delete-resource-policy-missing-not-notfound#task#5.

Cause 3 of 6: CreateGuardrail (202) - createdAt emitted as an epoch number. Note the open issue 'Bedrock CreateGuardrail refused 500 AmbiguousService instead of the modeled 202 guardrail envelope' covers the earlier routing refusal; the operation now answers 202 and this is the remaining serialization defect, which that issue does not carry.
Traces: bedrock-guardrail-full-policy-lifecycle#task#1.

Cause 4 of 6: GetGuardrail after create in the versioning fixture - status CREATING not READY, tier/examples/statusReasons/failureRecommendations missing, createdAt epoch.
Traces: bedrock-guardrail-lifecycle-and-versioning#task#2.

Cause 5 of 6: GetGuardrail after UpdateGuardrail - status UPDATING not READY, contentPolicy.tier/statusReasons/failureRecommendations missing, createdAt epoch.
Traces: bedrock-guardrail-lifecycle-and-versioning#task#4.

Cause 6 of 6: ListGuardrails - the summary's createdAt is an epoch number instead of a date-time string.
Traces: bedrock-guardrail-lifecycle-and-versioning#task#6.

For the record, not filed: GetGuardrail/DeleteGuardrail on a deleted guardrail (bedrock-guardrail-full-policy-lifecycle#task#4, #task#5) match on status 404 and differ only in the not-found message wording, whose rule is not established.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline.

Cause 1 (bedrock-guardrail-full-policy-lifecycle#task#2, GetGuardrail, 200 expected / 200 actual): $.status expected 'READY', got 'CREATING'; $.contentPolicy.tier expected {'tierName': 'CLASSIC'}, got None; $.topicPolicy.tier expected {'tierName': 'CLASSIC'}, got None; $.topicPolicy.topics[0].examples expected [], got None; $.statusReasons expected [], got None; $.failureRecommendations expected [], got None; $.createdAt and $.updatedAt expected a date-time-shaped value, got 1792691069.7873337.

Cause 2 (bedrock-delete-resource-policy-missing-not-notfound#task#5, GetGuardrail, 200/200): $.statusReasons [] -> None, $.failureRecommendations [] -> None, $.wordPolicy.managedWordLists [] -> None, $.createdAt/$.updatedAt -> 1792691061.0131795.

Cause 3 (bedrock-guardrail-full-policy-lifecycle#task#1, CreateGuardrail, 202/202): $.createdAt -> 1792691069.7873337 where a date-time-shaped value is required.

Cause 4 (bedrock-guardrail-lifecycle-and-versioning#task#2, GetGuardrail, 200/200): $.status 'READY' -> 'CREATING', plus the same missing tier/examples/statusReasons/failureRecommendations and $.createdAt -> 1792691070.442888.

Cause 5 (bedrock-guardrail-lifecycle-and-versioning#task#4, GetGuardrail, 200/200): $.status 'READY' -> 'UPDATING', $.contentPolicy.tier -> None, $.statusReasons/$.failureRecommendations -> None, $.createdAt -> epoch.

Cause 6 (bedrock-guardrail-lifecycle-and-versioning#task#6, ListGuardrails, 200/200): $.guardrails[arn="arn:aws:bedrock:us-west-2:000000000000:guardrail/0000017bomkc"].createdAt -> 1792691070.442888.

Model evidence (smithy_query, services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): GetGuardrailResponse.createdAt / .updatedAt = Timestamp, timestampFormat 'date-time', @required; statusReasons (GuardrailStatusReasons) and failureRecommendations (GuardrailFailureRecommendations) are lists; GuardrailWordPolicy.managedWordLists is a list; GuardrailContentPolicy.tier and GuardrailTopicPolicy.tier exist; GuardrailStatus values = [CREATING, UPDATING, VERSIONING, READY, FAILED, DELETING]. The specific instants, guardrail ids and ARNs above are environment-specific examples, not requirements.

## Steps to Reproduce
1. Boot the emulator built from mainline.
2. POST /guardrails with a content policy, a topic policy and a word policy -> 202; inspect $.createdAt: it must be an ISO-8601 date-time string, not an epoch number.
3. GET /guardrails/{guardrailId} -> 200 and check: $.status must be READY (not CREATING), $.statusReasons and $.failureRecommendations must be present as [], $.wordPolicy.managedWordLists must be present as [], $.topicPolicy.topics[0].examples must be present as [], $.contentPolicy.tier and $.topicPolicy.tier must be present as {"tierName":"CLASSIC"}, and $.createdAt/$.updatedAt must be date-time strings.
4. PUT /guardrails/{guardrailId} (UpdateGuardrail), then GET again -> $.status must be READY, not UPDATING.
5. GET /guardrails -> each summary's createdAt must be a date-time string.
6. Or replay the corpus and inspect fixtures bedrock-guardrail-full-policy-lifecycle, bedrock-guardrail-lifecycle-and-versioning and bedrock-delete-resource-policy-missing-not-notfound.
