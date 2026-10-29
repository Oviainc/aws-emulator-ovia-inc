Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# RevokeSignature's hand-written Unit output schema states 204 in its comment but answers the protocol-default 200, unlike both sibling Unit-output operations

_Filed: 2026-10-29T18:05:42Z_
_Operation: RevokeSignature | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This model binds exactly three operations to `smithy.api#Unit` output — CancelSigningProfile, RevokeSignature and RevokeSigningProfile — and this crate treats them as ONE class: `src/service.rs` wraps all three in `StatedEmptyLength` so their bodyless answer states `content-length: 0`.

Two of the three hand-write their `Unit` output schema for the sole stated purpose of carrying a non-default success status, and both set 204. RevokeSignature hand-writes the very same schema, carries the very same comment — "Remove it and the status falls back to the protocol default, so a modelled 204 answers 200" — and then passes `Some(200u16)`, i.e. the protocol default itself. The override therefore achieves nothing: the file was written to stop a 204 degrading to 200 and sets 200, so the comment above the constant states a rule the code under it breaks, and the one operation of the three that answers a success status different from its siblings is the one whose own comment says it answers 204.

The secondary effect is on the wire: because the status is 200 and not 204, the `StatedEmptyLength` wrapper's `content-length: 0` survives on this answer (RFC 9110 §15.3.5 forbids it on a 204, and `src/service.rs` notes the HTTP/1.1 writer strips it there), so the bodyless answers of one class are framed two different ways.

The defect is the self-contradiction and the divergence from the sibling rule, not a claim about any recording: whichever status is right, the file's comment, the constant it passes and the two siblings cannot all be right at once.

## Evidence

Both sides are in this crate.

The contradiction inside one file — services/service-com-amazonaws-signer-wallabyservice/src/ops/revoke_signature.rs:87-98:
```
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
...
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "PUT",
        "/signing-jobs/{jobId}/revoke",
        Some(200u16),
    ));
```
and revoke_signature.rs:179: `const SUCCESS_STATUS: u16 = 200u16;`

The sibling rule — src/ops/cancel_signing_profile.rs:102-108:
```
/// The status a successful cancel answers with.
///
/// The model gives this `DELETE` an `@http` code of 200 with a `Unit` output, but
/// the RECORDED AWS answer is `204 No Content` — and the wire is what an SDK or a
/// client reads, so 204 is what this operation returns. A `Unit` output carries
/// no body either way, so 204 loses nothing.
pub const CANCEL_SIGNING_PROFILE_SUCCESS_STATUS: u16 = 204;
```
and src/ops/revoke_signing_profile.rs:116-122, word for word the same reasoning: "the RECORDED AWS answer is `204 No Content` ... A `Unit` output carries no body either way, so 204 loses nothing." / `pub const REVOKE_SIGNING_PROFILE_SUCCESS_STATUS: u16 = 204;`

The three are one class in this crate — src/service.rs:79-88: "The operations whose modelled output carries NO body, and whose answers therefore state [`EMPTY_BODY_LENGTH`] ... `CancelSigningProfile`, `RevokeSignature` and `RevokeSigningProfile` are the ones whose `output` is `Unit`", and src/service.rs:411-417 adds exactly those three to `empty_answers`.

Model: `smithy_query(query="operation", operation="RevokeSignature")` gives `"output_shape": "Unit"`, `"http": {"method": "PUT", "uri": "/signing-jobs/{jobId}/revoke", "code": 200}` — the same shape and route code the two 204 siblings have (`CancelSigningProfile` / `RevokeSigningProfile`: `"output_shape": "Unit"`, `"code": 200`), so the model does not distinguish them.

Test coverage: tests/all/signature_revocation.rs:322-326 asserts `revoked.status, 200` for RevokeSignature with NO stated reason for the value (the module header, lines 3-28, lists the rules under test and says nothing about the status), while tests/all/revocation_survives_a_reput.rs:319-322 asserts `revoked.status, 204` for RevokeSigningProfile ("expected RevokeSigningProfile to answer 204") and tests/all/signing_profile_lifecycle.rs:911 asserts 204 for CancelSigningProfile.

## Steps to Reproduce

1. PUT /signing-profiles/Revoke_Status_Profile with {"platformId": "AWSLambda-SHA384-ECDSA"} (SigV4-signed for us-west-2).
2. POST /signing-jobs with an s3 source/destination, profileName Revoke_Status_Profile and a fresh clientRequestToken; keep the returned jobId.
3. PUT /signing-jobs/{jobId}/revoke with {"reason": "key compromised"} and observe HTTP 200 with an empty body and `content-length: 0`.
4. PUT /signing-profiles/Revoke_Status_Profile/revoke with {"profileVersion": <version>, "reason": "key compromised", "effectiveTime": <now>} and observe HTTP 204 — the same `Unit` output class answered with a different status.
5. Read src/ops/revoke_signature.rs:87-98: the comment says the `with_http` override exists so "a modelled 204" does not fall back to 200, while the value passed is `Some(200u16)`, the protocol default the override is there to avoid.
