Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-realtime-stage-participant-token-attributes-roundtrip
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-realtime-stage-participant-token-attributes-roundtrip
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime|CreateStage|2xx->2xx|$.participantTokens[*].duration
Issue-Commit: 75a7ee1add074dc1515110b8db46e44025ab2118

# CreateStage omits ParticipantToken$duration from the minted tokens instead of reporting the effective duration

> _Note: this issue was identified against the code base at commit `75a7ee1add074dc1515110b8db46e44025ab2118`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
One cause, found by replaying the wire corpus (1 divergence cluster, verdict=mismatch, no stub_path and no owned_by).

Cause 1 of 1 - CreateStage mints the participant tokens requested by participantTokenConfigurations but projects each ParticipantToken without its `duration` member, so the response never states how long the token it just issued is valid for.

AWS: Smithy: CreateStage (POST /CreateStage) input `participantTokenConfigurations` targets ParticipantTokenConfigurations (list, @length min 0 max 12) whose member ParticipantTokenConfiguration carries `duration` -> ParticipantTokenDurationMinutes (integer, @range min 1 max 20160), documented "Duration (in minutes), after which the corresponding participant token expires. Default: 720 (12 hours)." The output CreateStageResponse$participantTokens targets ParticipantTokenList of ParticipantToken, which carries the same `duration` -> ParticipantTokenDurationMinutes member, documented "Duration (in minutes), after which the participant token expires. Default: 720 (12 hours)." So for every entry of participantTokenConfigurations, AWS answers 200 with the corresponding ParticipantToken populated with the EFFECTIVE duration: the value the configuration supplied, or the documented default 720 when the configuration omitted it. The recorded response confirms this: both minted tokens report the default duration for a configuration that did not set one.

Emulator: answers 200 with the right number of tokens, but `duration` is absent from every element of participantTokens - the replay reports `$.participantTokens[0].duration: missing (expected 720, got None)` and `$.participantTokens[1].duration: missing (expected 720, got None)`. The default is evidently applied nowhere and the member is simply not projected; the 720 in the recording is the modeled DEFAULT, not a per-run value, so it is part of the requirement.

Why: `duration` is how a client learns the lifetime of a token it did not fully specify. A client that reads participantTokens[i].duration to schedule a refresh (or to validate it against expirationTime) gets null from the emulator, so either it cannot schedule a renewal at all or it deserializes into a non-optional integer and fails. Because the member is absent rather than wrong, the bug is also invisible to a client that only checks for a successful 200, and it hides whether the service applied a requested duration or the default - the one thing the field exists to report.

Fix: the CreateStage handler's token-minting and response projection for ParticipantToken: resolve each token's effective duration (the configuration's `duration` when present, otherwise the modeled default 720) into the stored participant token, and serialize that member on every element of participantTokens. The same projection is what GetParticipant/CreateParticipantToken responses share, so apply the default at mint time in state rather than only in this serializer.

Traces: ivs-realtime-stage-participant-token-attributes-roundtrip#task#1 (CreateStage). Unrelated to the open CreateStage issue 20261029T090134Z-ivs-realtime-createstage-validation--r2.md (thumbnail targetIntervalSeconds validation), which does not carry this replay key, and not covered by 20261031T073914Z-ivs-realtime-responses-omit-modeled-security-and-cors.md (response headers only).

## Evidence
Replay of trace set simparity-wire-aws-v1 (14 fixtures, 56 steps, 45 passed, 11 mismatched, score 80.4%; emulator_up=true). Cluster: fixture ivs-realtime-stage-participant-token-attributes-roundtrip, step task#1, operation CreateStage, service ivs-realtime, verdict=mismatch, expected_status 200, actual_status 200, stub_path empty, owned_by empty, affected_count 1.

divergence_key: service=ivs-realtime|CreateStage|status=200->200|keys=$.participantTokens[0].duration,$.participantTokens[1].duration

detail:
  $.participantTokens[0].duration: missing (expected 720, got None)
  $.participantTokens[1].duration: missing (expected 720, got None)

Expected (AWS): each minted ParticipantToken carries `duration`, an integer in [1, 20160], equal to the requested ParticipantTokenConfiguration$duration or to the documented default 720 when none was requested. Actual (emulator): the `duration` member is absent from every element of participantTokens. The status (200) and the token count match; only this member diverges. The expected value 720 is the model's documented default for an unspecified duration, not an account- or run-specific value.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. POST /CreateStage with two entries in participantTokenConfigurations that set userId/attributes but omit `duration` - fixture ivs-realtime-stage-participant-token-attributes-roundtrip, step task#1.
3. Inspect the 200 response body: participantTokens has two elements, each with participantId/token, but neither element contains a `duration` member.
4. AWS returns duration=720 (the documented default) on both elements; assert `$.participantTokens[*].duration` is present and an integer in [1, 20160] to see the divergence.
