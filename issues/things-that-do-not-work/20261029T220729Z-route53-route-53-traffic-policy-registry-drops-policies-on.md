Issue-Source: trace-sweep-divergence
Issue-Trace: route53-traffic-policy-lifecycle-and-list
Issue-Step: teardown#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-traffic-policy-lifecycle-and-list route53-traffic-policy-lifecycle-with-versions route53-traffic-policy-versioning
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53|CreateTrafficPolicyVersion|2xx->4xx|$.CreateTrafficPolicyVersionResponse
Issue-Key: route53|CreateTrafficPolicyVersion|2xx->4xx|status
Issue-Key: route53|DeleteTrafficPolicy|2xx->4xx|$.DeleteTrafficPolicyResponse
Issue-Key: route53|DeleteTrafficPolicy|2xx->4xx|status
Issue-Key: route53|ListTrafficPolicyVersions|2xx->4xx|$
Issue-Key: route53|ListTrafficPolicyVersions|2xx->4xx|$.ListTrafficPolicyVersionsResponse
Issue-Key: route53|ListTrafficPolicyVersions|2xx->4xx|$.ListTrafficPolicyVersionsResponse.TrafficPolicies
Issue-Key: route53|ListTrafficPolicyVersions|2xx->4xx|status
Issue-Key: route53|CreateTrafficPolicy|2xx->4xx|$.CreateTrafficPolicyResponse
Issue-Key: route53|CreateTrafficPolicy|2xx->4xx|$headers.location
Issue-Key: route53|CreateTrafficPolicy|2xx->4xx|status
Issue-Commit: a9b33a473a9762dcd2804e6444a9b0695dc27129

# Route 53 traffic-policy registry drops policies on version delete and refuses duplicate names

> _Note: this issue was identified against the code base at commit `a9b33a473a9762dcd2804e6444a9b0695dc27129`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrafficPolicyVersion

## Description
The emulator's traffic-policy registry models a policy as nothing but its surviving versions, and treats the policy Name as a unique key. Both differ from Route 53: the policy record outlives its versions, and the Name is not a uniqueness constraint on this path. 4 causes.

Cause 1 of 4 — CreateTrafficPolicyVersion on a policy whose earlier versions were deleted.
AWS: Smithy: CreateTrafficPolicyVersion is POST /2013-04-01/trafficpolicy/{Id} with success code 201 and binds NoSuchTrafficPolicy (404) only when the policy Id itself is unknown. Recorded behavior: after a policy's earlier versions have been deleted, the policy Id is still live, and a new version request answers 201 Created with a TrafficPolicy whose Id is the requested Id, whose Version is the next unused version number, and whose Name, Type, Document and Comment come from the request/stored policy.
Emulator: answers 404 with ErrorResponse Error{Type=Sender, Code=NoSuchTrafficPolicy, Message='The policy does not exist.'} and no CreateTrafficPolicyVersionResponse.
Why: a client that prunes old policy versions permanently loses the ability to add new ones, so the normal versioning workflow terminates with a not-found instead of a new version.
Fix: the traffic-policy state — keep the policy record keyed on Id independently of its version set, and remove it only on an explicit policy-level delete.
Traces: route53-traffic-policy-lifecycle-and-list#teardown#8.

Cause 2 of 4 — DeleteTrafficPolicy on that same still-live policy.
AWS: Smithy: DeleteTrafficPolicy is DELETE /2013-04-01/trafficpolicy/{Id}/{Version} with success code 200 and an empty output shape; it binds NoSuchTrafficPolicy (404) only for an unknown Id/Version pair. Recorded behavior: the delete of a live version answers 200 with an empty DeleteTrafficPolicyResponse element.
Emulator: answers 404 because the policy record was already dropped by cause 1's defect, so no DeleteTrafficPolicyResponse is produced.
Why: teardown cannot complete — the resource neither exists for writes nor can be cleaned up, leaving clients unable to reach a known-empty state.
Fix: the traffic-policy state, same registry change as cause 1.
Traces: route53-traffic-policy-lifecycle-and-list#teardown#9.

Cause 3 of 4 — ListTrafficPolicyVersions on a policy with no surviving versions.
AWS: Smithy: ListTrafficPolicyVersions output requires TrafficPolicies, IsTruncated, TrafficPolicyVersionMarker and MaxItems, and binds NoSuchTrafficPolicy (404) for an unknown Id. Recorded behavior: for a known Id whose versions have all been deleted the call answers 200 with an empty TrafficPolicies element, IsTruncated false and the request's MaxItems — it is an empty collection, not a missing resource.
Emulator: answers 404 with Error{Type=Sender, Code=NoSuchTrafficPolicy, Message='The policy does not exist.'} and never converges (the step's poll was capped after 61 attempts).
Why: a poll-until-empty teardown or reconciliation loop never terminates, because the emulator reports the collection as a missing resource rather than as empty.
Fix: ListTrafficPolicyVersions' handler plus the traffic-policy state — distinguish unknown Id from empty version set.
Traces: route53-traffic-policy-lifecycle-with-versions#teardown#8.

Cause 4 of 4 — CreateTrafficPolicy with a Name already used by an existing policy.
AWS: Smithy: CreateTrafficPolicy binds TrafficPolicyAlreadyExists (httpError 409) and documents it as a same-Name conflict, but the recorded response for this request is 201 Created with a TrafficPolicy carrying a newly generated Id distinct from the existing policy's, Version 1, the requested Name, Type, Document and Comment, plus the `Location` response header required by the model. So the duplicate-Name conflict does not fire for this path. Hypothesis: AWS scopes the conflict to a replayed create of the same policy rather than to Name alone.
Emulator: answers 409 with no CreateTrafficPolicyResponse and no `location` header.
Why: clients that legitimately reuse a policy name (per-environment names, blue/green rollouts) are refused a create AWS accepts, and the id of the new policy is never returned.
Fix: CreateTrafficPolicy's validation — stop keying the conflict check on Name alone.
Traces: route53-traffic-policy-versioning#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1:
- route53-traffic-policy-lifecycle-and-list#teardown#8 CreateTrafficPolicyVersion: status expected 201, got 404; $.CreateTrafficPolicyVersionResponse missing (expected TrafficPolicy with the requested Id, the next Version, Name, Type A, Document and Comment; got None).
- route53-traffic-policy-lifecycle-and-list#teardown#9 DeleteTrafficPolicy: status expected 200, got 404; $.DeleteTrafficPolicyResponse missing (expected empty element, got None).
- route53-traffic-policy-lifecycle-with-versions#teardown#8 ListTrafficPolicyVersions: status expected 200, got 404; expected {ListTrafficPolicyVersionsResponse: {TrafficPolicies: '', IsTruncated: 'false', MaxItems: '100'}}, got {ErrorResponse: {Error: {Type: Sender, Code: NoSuchTrafficPolicy, Message: 'The policy does not exist.'}}}; poll capped at 60s after 61 attempts.
- route53-traffic-policy-versioning#task#9 CreateTrafficPolicy: status expected 201, got 409; $.CreateTrafficPolicyResponse missing and $headers.location missing.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus and inspect the four steps named under Traces.
3. Individually: create a traffic policy, create a second version, DELETE /2013-04-01/trafficpolicy/{Id}/1, then POST /2013-04-01/trafficpolicy/{Id} with a new Document — the emulator answers 404 NoSuchTrafficPolicy.
4. Delete every version of a policy, then GET /2013-04-01/trafficpolicies/{Id}/versions — the emulator answers 404 instead of an empty TrafficPolicies list.
5. POST /2013-04-01/trafficpolicy with the Name of an existing policy — the emulator answers 409 instead of 201 with a new Id.
