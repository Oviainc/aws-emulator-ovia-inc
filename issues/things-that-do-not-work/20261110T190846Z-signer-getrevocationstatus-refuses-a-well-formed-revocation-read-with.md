Issue-Source: trace-sweep-divergence
Issue-Trace: signer-certificate-hashes-empty-list-rejected
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: signer-certificate-hashes-empty-list-rejected
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer|GetRevocationStatus|2xx->4xx|$.revokedEntities
Issue-Key: signer|GetRevocationStatus|2xx->4xx|status
Issue-Commit: bd416c0ab091bf65c958786969d0a3369dfa0e54

# GetRevocationStatus refuses a well-formed revocation read with 400 instead of answering 200 with an empty revokedEntities list

> _Note: this issue was identified against the code base at commit `bd416c0ab091bf65c958786969d0a3369dfa0e54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRevocationStatus

## Description
Cause 1 of 1: the wire-recorded, fully well-formed GetRevocationStatus read (GET /revocations with all five @required query members supplied — platformId naming a published signing platform, a non-empty certificateHashes, a signatureTimestamp, and ARN-shaped profileVersionArn and jobArn values) is answered HTTP 400 by the emulator instead of HTTP 200 carrying the modeled revokedEntities member. This is the positive-control step of the fixture, i.e. the request shape every other behavior of this operation is reached through, so the operation's success path is unreachable on this request shape.

AWS: HTTP 200 with a body carrying the modeled output member revokedEntities, present and empty when nothing the request named is revoked. Smithy (services/service-com-amazonaws-signer-wallabyservice/smithy-model/com-amazonaws-signer-wallabyservice.json, com.amazonaws.signer#GetRevocationStatus): http {method GET, uri /revocations, code 200}, endpoint hostPrefix "data-"; input members signatureTimestamp (Timestamp, @httpQuery signatureTimestamp, @required), platformId (PlatformId, @httpQuery, @required), profileVersionArn and jobArn (both target Arn — a string with @length min 20 max 2048 and NO @pattern, @httpQuery, @required), certificateHashes (CertificateHashes list, @httpQuery, @required); output member revokedEntities (RevokedEntities list); modeled errors ValidationException (client, httpError 400), AccessDeniedException (403), TooManyRequestsException (429), InternalServiceErrorException (500). The model constrains the two ARN members by LENGTH only and places no constraint on their internal segments, and the operation models NO not-found error — so an ARN that is well formed but names a profile version or job this endpoint holds nothing for must be answered as an entity that is simply not revoked (200, revokedEntities present and empty), never refused. The recorded response for this step is exactly that: status 200, $.revokedEntities == [].

Emulator: HTTP 400 with no revokedEntities member at all ($.revokedEntities expected [], got None) — i.e. a ValidationException-shaped refusal where the recording has a successful read. Hypothesis for which gate fires: ops/get_revocation_status.rs validates the two ARN members by resolving them through crate::state::profile_version_in_arn / job_region_and_id_in_arn (lines 232-245) and refuses with crate::state::invalid_arn_message when either answers None; profile_version_in_arn additionally requires is_profile_name(name) (src/state.rs), which applies the ProfileName MEMBER's 2..64 length floor to a name segment read out of an Arn-typed value, so a well-formed /signing-profiles/<name>/<version> ARN whose name segment is a single character is declared malformed. That is the same trigger the open behavioral issue 20261109T154607Z-signer-getrevocationstatus-refuses-a-well-formed-profileversionarn-whose.md documents for the same-named behavioral check (signer.certificate-hashes-empty-list-rejected). That issue does NOT cover this wire divergence: it is filed against the behavioral corpus and carries no replay key for this fixture step, so nothing re-replays this fixture when it is closed. The replay report confirms no open issue owns this cluster (owned_by empty).

Why: a client performing a revocation check on a profile version and signing job ARN it was handed is told its request is invalid instead of being told whether the signature is revoked. The operation is a trust decision and has no not-found channel, so the caller cannot distinguish "this endpoint will not parse my ARN" from "my ARN is wrong", and it has no way to obtain the answer at all. Every downstream behavior of the operation (certificateHashes validation, signatureTimestamp bounding, platform-catalog refusal, authorization, read consistency) is also unreachable behind this refusal.

Fix: GetRevocationStatus' input validation in the signer crate — the profileVersionArn and jobArn gates of ops/get_revocation_status.rs must judge ARN SHAPE only (partition/service/region/account plus the /signing-profiles/<name>/<version> and /signing-jobs/<id> resource forms) and must not import a member-level name-length constraint from the profile-management operations into an Arn-typed request value; the ARN parsers in src/state.rs that those gates call must tolerate name segments the profile-management operations would not accept, and the subsequent store lookup must answer "not revoked" (200 with revokedEntities present and empty) for anything it holds no record of rather than refusing. The "Invalid <member>: <value>" refusal must stay for values that genuinely are not ARN-shaped.

Traces: fixture signer-certificate-hashes-empty-list-rejected, step task#1 (GetRevocationStatus).

## Evidence
Wire-corpus replay of simparity-wire-aws-v1 against the emulator built from this checkout: 9 fixtures replayed, 49 steps, 39 passed, 10 mismatched, score 79.6%, emulator_up true (/__admin/health 200, data listener accepting).

Divergence cluster (verdict mismatch, divergence_key "service=signer|GetRevocationStatus|status=200->200|keys=$.revokedEntities,status" as reported; affected = [signer-certificate-hashes-empty-list-rejected#task#1], affected_count 1, stub_path empty, owned_by empty):
  fixture_id: signer-certificate-hashes-empty-list-rejected
  step_key:   task#1
  operation:  GetRevocationStatus (service signer)
  detail:     "status: expected 200, got 400" and "$.revokedEntities: missing (expected [], got None)"

Expected (recorded AWS response for the step): HTTP 200, body containing revokedEntities == [].
Actual (emulator): HTTP 400, body with no revokedEntities member.

The request values (account id, region, the profile-name and version segments of the ARNs, the certificate hash filler, the timestamp) are per-recording filler; the requirement is that a GetRevocationStatus request whose members all satisfy the modeled constraints is answered 200 with revokedEntities present, empty when nothing it names is revoked.

Source grounding in the checkout: services/service-com-amazonaws-signer-wallabyservice/src/ops/get_revocation_status.rs lines 194-245 are the only gates ahead of the store read — platform_id_pattern_refusal, the empty-certificateHashes gate (lines 207-211), the platform-catalog gate (217-221), then the profileVersionArn gate (232-239) and jobArn gate (240-245), both refusing via crate::state::invalid_arn_message when the state.rs ARN parser answers None; lines 258-299 then build revokedEntities and always emit it. Model facts from smithy_query(query="operation", operation="GetRevocationStatus", model_path="services/service-com-amazonaws-signer-wallabyservice/smithy-model/com-amazonaws-signer-wallabyservice.json").

## Steps to Reproduce
1. Clone the repo and build/boot the emulator from this branch (branch issues/finder-wi-1b250c7e263a4584acc9d6a91ea54255-1794336928119).
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Filter to fixture signer-certificate-hashes-empty-list-rejected, step task#1 (GetRevocationStatus, GET /revocations).
4. Observe HTTP 400 with no revokedEntities member, where the recorded response is HTTP 200 with revokedEntities == [].
5. Minimal reproduction: issue GET /revocations against the signer data endpoint with platformId=Notation-OCI-SHA384-ECDSA, a non-empty certificateHashes, a past signatureTimestamp, profileVersionArn=arn:aws:signer:<region>:<account>:/signing-profiles/<one-character name>/<10-char version> and jobArn=arn:aws:signer:<region>:<account>:/signing-jobs/<id> — all members inside their modeled @length bands. Observe the 400 refusal instead of 200 {"revokedEntities": []}.
6. Widen the profile-name segment of profileVersionArn to two characters and repeat: the same request is served 200, which isolates the gate to the name-segment length floor applied inside the ARN parser.
7. Confirm the refusal is still produced for a value that genuinely is not ARN-shaped (profileVersionArn set to a non-ARN string of the same length): that must remain 400 ValidationException "Invalid profileVersionArn: <value>".
