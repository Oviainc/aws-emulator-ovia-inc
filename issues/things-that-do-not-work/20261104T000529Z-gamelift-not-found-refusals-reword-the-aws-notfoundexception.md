Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-multi-compute-isolation
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-multi-compute-isolation gamelift-location-lifecycle-and-tags
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|DescribeCompute|4xx->4xx|$.Message
Issue-Key: gamelift|ListTagsForResource|4xx->4xx|$.Message
Issue-Commit: e7422c982dfb77c1099e81b60a45c65279a5e5d9

# GameLift not-found refusals reword the AWS NotFoundException message on DescribeCompute and ListTagsForResource

> _Note: this issue was identified against the code base at commit `e7422c982dfb77c1099e81b60a45c65279a5e5d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCompute

## Description
Two causes. Both operations classify the refusal correctly (HTTP 400, NotFoundException) but emit the emulator's own prose instead of the message AWS words for that error class, so the status and error code are right and only the message text diverges.

Cause 1 of 2: DescribeCompute for a compute that does not exist / was deregistered (fixture gamelift-multi-compute-isolation, step task#11; cluster also affects task#14).
AWS: Smithy: DescribeCompute errors include NotFoundException (client, httpError 400). Recorded refusal message is `Host (<ComputeName>) not found.` — fixed text `Host (`, `) not found.` with the single variable part being the ComputeName value echoed from the request; note the trailing period and that AWS says "Host", not "Compute".
Emulator: answers 400 with Message `Compute (<ComputeName>) not found` — wrong noun and no trailing period.
Why: clients and tests that match the AWS message to distinguish a deregistered host from other 400 NotFoundException causes (bad fleet, wrong location) fail to match, and human-facing surfaces show wording AWS never emits.
Fix: the DescribeCompute handler's not-found refusal path (message template for NotFoundException).

Cause 2 of 2: ListTagsForResource against a deleted/unknown GameLift resource ARN (fixture gamelift-location-lifecycle-and-tags, step task#11).
AWS: Smithy: ListTagsForResource errors include NotFoundException (client, httpError 400); input ResourceARN -> AmazonResourceName @length 1..1011. Recorded refusal message is `Resource with ARN <ResourceARN> not found` — fixed text `Resource with ARN `, ` not found`, with the variable part being the ARN echoed verbatim from the request.
Emulator: answers 400 with Message `Failed to find resource: <ResourceARN>` — a different template that leads with the emulator's internal phrasing.
Why: tag-management clients that assert on the AWS message (or surface it) see unrecognized text for the normal "resource is gone" case; the ARN is echoed in both, so only the surrounding wording is wrong.
Fix: the ListTagsForResource handler's resource-resolution failure path (message template for NotFoundException); the same template is likely shared with the other tagging operations and should be corrected there.

The recorded compute names and ARNs are per-run values that fill these templates; the requirement is the fixed text plus the request-derived value, not the literal recorded ids. Two open issues cover message rewording on OTHER GameLift operations — 20261103T083127Z-gamelift-refusal-fidelity-3-error-class-message-divergences.md (UpdateGameSessionQueue, DeleteMatchmakingRuleSet, UpdateMatchmakingConfiguration) and 20261103T145425Z-gamelift-wire-fidelity-listlocations-serves-one-entry-catalog.md (ListLocations, DeleteMatchmakingRuleSet) — neither covers DescribeCompute or ListTagsForResource or these steps.

## Evidence
Replay (trace set simparity-wire-aws-v1), both clusters HTTP 400 -> 400 with the error class matched; only $.Message diverges:
- DescribeCompute, gamelift-multi-compute-isolation#task#11 (also #task#14): $.Message value_mismatch — expected 'Host (sim2real-probe-gamelift-compute-a-e8a177ae) not found.', got 'Compute (sim2real-probe-gamelift-compute-a-e8a177ae) not found'.
- ListTagsForResource, gamelift-location-lifecycle-and-tags#task#11: $.Message value_mismatch — expected 'Resource with ARN arn:aws:gamelift:us-west-2:000000000000:location/custom-sim2real-probe-gamelift-loc-cb79c9db not found', got 'Failed to find resource: arn:aws:gamelift:us-west-2:000000000000:location/custom-sim2real-probe-gamelift-loc-cb79c9db'.
Model evidence: NotFoundException (client, httpError 400) is modeled on both DescribeCompute and ListTagsForResource.

## Steps to Reproduce
1. Register two computes in an Anywhere fleet, then DeregisterCompute one of them (as fixture gamelift-multi-compute-isolation does).
2. Call DescribeCompute for the deregistered ComputeName -> expect 400 NotFoundException with Message 'Host (<ComputeName>) not found.'; the emulator returns 'Compute (<ComputeName>) not found'.
3. Create a custom location, tag it, delete it, then call ListTagsForResource with its ARN (as fixture gamelift-location-lifecycle-and-tags does) -> expect 400 NotFoundException with Message 'Resource with ARN <ARN> not found'; the emulator returns 'Failed to find resource: <ARN>'.
4. Replay fixtures gamelift-multi-compute-isolation (task#11, task#14) and gamelift-location-lifecycle-and-tags (task#11).
