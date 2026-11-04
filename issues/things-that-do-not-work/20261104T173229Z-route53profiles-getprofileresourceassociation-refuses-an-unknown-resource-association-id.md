Issue-Source: trace-sweep-divergence
Issue-Trace: route53profiles-profile-error-paths
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53profiles-profile-error-paths
Issue-Shape-Id: com.amazonaws.route53profiles#Route53Profiles
Issue-Key: route53profiles|GetProfileResourceAssociation|4xx->4xx|$.Message
Issue-Key: route53profiles|GetProfileResourceAssociation|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 806a42b2cb4f422c835271649b6ece8770a3e771

# route53profiles GetProfileResourceAssociation refuses an unknown resource-association id with the wrong error shape and message

> _Note: this issue was identified against the code base at commit `806a42b2cb4f422c835271649b6ece8770a3e771`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProfileResourceAssociation

## Description
ONE net-new wire cause in the route53profiles Profile-resource-association plane, found by replaying the wire corpus (trace set simparity-wire-aws-v1). It is NOT covered by the open issue '20261029T194643Z-route53profiles-profile-status-reporting-not-found-refusal-wording.md' (route53profiles Profile status reporting / not-found refusal wording / CreateProfile Name validation): that issue's Causes 3, 5 and 6 are the shared [RSLVR-05007] not-found template for Profiles and Profile ASSOCIATIONS, and its fix makes every not-found refusal emit [RSLVR-05007] as ResourceNotFoundException. This operation is the opposite requirement — AWS does not treat an unresolvable ProfileResourceAssociationId as a not-found at all — so applying that issue's fix leaves this step diverging (and can even entrench it). The replay reports no owner for this cluster.

Cause 1 of 1: GetProfileResourceAssociation with a ProfileResourceAssociationId that resolves to no stored association is refused as ResourceNotFoundException with the generic [RSLVR-05007] not-found text, where AWS refuses it as an INVALID-PARAMETER class error with the id-specific [RSLVR-05200] template.

AWS: GetProfileResourceAssociation is GET /profileresourceassociation/{ProfileResourceAssociationId}, success 200, and models AccessDeniedException, InvalidParameterException, ResourceNotFoundException, ThrottlingException and ValidationException, all errorKind client at httpError 400 (defaulted) (Smithy: operation GetProfileResourceAssociation; input member ProfileResourceAssociationId -> ResourceId, @httpLabel, required, @length 1..64; shape InvalidParameterException {Message required, FieldName}, doc 'One or more parameters in this request are not valid.'; shape ResourceNotFoundException {Message, ResourceType}, doc 'The resource you are associating is not found.'). The recording shows AWS classifying an id that names no Profile resource association as a bad PARAMETER rather than a missing resource: HTTP 400 with an x-amzn-errortype naming an error shape other than ResourceNotFoundException, and a body Message built from this Resolver-family template — fixed text '[RSLVR-05200] The Profile resource association ID in your request isn't valid: "<id>". Check the ID and try again.' The fixed parts are the literal sub-error code [RSLVR-05200], the sentence 'The Profile resource association ID in your request isn't valid:' and the trailing sentence 'Check the ID and try again.'; the single variable part is <id>, the ProfileResourceAssociationId taken verbatim from the request path. The 400 status and the modeled error set are from the model; the exact template text and the choice of error shape are recorded behavior only. Hypothesis: this service validates the id's resource-prefix/format before any lookup, so a syntactically unacceptable or unresolvable 'rpra-' id never reaches the store and is reported as an invalid parameter.

Emulator: answers HTTP 400 (status is right) but with the WRONG error shape and the WRONG message. The handler looks the id up in the store and, on a miss, returns ResourceNotFoundException built by the shared not-found helper, so the replay reports $headers.x-amzn-errortype: shape_mismatch (got 'ResourceNotFoundException', expected a different shape) and $.Message: value_mismatch — got '[RSLVR-05007] Can't find the resource with ID "<id>".' where AWS returns the [RSLVR-05200] invalid-id template. Note the emulator already emits [RSLVR-05007] here while emitting its own prose on the Profile/association paths, which is why this cluster is a distinct cause from the other issue's message-template causes.

Why: a client cannot distinguish a malformed/unacceptable association id from a resource that is genuinely absent. Any SDK consumer that branches on the error shape (catch InvalidParameterException to re-validate input vs catch ResourceNotFoundException to treat the resource as deleted) takes the wrong branch, and retry/idempotency logic that treats not-found as 'already gone' will silently swallow a bad-input bug. Code that matches the [RSLVR-05200] marker or reads FieldName never matches.

Fix: GetProfileResourceAssociation's handler and the error it selects — an id that cannot be resolved must be refused with the invalid-parameter error shape AWS uses, carrying the [RSLVR-05200] template with the id filled from the request, instead of routing the store miss through the shared not-found helper. The message template belongs with the service's other Resolver-family message builders in the state module.

Traces: route53profiles-profile-error-paths#task#5 (GetProfileResourceAssociation on an id that was never minted). Code: services/service-com-amazonaws-route53profiles-route53profiles/src/ops/get_profile_resource_association.rs lines 164-173 (store miss mapped to ResourceNotFoundException via crate::state::profile_resource_association_not_found).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, /__admin/health 200): 17 steps total, 5 passed, 12 mismatched, 0 errored, 0 blocked, score 29.4%, 7 divergence clusters — all route53profiles. Six clusters are owned by the open issue 20261029T194643Z-route53profiles-profile-status-reporting-not-found-refusal-wording.md; this is the one cluster with no owner and no stub.

Cause 1 — fixture route53profiles-profile-error-paths step task#5, GetProfileResourceAssociation, expected 400 / actual 400:
  $.Message: value_mismatch — expected '[RSLVR-05200] The Profile resource association ID in your request isn't valid: "rpra-doesnotexist0". Check the ID and try again.', got '[RSLVR-05007] Can't find the resource with ID "rpra-doesnotexist0".'
  $headers.x-amzn-errortype: shape_mismatch — expected an error shape other than ResourceNotFoundException, got 'ResourceNotFoundException'.

The value 'rpra-doesnotexist0' is that recording's requested id; the requirement is the template with the id taken verbatim from the request path, not this literal. The status (400) is correct; the error shape and the message text are the divergence.

## Steps to Reproduce
Boot the emulator and point a route53profiles client at it.

1. Call GetProfileResourceAssociation(ProfileResourceAssociationId='rpra-doesnotexist0') — any well-formed 'rpra-' id that was never minted (1..64 chars, so it passes the modeled @length constraint).
2. Expect HTTP 400 whose x-amzn-errortype names the invalid-parameter error shape (NOT ResourceNotFoundException), with body Message '[RSLVR-05200] The Profile resource association ID in your request isn't valid: "rpra-doesnotexist0". Check the ID and try again.' — the id echoed verbatim from the request.
3. The emulator instead answers 400 with x-amzn-errortype: ResourceNotFoundException and Message '[RSLVR-05007] Can't find the resource with ID "rpra-doesnotexist0".'.

Positive control that must keep working: after AssociateResourceToProfile, GetProfileResourceAssociation on the returned id must still answer 200 with the ProfileResourceAssociation member.

Then re-replay fixture route53profiles-profile-error-paths in full and confirm step task#5 passes.
