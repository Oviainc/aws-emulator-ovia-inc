Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 69ff0d5dd7a30b3ec79ed56649baca6290c20cfe

# PutAttachmentRoutingPolicyLabel never reads its ClientToken, the one idempotency-token rule state.rs says every tokened operation of this service goes through

_Filed: 2026-10-29T13:52:46Z_
_Operation: PutAttachmentRoutingPolicyLabel | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `69ff0d5dd7a30b3ec79ed56649baca6290c20cfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutAttachmentRoutingPolicyLabel` models a `ClientToken` with `idempotencyToken: true` and a `ConflictException` (409), but the handler drops the member: it never normalises it (`crate::state::normalized_client_token`), never resolves it against stored state (`crate::state::replay_or_conflict`) and never stores it, so no record of this service remembers the token the label write carried.

The crate states the opposite rule in three places and implements it on the ten OTHER tokened operations (the nine creates and `PutCoreNetworkPolicy`): a non-empty token is resolved against the store, a retry of the SAME request is answered from the record it already produced, and the same token asking for something ELSE is refused `ConflictException` (409) naming that record and writing nothing.

Because this handler opts out, the same `ClientToken` replayed with a DIFFERENT `RoutingPolicyLabel` is answered 200 and silently overwrites the stored label, where every sibling tokened operation answers 409 for exactly that input class. The handler's own comment asserts that is how a caller "CHANGES the label rather than a request to refuse" — a rationale that contradicts the crate-wide rule state.rs and the create handlers state, and that nothing else in the crate (or any test) backs.

## Evidence

The model (smithy_query query="operation" operation="PutAttachmentRoutingPolicyLabel"):
```
"ClientToken": {"target": "ClientToken", "idempotencyToken": true, "length": {"min": 0, "max": 256}, "pattern": "^[\\s\\S]*$"}
...
{"shape": "ConflictException", "errorKind": "client", "httpError": 409}
```

The crate-wide rule — services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:8509-8510:
```
/// Every record of this service whose producing operation models an idempotency token:
/// each keeps it in a `client_token` field, so one implementation serves them all.
```
and src/ops/create_core_network.rs:211-217:
```
        // `ClientToken` is the operation's modelled `@idempotencyToken`, so a retry of the
        // same request answers the record the first call minted instead of minting a
        // second core network; the same token asking for something else is a conflict. The
        // walk itself — the token normalisation, the store lookup and the replay-or-refuse
        // branch — is [`crate::state::replay_or_conflict`], the one every tokened operation
        // of this service goes through
```

The handler that skips it — src/ops/put_attachment_routing_policy_label.rs:196-201:
```
        // `ClientToken` is this operation's modelled `@idempotencyToken` and needs no record
        // of its own: the whole effect of the call is the label it carries, so a retry under
        // the same token writes the same label and answers the same payload as the call it
        // repeats, while a later call carrying another label is how a caller CHANGES the
        // label rather than a request to refuse.
        record.routing_policy_label = Some(input.routing_policy_label);
```
(no `normalized_client_token`, no `replay_or_conflict`, nothing stored for the token — confirmed by grep: the only mention of `client_token` for this operation is the generated input field, src/ops/types/put_attachment_routing_policy_label_types.rs:20.)

Test coverage: tests/all/client_token_idempotency.rs:3-15 claims to span the whole class and then enumerates ten operations, leaving this one out:
```
//! End-to-end coverage of the ONE idempotency-token rule every operation of this service
//! that models `ClientToken` follows
...
//! The table below spans that class: one row per operation that models the token — the nine
//! creates and `PutCoreNetworkPolicy`
```
So no test pins this handler's divergent reading.

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateCoreNetwork with a policy document configuring an edge in us-west-2
2. CreateVpcAttachment on that core network (attachment-xxxx), wait for it to become AVAILABLE
3. POST /routing-policy-label with {"CoreNetworkId":"core-network-…","AttachmentId":"attachment-xxxx","RoutingPolicyLabel":"blue","ClientToken":"t1"} -> 200, RoutingPolicyLabel "blue"
4. POST /routing-policy-label again with the SAME "ClientToken":"t1" but "RoutingPolicyLabel":"red" -> 200 and the stored label becomes "red"; every other tokened operation of this crate answers ConflictException (409) for the same token asking for something else (crate::state::replay_or_conflict, src/ops/create_core_network.rs:219-243)
5. ListAttachmentRoutingPolicyAssociations for that core network -> reports "red", so the replayed token changed stored state instead of replaying the first call
