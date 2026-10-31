Issue-Source: trace-sweep-divergence
Issue-Trace: rolesanywhere-crl-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rolesanywhere-crl-lifecycle
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere|ListCrls|2xx->2xx|$.crls[name="sim2real-probe-rolesanywhere-crl-7edb1a4f"].createdAt
Issue-Key: rolesanywhere|ListCrls|2xx->2xx|$.crls[name="sim2real-probe-rolesanywhere-crl-7edb1a4f"].updatedAt
Issue-Commit: 7779c16d6d878b1b8e8c99e1673984c1963db177

# rolesanywhere ListCrls serializes CrlDetail timestamps as epoch floats

> _Note: this issue was identified against the code base at commit `7779c16d6d878b1b8e8c99e1673984c1963db177`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCrls

## Description
This issue carries 1 cause, in the rolesanywhere CRL list surface.

Cause 1 of 1: ListCrls emits CrlDetail.createdAt / updatedAt as bare JSON numbers (unix epoch seconds with a fractional part) instead of ISO-8601 date-time strings.
AWS: Smithy (services/service-com-amazonaws-rolesanywhere-rolesanywhere/smithy-model/com-amazonaws-rolesanywhere-rolesanywhere.json): ListCrls -> GET /crls, code 200, output shape ListCrlsResponse with member crls targeting the list CrlDetails of CrlDetail; errors AccessDeniedException (403) and ValidationException (400). CrlDetail.createdAt and CrlDetail.updatedAt target Timestamp with timestampFormat "date-time" and are documented "The ISO-8601 timestamp when the certificate revocation list (CRL) was created." / "... was last updated." So each element of crls must carry those two members as RFC3339/ISO-8601 date-time strings of the form YYYY-MM-DDThh:mm:ss.sssssssssZ. The particular instant is per-run state and is not itself a requirement.
Emulator: for every element of $.crls the emulator writes a JSON number for both members (e.g. 1793454724.1813862 where the recording carries a date-time string). The status is correct (200) and the rest of the CrlDetail projection matches; only the timestamp encoding diverges.
Why: every AWS SDK decodes a date-time-formatted member of this protocol from a JSON string. A number makes the client either fail to deserialize ListCrlsResponse or surface a wrong instant, so any client that pages CRLs and reads createdAt/updatedAt breaks on the response shape itself. The read-back path (GetCrl) and the write path (ImportCrl) have the same fault, so a consumer never obtains the documented ISO-8601 instant by any route.
Fix: the rolesanywhere ListCrls response serializer — specifically the shared CrlDetail serialization helper used when projecting list elements, which must honour timestampFormat=date-time rather than writing the raw epoch value.
Traces: rolesanywhere-crl-lifecycle#task#4.

Related open issue: 20261030T110232Z-rolesanywhere-crl-operations-serialize-timestamps-as-epoch-floats.md describes the same encoding fault for ImportCrl and GetCrl, but it does NOT carry this ListCrls replay key (its Issue-Key lines name only ImportCrl and GetCrl), so the list projection is not covered by it and is filed here. If both are fixed through one shared helper, this key must still be replayed to confirm the list path.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up true, 85 steps, 21 passed, score 24.7%).

rolesanywhere-crl-lifecycle#task#4, ListCrls, status 200 expected vs 200 actual (status matches; body diverges):
  $.crls[name="sim2real-probe-rolesanywhere-crl-7edb1a4f"].createdAt: value_mismatch (expected '2026-09-06T05:29:00.561083027Z', got 1793454724.1813862)
  $.crls[name="sim2real-probe-rolesanywhere-crl-7edb1a4f"].updatedAt: value_mismatch (expected '2026-09-06T05:29:00.561083027Z', got 1793454724.1813862)

The expected instants above and the CRL name in the selector are that recording's per-run values; the requirement is the ISO-8601 date-time string form of whatever instant the emulator's state holds.

Model citation: CrlDetail.createdAt / CrlDetail.updatedAt — target Timestamp, trait timestampFormat "date-time", docs "The ISO-8601 timestamp when the certificate revocation list (CRL) was created/was last updated."; ListCrls — GET /crls, code 200, output ListCrlsResponse{nextToken, crls: CrlDetails}, paginated (inputToken/outputToken nextToken, items crls).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout; confirm /__admin/health answers 200.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Replay fixture rolesanywhere-crl-lifecycle. Setup creates a trust anchor, step task#2 imports a CRL (ImportCrl), then step task#4 calls ListCrls (GET /crls).
4. Inspect the 200 body: for the CRL element in $.crls, the members createdAt and updatedAt are JSON numbers (epoch seconds with a fraction) rather than ISO-8601 date-time strings, so the step is reported as a mismatch on $.crls[...].createdAt and $.crls[...].updatedAt.
