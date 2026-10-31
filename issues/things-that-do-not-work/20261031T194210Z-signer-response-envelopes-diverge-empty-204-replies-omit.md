Issue-Source: trace-sweep-divergence
Issue-Trace: signer-lambda-profile-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: signer-lambda-profile-lifecycle signer-profile-revocation-workflow signer-cross-account-profile-permissions
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer|TagResource|2xx->2xx|$headers.content-length
Issue-Key: signer|UntagResource|2xx->2xx|$headers.content-length
Issue-Key: signer|RevokeSigningProfile|2xx->2xx|$headers.content-length
Issue-Key: signer|ListProfilePermissions|2xx->2xx|$.policySizeBytes
Issue-Commit: aede39749e16b7ea04942c6f83e8cac2192d58fe

# Signer response envelopes diverge: empty 204 replies omit Content-Length and ListProfilePermissions miscomputes policySizeBytes

> _Note: this issue was identified against the code base at commit `aede39749e16b7ea04942c6f83e8cac2192d58fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Four net-new wire divergences in the signer crate's response-envelope construction. Causes 1-3 are the same framing fault on every empty-body reply (the Content-Length: 0 header AWS sends with a 204 is never emitted); cause 4 is the byte-size field of a success body being computed from something other than the policy document the service returns. All four replay against signer, whose bootstrap issue (20261021T223324Z-bootstrap-service-com-amazonaws-signer-wallabyservice.md) is resolved, and none is covered by an open issue: the only neighbouring open one, 20261029T180542Z-signer-revokesignature-wrong-behavior.md, is about RevokeSignature answering status 200 instead of 204 and says nothing about response headers or about ListProfilePermissions.

Cause 1 of 4: TagResource empty reply omits Content-Length.
AWS: Smithy signer#TagResource is POST /tags/{resourceArn} with an empty output shape (TagResourceResponse has no members), and AWS answers it with a bodyless response that still carries the entity header Content-Length: 0. Recorded behavior: status 204 with header content-length: '0'.
Emulator: answers status 204 (matching) but sends no content-length header at all ($headers.content-length missing, got None).
Why: a client or proxy that frames the response by Content-Length rather than by the status code has no terminator for the message and must fall back on connection close or on treating the reply as indeterminate-length; strict HTTP intermediaries and SDK transports that assert on the header see a malformed empty reply, so a tagging call that AWS reports as cleanly complete can hang or be reported as a transport error.
Fix: the operation's response serializer for empty/Unit output shapes in the signer crate — the same place that chooses status 204 must set Content-Length: 0 on a bodyless reply.
Traces: signer-lambda-profile-lifecycle#task#4.

Cause 2 of 4: UntagResource empty reply omits Content-Length.
AWS: Smithy signer#UntagResource is DELETE /tags/{resourceArn} with an empty output shape (UntagResourceResponse has no members); AWS answers bodyless with Content-Length: 0. Recorded behavior: status 204 with header content-length: '0'.
Emulator: status 204 (matching) with no content-length header ($headers.content-length missing, got None).
Why: same as cause 1 — an untag that succeeded is framed differently from AWS, so length-framing clients cannot tell where the reply ends.
Fix: same empty-output response serializer in the signer crate; UntagResource shares it with TagResource.
Traces: signer-lambda-profile-lifecycle#task#6.

Cause 3 of 4: RevokeSigningProfile empty reply omits Content-Length.
AWS: Smithy signer#RevokeSigningProfile is PUT /signing-profiles/{profileName}/revoke with output shape Unit (no members); AWS answers bodyless and sets Content-Length. Recorded behavior: status 204 with a content-length header present (its value is shape-asserted, not value-asserted, so the requirement is that the header is emitted on a bodyless 204, not any one number).
Emulator: status 204 (matching) with no content-length header at all ($headers.content-length missing, got None).
Why: same framing gap on the revocation write path, which is the one signer operation whose only success signal IS the empty reply — a client that cannot frame it cannot confirm the revocation landed.
Fix: same empty-output/Unit response serializer in the signer crate.
Traces: signer-profile-revocation-workflow#task#4.

Cause 4 of 4: ListProfilePermissions reports a policySizeBytes larger than the policy it returns.
AWS: Smithy signer#ListProfilePermissions (GET /signing-profiles/{profileName}/permissions, 200) returns policySizeBytes, documented as 'Total size of the policy associated with the Signing Profile in bytes' (member doc on ListProfilePermissionsResponse.policySizeBytes, target PolicySizeBytes, integer, default 0). The requirement is that the number equals the byte length of the resource policy document the service holds for that profile — the same document whose statements are returned in the permissions list — so it must move only when the statements do, and must agree with what the permissions in the same response describe.
Emulator: for a response whose revisionId and permissions list match the recording exactly, it reports policySizeBytes 270 where AWS reports 251 — a constant-looking surplus consistent with sizing a differently serialized document (extra whitespace/pretty-printing, or extra envelope members such as a version/statement wrapper) rather than the policy AWS accounts for.
Why: callers use policySizeBytes to decide whether another AddProfilePermission statement still fits under the profile's policy-size budget. An inflated figure makes a profile look closer to (or past) its limit than it is, so clients stop adding permissions that AWS would accept; a figure that disagrees with the returned statements also breaks any client that recomputes the size locally to detect drift.
Fix: the policySizeBytes computation feeding the ListProfilePermissions handler's output — it must measure the stored policy document exactly as it is serialized for the caller, not a re-rendered copy.
Traces: signer-cross-account-profile-permissions#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200, data listener on :33067); 49 steps, 39 passed, 6 divergence clusters.

Cause 1 — fixture signer-lambda-profile-lifecycle, step task#4, service signer, operation TagResource, verdict mismatch:
  expected status 204, actual status 204
  detail: $headers.content-length: missing (expected '0', got None)

Cause 2 — fixture signer-lambda-profile-lifecycle, step task#6, service signer, operation UntagResource, verdict mismatch:
  expected status 204, actual status 204
  detail: $headers.content-length: missing (expected '0', got None)

Cause 3 — fixture signer-profile-revocation-workflow, step task#4, service signer, operation RevokeSigningProfile, verdict mismatch:
  expected status 204, actual status 204
  detail: $headers.content-length: missing (expected '<shaped-not-asserted>', got None)

Cause 4 — fixture signer-cross-account-profile-permissions, step task#3, service signer, operation ListProfilePermissions, verdict mismatch:
  expected status 200, actual status 200
  detail: $.policySizeBytes: value_mismatch (expected 251, got 270)
  (every other asserted member of the body, including revisionId and the permissions list, matched)

Smithy consulted: signer#TagResource (POST /tags/{resourceArn}, output TagResourceResponse with no members), signer#UntagResource (DELETE /tags/{resourceArn}, output UntagResourceResponse with no members), signer#RevokeSigningProfile (PUT /signing-profiles/{profileName}/revoke, output Unit), signer#ListProfilePermissions (GET /signing-profiles/{profileName}/permissions, 200; output member policySizeBytes -> PolicySizeBytes integer default 0, doc: 'Total size of the policy associated with the Signing Profile in bytes').

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: fixture signer-lambda-profile-lifecycle, step task#4 — create a signing profile, then call TagResource on its ARN with a valid tag map. Observe the 204 reply carries no content-length header; AWS sends content-length: 0.
4. Cause 2: same fixture, step task#6 — call UntagResource on that ARN with ?tagKeys=<key>. Observe the 204 reply again carries no content-length header.
5. Cause 3: fixture signer-profile-revocation-workflow, step task#4 — PUT /signing-profiles/{profileName}/revoke with profileVersion, reason and effectiveTime for an existing active profile. Observe the 204 reply carries no content-length header.
6. Cause 4: fixture signer-cross-account-profile-permissions, step task#3 — add the cross-account profile permissions the fixture adds, then GET /signing-profiles/{profileName}/permissions. The permissions list and revisionId match AWS, but policySizeBytes is reported as 270 where the recording has 251 for the same policy.
