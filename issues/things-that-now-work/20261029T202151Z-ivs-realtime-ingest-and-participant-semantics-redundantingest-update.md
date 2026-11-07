Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-realtime-update-ingest-configuration-redundant-ingest-toggle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-realtime-update-ingest-configuration-redundant-ingest-toggle ivs-realtime-create-participant-token-empty-capabilities-list-honored ivs-realtime-disconnect-participant-unknown-id-is-noop-success
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime|UpdateIngestConfiguration|2xx->2xx|$.ingestConfiguration.redundantIngest
Issue-Key: ivs-realtime|UpdateIngestConfiguration|2xx->2xx|$.ingestConfiguration.redundantIngestCredentials
Issue-Key: ivs-realtime|UpdateIngestConfiguration|2xx->2xx|$.ingestConfiguration.userId
Issue-Key: ivs-realtime|UpdateIngestConfiguration|2xx->2xx|$headers.access-control-allow-origin
Issue-Key: ivs-realtime|UpdateIngestConfiguration|2xx->2xx|$headers.access-control-expose-headers
Issue-Key: ivs-realtime|UpdateIngestConfiguration|2xx->2xx|$headers.cache-control
Issue-Key: ivs-realtime|UpdateIngestConfiguration|2xx->2xx|$headers.content-security-policy
Issue-Key: ivs-realtime|GetIngestConfiguration|2xx->2xx|$.ingestConfiguration.redundantIngest
Issue-Key: ivs-realtime|GetIngestConfiguration|2xx->2xx|$.ingestConfiguration.redundantIngestCredentials
Issue-Key: ivs-realtime|CreateParticipantToken|2xx->2xx|$.participantToken.token
Issue-Key: ivs-realtime|DisconnectParticipant|2xx->4xx|$
Issue-Key: ivs-realtime|DisconnectParticipant|2xx->4xx|status
Issue-Related: 20261029T202151Z-ivs-realtime-response-projection-drops-documented-empty-string.md
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# ivs-realtime ingest and participant semantics: redundantIngest update ignored, unknown-participant disconnect refused, token not JWT-shaped (3 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateIngestConfiguration

## Description
Three independent behavioral divergences on the ivs-realtime ingest/participant plane, found by replaying the wire corpus (6 divergence clusters). They share one fault family: the handler's decision about state, not the shape of the envelope. A fixer must close all three before submitting.

Cause 1 of 3 — an UpdateIngestConfiguration that enables redundant ingest is not persisted, and the redundant credentials are never produced.
AWS: Smithy: UpdateIngestConfiguration input `redundantIngest` targets RedundantIngest, type boolean, @default false; output IngestConfiguration carries redundantIngest and redundantIngestCredentials (list of RedundantIngestCredential{participantId, streamKey}), documented as "present only when redundantIngest is set to true". So an update with redundantIngest=true returns 200 with redundantIngest=true and a redundantIngestCredentials list holding one credential per redundant ingest endpoint (each with its own participantId and an rt_-prefixed streamKey), and every later GetIngestConfiguration reports the same.
Emulator: the update answers 200 but reports $.ingestConfiguration.redundantIngest = false and omits $.ingestConfiguration.redundantIngestCredentials entirely; the following GetIngestConfiguration reports the same false/absent pair, and a second (redundant) update to the same value does not converge either — the credentials list stays absent through the rest of the fixture.
Why: the input parameter is silently ignored, so a client that enables redundant ingest gets a success it cannot act on: it never receives the second stream key it must broadcast to, and a read-back check of redundantIngest tells it the feature is off while the call reported success. Any redundant-ingest publishing flow is unimplementable against the emulator.
Fix: the UpdateIngestConfiguration handler and the ingest-configuration state: apply the requested redundantIngest to the stored configuration and, while it is true, materialize the redundantIngestCredentials list (one credential per endpoint, each with a participantId and a stream key matching the modeled ^rt_[0-9]+_[a-z0-9-]+_[a-zA-Z0-9-]+_.+$ shape); drop the list again when it is set back to false.
Traces: ivs-realtime-update-ingest-configuration-redundant-ingest-toggle#task#3 (UpdateIngestConfiguration), #task#4 (GetIngestConfiguration), #task#5 (UpdateIngestConfiguration), #task#6 (GetIngestConfiguration). Those same four steps additionally show the IngestConfiguration$userId-absent and CORS-header defects filed separately in the ivs-realtime response-projection bundle of this sweep.

Cause 2 of 3 — DisconnectParticipant refuses an unknown participantId instead of succeeding as a no-op.
AWS: Smithy: DisconnectParticipant (POST /DisconnectParticipant) has an EMPTY output structure and models ResourceNotFoundException (404) only for the stage; documentation: "Disconnects a specified participant from a specified stage." Recorded behavior: called with a well-formed participantId (matching @pattern ^[a-zA-Z0-9-_]*$, @length max 64) that names no participant in an existing stage, AWS answers 200 with an empty JSON body {} — the disconnect is idempotent/no-op for a participant that is not connected.
Emulator: answers 404 with __type/Code "ResourceNotFoundException" and message "Resource: <participantId> not found" (the fixed text being "Resource: " ... " not found" and the variable part the participantId copied from the request).
Why: disconnect is the operation clients call defensively (on timeout, on retry, on cleanup). AWS lets the retry succeed; the emulator turns the second, already-effective disconnect — and any cleanup of a participant that has already dropped — into a hard client error, so moderation/cleanup paths that treat 404 as fatal break, and retry logic diverges.
Fix: the DisconnectParticipant handler's lookup branch: once the stage resolves, an unmatched participantId must fall through to the empty 200 response instead of raising ResourceNotFoundException. Keep 404 for a stageArn that names no stage. Note this is NOT the already-resolved replica case (20261028T203643Z-ivs-realtime-disconnect-participant-error-response, which covered a participant the stage reports CONNECTED); here no such participant exists at all.
Traces: ivs-realtime-disconnect-participant-unknown-id-is-noop-success#task#2.

Cause 3 of 3 — the minted participant token is not shaped like the token AWS issues.
AWS: Smithy: CreateParticipantToken output ParticipantToken$token targets ParticipantTokenString (string, @sensitive), documented "The issued client token, encrypted" and "treat tokens as opaque". Recorded behavior only: AWS's recorded token is a JWT — three base64url segments (alphabet A-Za-z0-9-_, no padding) joined by two '.' separators — which is the shape the IVS broadcast/player SDKs parse. The corpus asserts that shape, not the literal bytes (expected '<shaped-not-asserted>').
Emulator: returns a single ~1 KB standard-base64 blob with no '.' separator at all and characters outside base64url ('+', '/', '='), so it fails the token's shape assertion.
Why: although the token is documented as opaque, every real client hands it to an IVS SDK that splits it on '.' and base64url-decodes the segments; a blob with no separators is rejected before the join is attempted, so a stage join cannot be exercised against the emulator with a token it issued itself.
Fix: the CreateParticipantToken handler's token minting: emit the token as dot-separated base64url segments (header/payload/signature), using the url-safe alphabet without padding, instead of a single standard-base64 blob.
Traces: ivs-realtime-create-participant-token-empty-capabilities-list-honored#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (14 fixtures, 56 steps, 40 passed, 16 mismatched, score 71.4%; emulator up). 6 clusters, all verdict=mismatch, none carrying stub_path or owned_by:

1. ivs-realtime-update-ingest-configuration-redundant-ingest-toggle#task#3 (UpdateIngestConfiguration, expected 200, got 200): $.ingestConfiguration.redundantIngest: expected True, got False; $.ingestConfiguration.redundantIngestCredentials: missing (expected [{'participantId': <shaped>, 'streamKey': <shaped>}, {'participantId': <shaped>, 'streamKey': <shaped>}], got None). (Same cluster also reports userId/CORS-header mismatches filed in the projection bundle.)
2. ...#task#4 (GetIngestConfiguration, 200->200): redundantIngest expected True, got False; redundantIngestCredentials missing (expected 2 credentials, got None).
3. ...#task#5 (UpdateIngestConfiguration, 200->200): redundantIngestCredentials missing (expected 2 credentials, got None).
4. ...#task#6 (GetIngestConfiguration, 200->200): redundantIngestCredentials missing (expected 2 credentials, got None).
5. ivs-realtime-disconnect-participant-unknown-id-is-noop-success#task#2 (DisconnectParticipant): status expected 200, got 404; body expected {}, got {'__type': 'ResourceNotFoundException', 'message': 'Resource: <participantId> not found', 'Code': 'ResourceNotFoundException', 'Message': 'Resource: <participantId> not found', 'RequestID': <per-run value>}. The participantId in the message is the one sent in the request; the RequestID is a per-run value and is not part of the finding.
6. ivs-realtime-create-participant-token-empty-capabilities-list-honored#task#2 (CreateParticipantToken, 200->200): $.participantToken.token: value_mismatch (expected '<shaped-not-asserted>' — the JWT shape — got a single ~1 KB standard-base64 string containing '+', '/' and '=' and no '.' separator).

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: create an ingest configuration, then POST /UpdateIngestConfiguration with redundantIngest=true and read it back with GetIngestConfiguration — fixture ivs-realtime-update-ingest-configuration-redundant-ingest-toggle, steps task#3..task#6. Observe redundantIngest=false on the update response and on every read, and redundantIngestCredentials never present.
3. Cause 2: create a stage, then POST /DisconnectParticipant with that stageArn and a well-formed participantId that names no participant in the stage — fixture ivs-realtime-disconnect-participant-unknown-id-is-noop-success, step task#2. Observe 404 ResourceNotFoundException "Resource: <participantId> not found" where AWS answers 200 {}.
4. Cause 3: POST /CreateParticipantToken with an empty capabilities list — fixture ivs-realtime-create-participant-token-empty-capabilities-list-honored, step task#2. Inspect participantToken.token: a single standard-base64 blob with no '.' separators, not the three dot-separated base64url segments AWS returns.
