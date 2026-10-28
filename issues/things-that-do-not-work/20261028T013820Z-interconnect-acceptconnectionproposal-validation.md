Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c550c8d329f766442e2f60b5ca19de7c84eb66f9

# AcceptConnectionProposal accepts an attachPoint.arn from another Region, contradicting its own region-scope rule

_Filed: 2026-10-28T01:38:20Z_
_Operation: AcceptConnectionProposal | Class: validation_

> _Note: this issue was identified against the code base at commit `c550c8d329f766442e2f60b5ca19de7c84eb66f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AcceptConnectionProposal`'s store judgement documents the same region-scope rule as `CreateConnection` and names the attach-point ARN explicitly, but the `arn` branch resolves through `crate::state::connection_by_arn(arn)` without the request's Region. Because that helper only compares the supplied ARN text against the stored connection's own ARN, an ARN naming a live connection created through ANOTHER Region's endpoint resolves here, and the acceptance associates the connection with an attach point that no other operation of this endpoint can see. The `region` argument the function takes is used for the activation-key lookup only; the ARN branch ignores it.

The three sibling ARN-identified handlers of this crate refuse an out-of-scope ARN with `crate::state::arn_scope_violation(&parsed, &crate::state::request_region(ctx))` before resolving anything. This handler, which takes the same `AmazonResourceName` shape in `AttachPoint$arn`, never calls that helper.

## Evidence

Handler doc states the rule — services/service-com-amazonaws-interconnect-interconnect/src/ops/accept_connection_proposal.rs:151-153:
```
/// `region` is the Region the request addressed, which is the scope the proposal is
/// resolved in: a connection belongs to the Region it was created in, so neither an
/// activation key nor an attach-point ARN of another Region resolves here.
```
The code under it breaks it — accept_connection_proposal.rs:255-256 (no `region`):
```
    if let AttachPoint::Arn(arn) = &input.attach_point {
        if store.connection_by_arn(arn).is_none() {
```
The same file's inline comment even claims the stronger rule at accept_connection_proposal.rs:250-254: "in full, so an ARN whose Region or account is not the stored one names no resource this account holds ... The same rule the create path applies" — and the stored one IS the other Region's, so the check passes.

The resolver is unscoped — src/state.rs:441-443:
```
    pub fn connection_by_arn(&self, arn: &str) -> Option<&StoredConnection> {
        let id = connection_id_from_arn(arn)?;
        self.connections.get(id).filter(|stored| holds_arn(stored, arn))
```
against the crate's single scoping rule, src/state.rs:212-214: "ONE predicate, so every resolution path below scopes the store the same way."

Sibling handler applying the missing check — src/ops/list_tags_for_resource.rs:182-184:
```
        if let Some(message) =
            crate::state::arn_scope_violation(&parsed, &crate::state::request_region(ctx))
        {
```

Test coverage: tests/all/accept_connection_proposal.rs drives the acceptance per-Region but no row supplies an attach-point ARN belonging to a different Region's connection; tests/all/arn_scope_is_relative_to_the_request.rs:21 scopes its table to `ListTagsForResource,TagResource,UntagResource` only.

## Steps to Reproduce

1. Create a connection through the us-east-1 endpoint and note its `arn` (it will be arn:aws:interconnect:us-east-1:<account>:connection/<id>).
2. Create a second connection through the eu-west-1 endpoint and note its `activationKey` (state `requested`, i.e. a proposal awaiting acceptance).
3. Call AcceptConnectionProposal signed for eu-west-1 with that activationKey and `attachPoint: { "arn": "<the us-east-1 ARN from step 1>" }`.
4. Observe HTTP 200: the out-of-Region ARN resolved and the connection is now associated with it, while ListTagsForResource in eu-west-1 refuses that same ARN with HTTP 400 InterconnectValidationException.
5. Expected, per accept_connection_proposal.rs:151-153: an attach-point ARN of another Region does not resolve here.
