Issue-Source: trace-sweep-divergence
Issue-Trace: elementalinference-feed-association-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elementalinference-feed-association-error-paths elementalinference-feed-dictionary-subtitling-reference elementalinference-feed-lifecycle-associate-disassociate elementalinference-list-dictionaries-and-feeds-pagination
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elemental-inference|CreateFeed|2xx->2xx|$.dataEndpoints[*]
Issue-Key: elemental-inference|GetFeed|2xx->2xx|$.dataEndpoints[*]
Issue-Key: elemental-inference|UpdateFeed|2xx->2xx|$.dataEndpoints[*]
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# ElementalInference advertises a foreign AWS data-plane host in dataEndpoints instead of its own endpoint (3 causes)

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFeed

## Description
Every feed-shaped response of this service carries the required `dataEndpoints` list, and in all three operations that return it the emulator serves a host it does not own and cannot answer on. The wire replay flags each one as `host_not_own` on `$.dataEndpoints[0]`. Three causes, one per operation that writes the field onto the wire; all three are fed by the single minting helper named under Fix, so a fix there closes all three, but each must be re-replayed and closed before submitting.

Cause 1 of 3 — CreateFeed (fixture elementalinference-feed-association-error-paths, step task#1, HTTP 202 both sides).
AWS: Smithy: CreateFeed http POST /v1/feed code 202; CreateFeedResponse$dataEndpoints target StringList, **required**, documented as "An array of endpoints for the feed. Typically, there is only one endpoint. The feed receives source media at this endpoint (when the calling application calls PutMedia) and returns the resulting metadata to this endpoint (when the calling application calls GetMetadata)." The model fixes only the member's shape, so the rest is recorded behavior only: the recording returns ONE absolute https URL with a trailing slash, and that URL is a LIVE endpoint of the environment serving the call — the recorded run signs a data-plane request to it and gets a response. The opaque host label and the domain of the recording ('766eefa96a.elemental-inference-data.us-west-2.api.aws') are environment-specific and are NOT the requirement; the requirement is that the advertised host is one the responding service actually answers on, so a client can concatenate a path onto it and reach the feed.
Emulator: returns `https://<10-hex-label>.elemental-inference-data.<region>.api.aws/`, a hard-coded public AWS-domain host that is neither the emulator's own address nor backed by any listener in this build (the replay reports `host_not_own (expected 'https://766eefa96a.elemental-inference-data.us-west-2.api.aws/', got 'https://fc19483bb7.elemental-inference-data.us-west-2.api.aws/')`). The crate's own comment admits it: "no listener backs them — the field reports where the feed would be reached".
Why: a client that follows the endpoint it was just handed — which is the documented way to push media and read metadata — leaves the emulator entirely and sends a signed request to a public AWS domain (or to a name that does not resolve) instead of to the emulator it is talking to. The emulated feature cannot be exercised end to end, and the advertised value is also unusable to any recorded-flow replay, because a host the responder does not own can never be normalized to the responder.
Fix: the data-endpoint minting in this crate's state (the helper that formats the URL, plus the constants for the domain suffix and the host label), which CreateFeed's handler stores on the new feed record; it must mint an endpoint rooted at the address the emulator is actually reached on, and that route must be mounted.
Traces: fixture elementalinference-feed-association-error-paths step task#1; same cluster also elementalinference-feed-dictionary-subtitling-reference#task#3, elementalinference-feed-lifecycle-associate-disassociate#task#1, elementalinference-list-dictionaries-and-feeds-pagination#task#3. Minted at services/service-com-amazonaws-elementalinference-elementalinference/src/state.rs lines 2283-2325 (DATA_ENDPOINT_TLD, DATA_ENDPOINT_LABEL_LEN, feed_data_endpoints, data_endpoint_label) and stored at src/ops/create_feed.rs line 416.

Cause 2 of 3 — GetFeed (fixture elementalinference-feed-association-error-paths, step task#2, HTTP 200 both sides).
AWS: Smithy: GetFeed http GET /v1/feed/{id} code 200; GetFeedResponse$dataEndpoints target StringList, **required**. Recorded behavior only for the value: a read of the feed reports the SAME live endpoint the create advertised, i.e. an endpoint the responding service answers on.
Emulator: serializes the value stored at create time, so the read repeats the foreign AWS-domain host; replay verdict `host_not_own` on `$.dataEndpoints[0]`.
Why: GetFeed is the operation a client polls until the feed is usable and then reads the endpoint from, so this is the path by which the unreachable host actually reaches client code; every consumer of the feed inherits it.
Fix: the GetFeed handler's output is correct in reading the stored record — the stored value is what is wrong, so the fix belongs in the same state-level minting helper (and in a migration of the value for records already stored), not in this serializer.
Traces: fixture elementalinference-feed-association-error-paths step task#2; same cluster also elementalinference-feed-dictionary-subtitling-reference#task#4, elementalinference-feed-lifecycle-associate-disassociate#task#2 / #task#5 / #task#7, elementalinference-list-dictionaries-and-feeds-pagination#task#6.

Cause 3 of 3 — UpdateFeed (fixture elementalinference-feed-dictionary-subtitling-reference, step task#6, HTTP 200 both sides).
AWS: Smithy: UpdateFeed http PUT /v1/feed/{id} code 200; UpdateFeedResponse$dataEndpoints target StringList, **required**. Recorded behavior only: the update echoes the feed's live endpoint unchanged — an update to the output list does not move the feed's data plane.
Emulator: echoes the stored foreign host (replay: `host_not_own (expected 'https://766eefa96a.elemental-inference-data.us-west-2.api.aws/', got 'https://089e2d1c09.elemental-inference-data.us-west-2.api.aws/')`); the differing opaque label is only the per-feed derivation and is not itself the defect — the domain not being the emulator's is.
Why: a client that re-reads the endpoint after reconfiguring outputs gets the same unreachable host, so there is no path through the API that yields a usable endpoint.
Fix: as above — the shared state-level data-endpoint minting, which UpdateFeed's response is built from.
Traces: fixture elementalinference-feed-dictionary-subtitling-reference step task#6.

Related but NOT covering this: open behavioral issue 20261029T183811Z-elementalinference-archived-feed-association-refusal-searchfixtures-7-day.md, whose Cause 3 reports that a sigv4 POST to the advertised dataEndpoint answers 501 NotImplemented. That issue is grounded in behavioral check ids and carries no replay key for these wire steps (the replay reports `owned_by: []` for all three clusters), and it describes the missing data-plane ROUTE rather than the advertised host being foreign. Both must hold: the minted endpoint must name the emulator, and that endpoint must be served.

## Evidence
Replay summary: trace_set simparity-wire-aws-v1, emulator_up true, 54 steps, 43 passed, 11 mismatched, score 79.6%, 3 divergence clusters — all three on `$.dataEndpoints[0]`, all verdict `mismatch`, none with a stub_path or an owned_by.

Cause 1 — CreateFeed, divergence_key `service=elemental-inference|CreateFeed|status=202->202|keys=$.dataEndpoints[0]`, expected_status 202, actual_status 202, detail: `$.dataEndpoints[0]: host_not_own (expected 'https://766eefa96a.elemental-inference-data.us-west-2.api.aws/', got 'https://fc19483bb7.elemental-inference-data.us-west-2.api.aws/')`, affected_count 4.

Cause 2 — GetFeed, divergence_key `service=elemental-inference|GetFeed|status=200->200|keys=$.dataEndpoints[0]`, expected_status 200, actual_status 200, same `host_not_own` detail, affected_count 6.

Cause 3 — UpdateFeed, divergence_key `service=elemental-inference|UpdateFeed|status=200->200|keys=$.dataEndpoints[0]`, expected_status 200, actual_status 200, detail: `$.dataEndpoints[0]: host_not_own (expected 'https://766eefa96a.elemental-inference-data.us-west-2.api.aws/', got 'https://089e2d1c09.elemental-inference-data.us-west-2.api.aws/')`, affected_count 1.

Status codes match on every step; the only diverging member is the required `dataEndpoints` list. What diverges is not the opaque host label or the port (both environment-specific and not a requirement) but the fact that the host announced is not one the emulator answers on: the emulator serves its data listener on its own address while handing clients `*.elemental-inference-data.<region>.api.aws`. Code: services/service-com-amazonaws-elementalinference-elementalinference/src/state.rs:2310-2315 formats `https://{label}.{SERVICE_NAMESPACE}-data.{region}.{DATA_ENDPOINT_TLD}/` with DATA_ENDPOINT_TLD = "api.aws" (line 2291); the doc comment at lines 2306-2309 states outright that no listener backs the value.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1).
2. Cause 1: fixture elementalinference-feed-association-error-paths step task#1 — POST /v1/feed with a name and one output; the 202 body's `dataEndpoints[0]` is `https://<label>.elemental-inference-data.<region>.api.aws/`, a host the emulator does not serve.
3. Cause 2: same fixture step task#2 — GET /v1/feed/{id} for the feed just created; the 200 body repeats that host.
4. Cause 3: fixture elementalinference-feed-dictionary-subtitling-reference step task#6 — PUT /v1/feed/{id} with an updated output list; the 200 body echoes the same foreign host.
5. In each case compare the advertised authority with the address the request was sent to: they differ, so a client that follows `dataEndpoints[0]` is routed away from the emulator. Expected: the URL's authority is the emulator's own (the one the control-plane request reached), with the path shape preserved and the opaque label free to differ per environment.
