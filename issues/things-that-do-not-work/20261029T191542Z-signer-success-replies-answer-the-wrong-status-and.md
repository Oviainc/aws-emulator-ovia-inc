Issue-Source: trace-sweep-divergence
Issue-Trace: signer-lambda-profile-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: signer-lambda-profile-lifecycle signer-cross-account-profile-permissions signer-error-paths-jobs-and-permissions signer-profile-revocation-workflow
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer|TagResource|2xx->2xx|status
Issue-Key: signer|UntagResource|2xx->2xx|status
Issue-Key: signer|ListProfilePermissions|2xx->2xx|$.policySizeBytes
Issue-Key: signer|RemoveProfilePermission|2xx->2xx|$
Issue-Key: signer|RevokeSigningProfile|2xx->2xx|$headers.content-length
Issue-Commit: f04d881c92d4dd0b362060ab10db1f8e619a8cb7

# Signer success replies answer the wrong status and the wrong response envelope

> _Note: this issue was identified against the code base at commit `f04d881c92d4dd0b362060ab10db1f8e619a8cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Five net-new wire divergences share one root cause: on paths that succeed, the emulator's response envelope is not AWS's — wrong HTTP status on the no-content operations, a member AWS does not return, a missing framing header, and a computed size that measures the wrong document. All five are on service signer and must all be closed before this issue is submitted.

Cause 1 of 5: TagResource answers 200 instead of 204 (fixture signer-lambda-profile-lifecycle, step task#4).
AWS: recorded behavior only for the status — the Smithy model carries no @httpResponseCode on TagResource, so the generated default is 200, but the recorded AWS reply is 204 with no body (TagResourceResponse has no members). The requirement is the 204 no-content status, not the protocol default.
Emulator: 200.
Why: a client or SDK that treats 204 as the success signal, or that refuses to parse a body-less 200 as JSON, mis-handles a tag write that in fact succeeded.
Fix: the TagResource response serializer's status code. Note the open issue 20261029T180542Z-signer-revokesignature-wrong-behavior.md records the same 200-instead-of-204 shape for RevokeSignature's hand-written Unit output; it carries neither of these fixtures' replay keys and does not cover the tagging operations, so this cause is net-new.

Cause 2 of 5: UntagResource answers 200 instead of 204 (fixture signer-lambda-profile-lifecycle, step task#6).
AWS: recorded behavior only — recorded reply is 204 with no body (UntagResourceResponse has no members; no @httpResponseCode in the model).
Emulator: 200.
Why/Fix: as cause 1, in the UntagResource response serializer. The two tagging writers share the defect, so the status belongs in whatever helper serializes signer's no-content outputs.

Cause 3 of 5: RemoveProfilePermission returns a revisionId AWS does not return (fixture signer-cross-account-profile-permissions, step task#4).
AWS: RemoveProfilePermissionResponse models an optional `revisionId` String, and the recorded success reply is the empty object {} — the service does not hand back a revision id when a statement is removed.
Emulator: 200 with {'revisionId': '00000000-0000-0002-0000-000000000002'} — a synthetic sequential counter value, which also leaks the emulator's id-minting scheme.
Why: a client that reads revisionId from the removal reply and feeds it to the next RemoveProfilePermission/AddProfilePermission is chaining a revision id AWS never issues; against AWS that field is absent and the caller must re-read the policy, which is the behavior the emulator hides.
Fix: the RemoveProfilePermission output serializer — do not populate revisionId on removal. (The same cluster also covers signer-error-paths-jobs-and-permissions#task#6.)

Cause 4 of 5: ListProfilePermissions reports the wrong policySizeBytes (fixture signer-cross-account-profile-permissions, step task#3).
AWS: policySizeBytes (Smithy: PolicySizeBytes integer, default 0) is the byte length of the profile's resource policy document as the service stores it; in the recorded read of a one-statement policy it is 251.
Emulator: 251 is reported as 270 — it measures a differently formatted/decorated policy document, so the number is consistently off rather than off by a per-run amount. The requirement is that the value equal the length of the canonical policy document the service would serve; 251 and 270 are this fixture's values, not constants to hard-code.
Why: callers compare policySizeBytes against the resource-policy size quota to decide whether another statement fits, so an inflated figure makes them stop adding statements early (or, with a different policy shape, overshoot).
Fix: the policy-size computation behind ListProfilePermissions — serialize the stored policy canonically (the same form AWS measures) and take the length of that.

Cause 5 of 5: RevokeSigningProfile's 204 omits content-length (fixture signer-profile-revocation-workflow, step task#4).
AWS: the recorded 204 reply carries a content-length header (its value is not asserted, only its presence).
Emulator: answers 204 with no content-length header at all.
Why: HTTP clients and proxies that require an explicit framing header on a response with no transfer-encoding mis-read or stall on the reply, even though the revoke itself succeeded.
Fix: the response serializer for signer's Unit-output revoke path — emit the framing header on no-content replies.

Traces: signer-lambda-profile-lifecycle#task#4 (representative), signer-lambda-profile-lifecycle#task#6, signer-cross-account-profile-permissions#task#3, signer-cross-account-profile-permissions#task#4, signer-profile-revocation-workflow#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline.

Cause 1 — TagResource, signer-lambda-profile-lifecycle#task#4: status expected 204, got 200.
Cause 2 — UntagResource, signer-lambda-profile-lifecycle#task#6: status expected 204, got 200.
Cause 3 — RemoveProfilePermission, signer-cross-account-profile-permissions#task#4, status 200/200:
  $: expected {}, got {'revisionId': '00000000-0000-0002-0000-000000000002'}
  Also affected: signer-error-paths-jobs-and-permissions#task#6 (same cluster)
Cause 4 — ListProfilePermissions, signer-cross-account-profile-permissions#task#3, status 200/200:
  $.policySizeBytes: expected 251, got 270
Cause 5 — RevokeSigningProfile, signer-profile-revocation-workflow#task#4, status 204/204:
  $headers.content-length: expected present (shaped, not asserted), got None

Model facts used: Smithy signer (restJson1) — TagResource POST /tags/{resourceArn} output {} with no @httpResponseCode (default 200); UntagResource DELETE /tags/{resourceArn} output {}; RemoveProfilePermission DELETE /signing-profiles/{profileName}/permissions/{statementId} output {revisionId: String}; ListProfilePermissions output {revisionId, policySizeBytes: PolicySizeBytes default 0, permissions, nextToken}; RevokeSigningProfile PUT /signing-profiles/{profileName}/revoke with Unit output.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Causes 1 and 2: replay signer-lambda-profile-lifecycle steps task#4 and task#6 — TagResource then UntagResource on the created profile's ARN; observe the HTTP status (expect 204).
4. Cause 3: replay signer-cross-account-profile-permissions step task#4 — RemoveProfilePermission with the current revisionId; observe the response body (expect {}).
5. Cause 4: same fixture, step task#3 — ListProfilePermissions on the profile holding one statement; compare $.policySizeBytes against the byte length of the canonical policy document.
6. Cause 5: replay signer-profile-revocation-workflow step task#4 — RevokeSigningProfile on an active profile version; inspect the 204 reply's headers for content-length.
