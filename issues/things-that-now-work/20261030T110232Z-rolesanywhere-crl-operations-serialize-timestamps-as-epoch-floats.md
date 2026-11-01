Issue-Source: trace-sweep-divergence
Issue-Trace: rolesanywhere-crl-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rolesanywhere-crl-lifecycle rolesanywhere-not-found-error-paths rolesanywhere-validation-error-paths
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere|ImportCrl|2xx->2xx|$.crl.createdAt
Issue-Key: rolesanywhere|ImportCrl|2xx->2xx|$.crl.updatedAt
Issue-Key: rolesanywhere|GetCrl|2xx->2xx|$.crl.createdAt
Issue-Key: rolesanywhere|GetCrl|2xx->2xx|$.crl.updatedAt
Issue-Key: rolesanywhere|GetCrl|4xx->2xx|$.message
Issue-Key: rolesanywhere|GetCrl|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: rolesanywhere|GetCrl|4xx->2xx|status
Issue-Key: rolesanywhere|GetCrl|4xx->4xx|$.message
Issue-Key: rolesanywhere|ImportCrl|4xx->2xx|$.message
Issue-Key: rolesanywhere|ImportCrl|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: rolesanywhere|ImportCrl|4xx->2xx|status
Issue-Commit: 14fcb532a6243119e1963a7bcc0017c1001c0e15

# rolesanywhere CRL operations serialize timestamps as epoch floats and skip CRL not-found refusals

> _Note: this issue was identified against the code base at commit `14fcb532a6243119e1963a7bcc0017c1001c0e15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportCrl

## Description
This issue bundles 5 distinct causes, all in the rolesanywhere CRL surface (ImportCrl / GetCrl / DeleteCrl). Two are wire-serialization faults on CrlDetail timestamps, three are missing or mis-worded ResourceNotFoundException refusals. A fixer must close ALL FIVE before submitting.

Cause 1 of 5: ImportCrl serializes CrlDetail.createdAt / updatedAt as epoch floats.
AWS: Smithy: ImportCrl -> output CrlDetailResponse.crl (CrlDetail, required), http POST /crls code 201. CrlDetail.createdAt and CrlDetail.updatedAt target Timestamp with timestampFormat "date-time" and are documented as "The ISO-8601 timestamp when the certificate revocation list (CRL) was created/was last updated." So both members must be emitted as RFC3339/ISO-8601 date-time strings (e.g. a string of the form YYYY-MM-DDThh:mm:ss.ssssssssZ); the instant itself is per-run and is not a requirement.
Emulator: emits a bare JSON number (unix epoch seconds with a fractional part) for both members, e.g. 1793358034.3732877 where the recording carries a date-time string.
Why: every AWS SDK decodes a date-time-formatted member from a string for this protocol; a JSON number makes the client either fail to parse the field or surface a wrong instant, so clients that read CRL createdAt/updatedAt break on the response shape itself, not just the value.
Fix: the rolesanywhere ImportCrl response serializer for CrlDetail — the timestamp member serialization must honour timestampFormat=date-time instead of writing the raw epoch value.
Traces: rolesanywhere-crl-lifecycle#task#2.

Cause 2 of 5: GetCrl serializes CrlDetail.createdAt / updatedAt as epoch floats.
AWS: Smithy: GetCrl -> GET /crl/{crlId}, 200, output CrlDetailResponse.crl (CrlDetail, required); same CrlDetail.createdAt/updatedAt Timestamp members with timestampFormat "date-time".
Emulator: the read path returns the same epoch-float numbers for createdAt and updatedAt as the write path does.
Why: a client that imports a CRL and then reads it back gets an undecodable/incorrect timestamp on both paths, so round-tripping a CRL never yields the ISO-8601 instants AWS documents.
Fix: the rolesanywhere GetCrl response serializer (same CrlDetail serialization helper as Cause 1) — fixing one without the other leaves the read path diverging.
Traces: rolesanywhere-crl-lifecycle#task#3.

Cause 3 of 5: GetCrl on a deleted CRL returns 200 with the CRL instead of 404 ResourceNotFoundException.
AWS: Smithy: GetCrl errors = ResourceNotFoundException (client, httpError 404), documented "The resource could not be found." After DeleteCrl (modeled with ResourceNotFoundException too), the CRL id no longer resolves, so GetCrl must answer HTTP 404 with header x-amzn-errortype: ResourceNotFoundException (the recording shows the Coral-suffixed form "ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.rolesanywhere.service/") and body member message. The recorded message text is "The item doesn't exist. arn: <crl-arn> ; aborting." — the fixed text is "The item doesn't exist. arn:", then a space, the ARN of the requested CRL built from the request's crlId and the emulator's partition/region/account, then " ; aborting.". The ARN in any one recording is a per-run value, not part of the template.
Emulator: answers HTTP 200 and serves the deleted CRL body — no message member and no x-amzn-errortype header at all. The CRL therefore survives its own DeleteCrl as far as GetCrl is concerned.
Why: a client cannot tell a deleted CRL from a live one; delete-then-verify flows and idempotent cleanup loops read the stale record as success, and error handling that branches on ResourceNotFoundException never fires.
Fix: rolesanywhere DeleteCrl/GetCrl state handling — DeleteCrl must actually remove the CRL from the store (or mark it gone), and GetCrl must raise ResourceNotFoundException for an unresolvable crlId.
Traces: rolesanywhere-crl-lifecycle#teardown#10.

Cause 4 of 5: GetCrl's 404 refusal carries an emulator-authored message instead of AWS wording.
AWS: Smithy: ResourceNotFoundException (client, 404) with a single message member. AWS words this refusal as "The item doesn't exist. arn: <crl-arn> ; aborting." — fixed text "The item doesn't exist. arn:" + the requested CRL's ARN (derived from the request crlId plus partition/region/account) + " ; aborting.".
Emulator: returns the right status (404) but the message is the three-word emulator string "Crl not found" — neither the fixed text nor the ARN variable part is present.
Why: clients and operators that match on or display AWS's refusal text get a message that identifies no resource; the ARN is the only part of the refusal naming what was missing.
Fix: the rolesanywhere GetCrl handler's not-found error construction — render the modeled message template rather than an ad-hoc string. Note the existing open issue 20261029T192145Z-rolesanywhere-not-found-refusals-carry-emulator-authored-messages.md covers the Profile/TrustAnchor variants of this wording fault and does NOT carry this CRL replay key; 20261030T104446Z-rolesanywhere-*-stub.md issues cover unimplemented CRL ops only.
Traces: rolesanywhere-not-found-error-paths#task#3.

Cause 5 of 5: ImportCrl accepts a trustAnchorArn that names no trust anchor, returning 201 instead of 404 ResourceNotFoundException.
AWS: Smithy: ImportCrl input trustAnchorArn is required, TrustAnchorArn (length 1..1011, pattern ^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:trust-anchor.*)$); the modeled error list is AccessDeniedException/ValidationException only, so the not-found behavior here is recorded behavior only: AWS resolves the referenced trust anchor and refuses an unknown one with HTTP 404, header x-amzn-errortype ResourceNotFoundException (recorded as "ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.rolesanywhere.service/") and message "Unable to find Trust Anchor with ID <id>." — fixed text "Unable to find Trust Anchor with ID " followed by the trust-anchor UUID taken from the request's trustAnchorArn, then ".". The UUID in the recording is a per-run value.
Emulator: performs no existence check: it answers HTTP 201 with a freshly created CRL whose trustAnchorArn points at a nonexistent trust anchor, emitting no message and no x-amzn-errortype.
Why: the emulator creates dangling CRLs that reference nothing, so a client's typo or stale ARN silently "succeeds" and revocation is attached to no trust anchor; negative tests expecting a 404 pass against AWS and fail against the emulator.
Fix: rolesanywhere ImportCrl handler validation — resolve trustAnchorArn against trust-anchor state after the pattern check and raise ResourceNotFoundException when it does not exist. The existing open issue 20261029T192145Z-rolesanywhere-validation-refusals-render-patterns-and-enum-sets.md covers the CreateProfile pattern rendering and the CreateTrustAnchor PCA source check, not this ImportCrl reference check, and does not carry this replay key.
Traces: rolesanywhere-validation-error-paths#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 85 steps, 16 passed, score 18.8%).

Cause 1 — rolesanywhere-crl-lifecycle#task#2, ImportCrl, status 201 vs 201:
  $.crl.createdAt: expected '2026-09-06T05:29:00.561083027Z', got 1793358034.3732877
  $.crl.updatedAt: expected '2026-09-06T05:29:00.561083027Z', got 1793358034.3732877

Cause 2 — rolesanywhere-crl-lifecycle#task#3, GetCrl, status 200 vs 200:
  $.crl.createdAt: expected '2026-09-06T05:29:00.561083027Z', got 1793358034.3732877
  $.crl.updatedAt: expected '2026-09-06T05:29:00.561083027Z', got 1793358034.3732877
(the expected instants above are that recording's per-run values; the requirement is the ISO-8601 date-time string form)

Cause 3 — rolesanywhere-crl-lifecycle#teardown#10, GetCrl after DeleteCrl:
  status: expected 404, got 200
  $.message: expected "The item doesn't exist. arn: arn:aws:rolesanywhere:us-west-2:000000000000:crl/541a23d4-79b0-3172-9d88-7bf2a4bf799b ; aborting.", got None
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.rolesanywhere.service/', got None

Cause 4 — rolesanywhere-not-found-error-paths#task#3, GetCrl, status 404 vs 404:
  $.message: expected "The item doesn't exist. arn: arn:aws:rolesanywhere:us-west-2:000000000000:crl/aac7ac34-8e10-4cb8-ad84-794ff5571bd7 ; aborting.", got 'Crl not found'

Cause 5 — rolesanywhere-validation-error-paths#task#6, ImportCrl with an unknown trustAnchorArn:
  status: expected 404, got 201
  $.message: expected 'Unable to find Trust Anchor with ID 2521117b-c08f-42dc-a517-245874313ff8.', got None
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.rolesanywhere.service/', got None

Model citations: Smithy ImportCrl (POST /crls, 201; trustAnchorArn required, pattern ^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:trust-anchor.*)$), GetCrl (GET /crl/{crlId}, 200, errors [ResourceNotFoundException 404]), DeleteCrl (DELETE /crl/{crlId}, errors [AccessDeniedException 403, ResourceNotFoundException 404]), CrlDetail.createdAt/updatedAt (Timestamp, timestampFormat date-time, documented "ISO-8601 timestamp"), ResourceNotFoundException (client, httpError 404, member message).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it; confirm /__admin/health answers 200.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1/2/3: replay fixture rolesanywhere-crl-lifecycle. Step task#2 imports a CRL (ImportCrl with a valid trustAnchorArn) — inspect $.crl.createdAt/$.crl.updatedAt in the 201 body: they are JSON numbers, not ISO-8601 strings. Step task#3 (GetCrl on that crlId) shows the same. Step teardown#10 calls GetCrl after the CRL was deleted and gets 200 with the CRL body instead of 404 + x-amzn-errortype ResourceNotFoundException.
4. Cause 4: replay fixture rolesanywhere-not-found-error-paths step task#3 — GetCrl on an unknown crlId returns 404 but with message 'Crl not found' instead of "The item doesn't exist. arn: <crl arn> ; aborting.".
5. Cause 5: replay fixture rolesanywhere-validation-error-paths step task#6 — ImportCrl whose trustAnchorArn references a nonexistent trust anchor returns 201 and creates a CRL instead of 404 ResourceNotFoundException with message 'Unable to find Trust Anchor with ID <id>.'.
