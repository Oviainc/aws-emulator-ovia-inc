Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-resolver-endpoint-missing-resource-errors
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-resolver-endpoint-missing-resource-errors
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|GetResolverEndpoint|4xx->4xx|$.Message
Issue-Key: route53resolver|ListResolverEndpointIpAddresses|4xx->4xx|$.Message
Issue-Key: route53resolver|UpdateResolverEndpoint|4xx->4xx|$.Message
Issue-Key: route53resolver|DeleteResolverEndpoint|4xx->4xx|$.Message
Issue-Commit: eb649b8e6cc19057378cc7d140b29e6e37a84f5c

# Resolver endpoint not-found error omits RSLVR code prefix in Message

> _Note: this issue was identified against the code base at commit `eb649b8e6cc19057378cc7d140b29e6e37a84f5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResolverEndpoint

## Description
One shared formatting fault in route53resolver's ResourceNotFoundException message, reached through four different resolver-endpoint operations. Each operation is a separate replayed cause and all four must be closed; they almost certainly share a single not-found message formatter.

Common shape of all four causes:
AWS: every resolver-endpoint read/mutate operation models ResourceNotFoundException (Smithy: GetResolverEndpoint / ListResolverEndpointIpAddresses / UpdateResolverEndpoint / DeleteResolverEndpoint all list ResourceNotFoundException, errorKind client, httpError 400; ResourceNotFoundException has members Message and ResourceType, doc "The specified resource doesn't exist."). The model does not state the message text, so this is recorded behavior: in all four recordings the Message is built from the request's ResolverEndpointId to a single fixed shape, '[RSLVR-00501] Resolver endpoint with ID "<id>" does not exist.' - a bracketed RSLVR-00501 service error code, the id in double quotes, and a terminating period. The rule is thus the RSLVR-coded template plus the request's endpoint id; the particular id is an example.
Emulator: returns the right status (400) and the right error code, but formats the Message as Resolver endpoint with ID '<id>' does not exist - no [RSLVR-00501] prefix, single quotes around the id, and no trailing period (replay detail on each step: "$.Message: value_mismatch (expected '[RSLVR-00501] Resolver endpoint with ID \"rslvr-out-0123456789abcdef0\" does not exist.', got \"Resolver endpoint with ID 'rslvr-out-0123456789abcdef0' does not exist\")"). Status and __type agree; only Message diverges.
Why: Route 53 Resolver surfaces its RSLVR-nnnnn codes in the exception message, and operators and tooling key on them to distinguish which resolver fault occurred (the HTTP status and ResourceNotFoundException alone do not identify it). Log scrapers, runbook automation and assertions that match the RSLVR-00501 code find nothing against the emulator, and message snapshots recorded on AWS do not match.
Fix: the shared not-found error construction for the resolver-endpoint operations (their handlers' error/serialization path for ResourceNotFoundException) must emit the RSLVR-coded template - the [RSLVR-00501] prefix, the endpoint id double-quoted, terminating period - rather than the emulator's own ad-hoc wording, so every operation that raises it is consistent.

Cause 1 of 4: GetResolverEndpoint. Trace route53resolver-resolver-endpoint-missing-resource-errors#task#3.
Cause 2 of 4: ListResolverEndpointIpAddresses. Trace route53resolver-resolver-endpoint-missing-resource-errors#task#4.
Cause 3 of 4: UpdateResolverEndpoint. Trace route53resolver-resolver-endpoint-missing-resource-errors#task#7.
Cause 4 of 4: DeleteResolverEndpoint. Trace route53resolver-resolver-endpoint-missing-resource-errors#task#8.
Traces: the four fixture#step bindings above, all in fixture route53resolver-resolver-endpoint-missing-resource-errors.

## Evidence
All four steps: HTTP 400 and the error code match; only $.Message diverges.
Expected (recorded, each step): '[RSLVR-00501] Resolver endpoint with ID "rslvr-out-0123456789abcdef0" does not exist.'
Actual (each step): "Resolver endpoint with ID 'rslvr-out-0123456789abcdef0' does not exist"
Replay divergence keys:
- service=route53resolver|GetResolverEndpoint|status=400->400|keys=$.Message (step task#3)
- service=route53resolver|ListResolverEndpointIpAddresses|status=400->400|keys=$.Message (step task#4)
- service=route53resolver|UpdateResolverEndpoint|status=400->400|keys=$.Message (step task#7)
- service=route53resolver|DeleteResolverEndpoint|status=400->400|keys=$.Message (step task#8)
The identical deviation across four unrelated operations is what identifies a single shared formatter, and the identical recorded template across all four is what shows the [RSLVR-00501] prefix is produced by a rule, not incidental text.
Model citation: Smithy ResourceNotFoundException (client, httpError 400, members Message/ResourceType) is listed by all four operations; the model states no message text, so the template is taken from the recordings.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay wire fixture route53resolver-resolver-endpoint-missing-resource-errors from trace set simparity-wire-aws-v1.
3. Steps task#3 (GetResolverEndpoint), task#4 (ListResolverEndpointIpAddresses), task#7 (UpdateResolverEndpoint) and task#8 (DeleteResolverEndpoint) each reference a resolver endpoint id that does not exist.
4. Each returns HTTP 400 with the correct error code but a Message lacking the [RSLVR-00501] prefix, using single quotes around the id and no trailing period.
5. replay_wire_corpus reports all four as $.Message value_mismatch clusters.
