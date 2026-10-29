Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 9e55f581c86f2cc2a9694ee1b8e2f61584dc3f55

# ARN-addressed not-found message lifts a "tunnel id" out of a string that is not a tunnel ARN, against the helper's own documented domain

_Filed: 2026-10-29T07:16:38Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `9e55f581c86f2cc2a9694ee1b8e2f61584dc3f55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three ARN-addressed operations (`ListTagsForResource`, `TagResource`, `UntagResource`) build their modelled `ResourceNotFoundException` message through `state::tunnel_not_found_message_for_arn`, whose documented rule is: name the tunnel by the id lifted out of the ARN, and — when the supplied string "names no tunnel resource" and so "carries no id to lift" — name the supplied string itself. The lifting helper `state::tunnel_id_from_arn` is documented to answer `None` for anything "not of the form `arn:<partition>:iot:<region>:<account>:tunnel/<tunnelId>`", but its body validates none of that form: it takes everything after the fifth colon and only strips the `tunnel/` prefix. It never checks the literal `arn` prefix, the partition, or the `iot` namespace that the crate's own `TUNNEL_ARN_SERVICE` constant defines.

So any caller string with at least six colon-separated fields whose sixth field starts with `tunnel/` — e.g. `arn:aws:s3:us-east-1:000000000000:tunnel/abc`, `arn:aws:iotsecuredtunneling:us-east-1:0:tunnel/abc`, or even `a:b:c:d:e:tunnel/abc` — is treated as a tunnel ARN. The 404 then reports `Tunnel abc does not exist.`, inventing a tunnel id the request never named as a tunnel id, instead of naming the supplied string as the helper above it documents for exactly this case. The lifted fragment is also unconstrained by `TunnelId`'s modelled pattern, so arbitrary caller text is reflected back inside the sentence.

## Evidence

The documented domain and the body that does not enforce it, `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/state.rs:696-706`:

    /// The `tunnelId` a resource ARN names, or `None` when `arn` is not of the form
    /// `arn:<partition>:iot:<region>:<account>:tunnel/<tunnelId>`.
    ///
    /// `TunnelId`'s modelled pattern `^[a-zA-Z0-9_\-+=:]{1,128}$` admits `:`, so the
    /// resource segment is everything after the fifth colon rather than the sixth
    /// colon-separated field.
    pub fn tunnel_id_from_arn(arn: &str) -> Option<&str> {
        let resource = arn.splitn(6, ':').nth(5)?;
        let tunnel_id = resource.strip_prefix(TUNNEL_ARN_RESOURCE_PREFIX)?;
        (!tunnel_id.is_empty()).then_some(tunnel_id)
    }

The rule the caller above it states, `src/state.rs:720-728` — the other half of the contradiction:

    /// [`tunnel_not_found_message`] for a request that addressed the tunnel by ARN:
    /// the message still names the tunnel by its id, lifted out of `arn`.
    ///
    /// An ARN that names no tunnel resource carries no id to lift, so there the
    /// supplied string itself is the only identifier the request offered and the
    /// message names that.
    pub fn tunnel_not_found_message_for_arn(arn: &str) -> String {
        tunnel_not_found_message(tunnel_id_from_arn(arn).unwrap_or(arn))
    }

The namespace the crate itself says a tunnel ARN carries, `src/state.rs:205-210`: `/// The ARN service namespace of a tunnel: AWS IoT Secure Tunneling resources are addressed in the `iot` namespace (`arn:aws:iot:<region>:<account>:tunnel/<id>`) ... pub const TUNNEL_ARN_SERVICE: &str = "iot";` — `tunnel_id_from_arn` never compares against it.

The handler that answers with it, `src/ops/list_tags_for_resource.rs:140-154`:

    if store
        .tunnel_id_for_arn(&region, &input.resource_arn)
        .is_none()
    { ... message: Some(crate::state::tunnel_not_found_message_for_arn(
                        &input.resource_arn,
                    )), ... }

(`src/ops/tag_resource.rs:159-170` and `src/ops/untag_resource.rs:145-156` call the same helper.)

Test coverage: `tests/all/tunnel_refusal_rules.rs:489-534` (`a_not_found_refusal_on_an_arn_that_names_no_tunnel_still_refuses`) spans exactly three ARNs — `arn:aws:iot:{REGION}:{ACCOUNT}:thing/thing-1` (a different resource type, so the prefix strip fails), another account's well-formed tunnel ARN, and `just-a-string` (fewer than six fields) — and asserts only `message(&body).starts_with("Tunnel ") && ends_with("does not exist.")`. No row supplies a six-field string whose service namespace is not `iot` with a `tunnel/` resource segment, so this case is uncovered and the divergence from the documented domain is not caught.

## Steps to Reproduce

1. Mount the service on an `InMemoryEngine` and sign requests for one region (as `tests/all/tunnel_refusal_rules.rs` does).
2. Call `ListTagsForResource` with `{"resourceArn": "arn:aws:s3:us-east-1:000000000000:tunnel/abc"}` — a string that names no tunnel resource (wrong service namespace).
3. Observe HTTP 404 `ResourceNotFoundException` with `message: "Tunnel abc does not exist."`, i.e. an id lifted from a string the crate's own `tunnel_id_from_arn` doc says must answer `None` ("not of the form `arn:<partition>:iot:<region>:<account>:tunnel/<tunnelId>`"), instead of the supplied string that `tunnel_not_found_message_for_arn` documents for this case.
4. Repeat with `{"resourceArn": "a:b:c:d:e:tunnel/abc"}` — not an ARN at all, yet the same `Tunnel abc does not exist.` is returned; compare with `{"resourceArn": "just-a-string"}`, which correctly names the supplied string.
5. The same two calls against `TagResource` (`{"resourceArn": ..., "tags": [{"key":"env","value":"test"}]}`) and `UntagResource` (`{"resourceArn": ..., "tagKeys": ["env"]}`) produce the same invented id.
