Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-realtime-stage-endpoints-present-for-all-protocols
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-realtime-stage-endpoints-present-for-all-protocols ivs-realtime-disconnect-participant-unknown-id-is-noop-success ivs-realtime-stage-participant-token-attributes-roundtrip
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime|GetStage|2xx->2xx|$.stage.endpoints.events
Issue-Key: ivs-realtime|GetStage|2xx->2xx|$.stage.endpoints.whip
Issue-Key: ivs-realtime|CreateStage|2xx->2xx|$.stage.endpoints.whip
Issue-Related: 20261106T133006Z-ivs-realtime-createstage-omits-participanttoken-duration-from-the-minted-tokens.md
Issue-Commit: 46e6021cf6fe7535ec502772e1179208927a5024

# ivs-realtime stage responses announce upstream live-video.net endpoints and omit participantToken duration

> _Note: this issue was identified against the code base at commit `46e6021cf6fe7535ec502772e1179208927a5024`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStage

## Description
Three causes, found by replaying the wire corpus simparity-wire-aws-v1 (3 net-new divergence clusters, all verdict=mismatch, all with empty stub_path and empty owned_by). The shared fault is the projection of the Stage structure that GetStage and CreateStage both return: its endpoints members are emitted as the upstream Amazon IVS production hostnames instead of the address the emulator is actually reached on, and the same CreateStage projection also drops ParticipantToken$duration. All three causes are ivs-realtime and all three must be closed before this issue is submitted.

Cause 1 of 3 - GetStage projects Stage$endpoints$whip as an upstream IVS host, not as an endpoint served by the emulator.
AWS: Smithy: GetStage (POST /GetStage), output GetStageResponse$stage -> Stage, whose member endpoints -> StageEndpoints carries whip/events/rtmp/rtmps, each targeting StageEndpoint (string, @length min 0 max 2048). StageEndpoints is documented "Summary information about various endpoints for a stage. We recommend that you cache these values at stage creation"; whip is "The endpoint to be used for IVS real-time streaming using the WHIP protocol." So the requirement is structural: for a stage that exists, GetStage answers 200 with endpoints whose values are absolute URLs on the host that actually serves that stage's real-time traffic for the caller. On AWS that host is the IVS production edge; on an emulator it is the emulator's own data listener, because the emulator is the thing serving the stage. The URL's fixed text is the scheme and the protocol-specific path/subdomain shape; the host, the per-stage id label and the port are values taken from the serving deployment and the stage's state, never from one recording.
Emulator: answers 200 but returns a whip endpoint on an upstream Amazon production domain (a 'https://<hex-label>.global-bm.whip.live-video.net' form). The replay check reports '$.stage.endpoints.whip: host_not_own' - the response announces an address other than the one the emulator is reached on, so the value is wrong irrespective of the recorded label's digits.
Why: a client does exactly what the field tells it to - it caches the endpoint and then publishes/subscribes against it. Pointed at live-video.net, the client leaves the sandbox: it either egresses to real AWS (unauthenticated, so it fails late with a transport or auth error that names no emulator), or it cannot resolve/reach the host at all. The failure surfaces in the client's media path rather than in the API call that handed out the bad value, so the emulator looks healthy while every real-time session built on it is unusable.
Fix: the GetStage handler's Stage projection (and the shared stage-endpoint construction in state, so every operation that returns a Stage agrees): build the endpoint URLs from the emulator's own advertised host and port for this request, keeping the per-protocol scheme/shape and the stage's own id label, instead of emitting constant upstream IVS domains.
Traces: ivs-realtime-stage-endpoints-present-for-all-protocols#task#2 (GetStage, $.stage.endpoints.events and $.stage.endpoints.whip both host_not_own) and ivs-realtime-disconnect-participant-unknown-id-is-noop-success#task#3 (GetStage, $.stage.endpoints.whip host_not_own).

Cause 2 of 3 - the events endpoint has the same defect, on the websocket scheme.
AWS: Smithy: StageEndpoints$events -> StageEndpoint, documented "Events endpoint." AWS returns a wss:// URL on the host that serves the stage's event channel for the caller. Fixed text: the wss scheme and the events-endpoint path shape; variable: the serving host/port.
Emulator: returns 'wss://global.events.live-video.net' - a constant upstream Amazon host, reported by the replay as '$.stage.endpoints.events: host_not_own (expected <shaped-not-asserted>, got wss://global.events.live-video.net)'. The expectation is shape-and-ownership, not the recorded literal.
Why: the events channel is how an SDK learns participant join/leave and publish state. A client that opens the announced wss URL dials out to a real Amazon endpoint it has no credentials for, so it never receives stage events and typically reports a connection or handshake failure that implicates the network rather than the emulator.
Fix: same component as Cause 1 - the shared stage-endpoint builder in state plus the Stage serializer: derive the events URL from the emulator's own listener address and scheme (wss/ws as served) rather than hardcoding the production events domain.
Traces: ivs-realtime-stage-endpoints-present-for-all-protocols#task#2 (GetStage).

Cause 3 of 3 - CreateStage returns the newly minted participant tokens without ParticipantToken$duration, and also announces the upstream whip endpoint.
AWS: Smithy: CreateStage (POST /CreateStage) input participantTokenConfigurations -> ParticipantTokenConfigurations (list, @length min 0 max 12); output CreateStageResponse$participantTokens -> ParticipantTokenList of ParticipantToken, whose member duration -> ParticipantTokenDurationMinutes (integer, @range min 1 max 20160) is documented "Duration (in minutes), after which the participant token expires. Default: 720 (12 hours)." So every minted token is returned with its EFFECTIVE duration: the value the matching configuration supplied, or the documented default 720 when it supplied none. The 720 in the recording is the model's documented default, not a per-run value. CreateStageResponse$stage is the same Stage structure as Cause 1, so its endpoints carry the same ownership requirement.
Emulator: answers 200 with the correct number of tokens, but duration is absent from every element - the replay reports '$.participantTokens[0].duration: missing (expected 720, got None)' and '$.participantTokens[1].duration: missing (expected 720, got None)' - and the response's $.stage.endpoints.whip is again host_not_own (a global-bm.whip.live-video.net URL).
Why: duration is how a client learns the lifetime of a token it did not fully specify. Reading it back as null, a client cannot schedule a refresh before expiry, or it deserializes into a non-optional integer and fails outright; and because the member is merely absent, a client that only checks for 200 never sees that the service applied neither the requested duration nor the default. The endpoint half has the Cause 1 consequence: the stage handed back at creation time already points callers out of the emulator.
Fix: the CreateStage handler - resolve each token's effective duration at mint time in state (configuration value, else the modeled default 720) and serialize that member on every element of participantTokens; and route the response's Stage through the same own-host endpoint builder as Cause 1.
Traces: ivs-realtime-stage-participant-token-attributes-roundtrip#task#1 (CreateStage). The duration half overlaps open issue 20261106T133006Z-ivs-realtime-createstage-omits-participanttoken-duration-from-the-minted-tokens.md, but replay reports this cluster with owned_by empty - the cluster's key now also covers $.stage.endpoints.whip, so that issue does not carry this cluster's replay key and does not close it. The header-only issues 20261031T073914Z-ivs-realtime-responses-omit-modeled-security-and-cors.md and 20261108T084350Z-ivs-realtime-updateingestconfiguration-response-omits-cors-and-security.md cover response headers and are unrelated to these body members.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true (/__admin/health 200, data listener accepting), 14 fixtures replayed, 56 steps, 47 passed, 9 mismatched, 0 errored, 0 blocked, score 83.9%, divergence_count=7. Four of the seven clusters carry owned_by and are skipped; the three below are net-new (stub_path empty, owned_by empty).

Cluster A (representative) - fixture ivs-realtime-stage-endpoints-present-for-all-protocols, step task#2, operation GetStage, service ivs-realtime, verdict=mismatch, expected_status 200, actual_status 200, affected_count 1.
divergence_key: service=ivs-realtime|GetStage|status=200->200|keys=$.stage.endpoints.events,$.stage.endpoints.whip
detail:
  $.stage.endpoints.events: host_not_own (expected '<shaped-not-asserted>', got 'wss://global.events.live-video.net')
  $.stage.endpoints.whip: host_not_own (expected '<shaped-not-asserted>', got 'https://000001fef943.global-bm.whip.live-video.net')

Cluster B - fixture ivs-realtime-disconnect-participant-unknown-id-is-noop-success, step task#3, operation GetStage, verdict=mismatch, expected_status 200, actual_status 200, affected_count 1.
divergence_key: service=ivs-realtime|GetStage|status=200->200|keys=$.stage.endpoints.whip
detail: $.stage.endpoints.whip: host_not_own - the emulator serves a 'https://<hex>.global-bm.whip.live-video.net' URL; the check is host ownership, so the recorded label's digits are not asserted.

Cluster C - fixture ivs-realtime-stage-participant-token-attributes-roundtrip, step task#1, operation CreateStage, verdict=mismatch, expected_status 200, actual_status 200, affected_count 1.
divergence_key: service=ivs-realtime|CreateStage|status=200->200|keys=$.participantTokens[0].duration,$.participantTokens[1].duration,$.stage.endpoints.whip
detail:
  $.participantTokens[0].duration: missing (expected 720, got None)
  $.participantTokens[1].duration: missing (expected 720, got None)
  $.stage.endpoints.whip: host_not_own (got a global-bm.whip.live-video.net URL)

Expected (AWS): Stage$endpoints.{events,whip,rtmp,rtmps} are absolute URLs on the host that actually serves that stage for the caller - on the emulator, its own advertised host/port - with the per-protocol scheme and shape as the fixed part and the host, port and stage id label as values from the deployment and state; and every element of CreateStageResponse$participantTokens carries duration, an integer in [1,20160], equal to the requested ParticipantTokenConfiguration$duration or to the documented default 720.
Actual (emulator): constant upstream Amazon production hosts (global.events.live-video.net, *.global-bm.whip.live-video.net) in Stage$endpoints, and no duration member at all on the minted tokens. Statuses (200) and token counts match; only these members diverge.

## Steps to Reproduce
1. Clone this repo, boot the emulator built from it, and replay trace set simparity-wire-aws-v1.
2. Cause 1/2: POST /CreateStage then POST /GetStage for that stage ARN - fixture ivs-realtime-stage-endpoints-present-for-all-protocols, step task#2 (and fixture ivs-realtime-disconnect-participant-unknown-id-is-noop-success, step task#3). Inspect the 200 body: $.stage.endpoints.events is 'wss://global.events.live-video.net' and $.stage.endpoints.whip is a '*.global-bm.whip.live-video.net' URL. Assert that the host of each endpoint equals the host:port the emulator is being reached on to see the divergence.
3. Cause 3: POST /CreateStage with two participantTokenConfigurations that set userId/attributes and omit duration - fixture ivs-realtime-stage-participant-token-attributes-roundtrip, step task#1. In the 200 body, participantTokens has two elements each with participantId/token but no duration member; assert $.participantTokens[*].duration is present and in [1,20160] (720 for an unspecified duration). The same response's $.stage.endpoints.whip again names an upstream live-video.net host.
4. Re-replay all three fixture steps after the fix: the endpoints must resolve to the emulator's own listener and duration must be populated on every minted token.
