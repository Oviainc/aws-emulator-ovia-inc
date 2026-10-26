Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: b314e0f3433a6118e8049d188afadfe9e3c07a46

# DisassociateUser skips the instance-authorization gate its siblings run, on the strength of a comment that says the handler is still a 501 scaffold

_Filed: 2026-10-26T04:11:47Z_
_Operation: DisassociateUser | Class: error-response_

> _Note: this issue was identified against the code base at commit `b314e0f3433a6118e8049d188afadfe9e3c07a46`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/service.rs` hoists this service's instance-authorization gate ahead of the boundary validator for `AssociateUser` only, and justifies leaving `DisassociateUser` ungated by asserting that `DisassociateUser` "answers the scaffold's 501 for every request". That premise is false: `src/ops/disassociate_user.rs` carries no `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and is a fully implemented handler that accepts a disassociation and answers 200 with an `InstanceUserSummary`.

As a result `DisassociateUser` is the only user-plane operation that never asks `authorizes_instance` at all: `AssociateUser` asks it (twice — in the handler and in the pre-validation seam) and `ListUserAssociations` asks it before anything else, both citing the same predicate and the same fail-closed rule. Because `DisassociateUser` reaches its fail-closed refusal only after the framework's deserialize -> validate step, a request this service would not authorize is NOT refused identically whatever else the request says: e.g. a request naming an instance the account's fleet does not hold together with an `InstanceUserArn` outside the `Arn` `@pattern` is answered as a pattern `ValidationException`, whereas the gate that `associate_user.rs` documents exists precisely so that such a call is refused with the fixed `UnauthorizedException` regardless of the request's shape.

The same stale "still a scaffold" premise is repeated in the test suites for operations that are now implemented, so the rationale is not a one-off typo: `tests/all/list_instances.rs:21-25` seeds `state::instances` directly rather than driving `AssociateUser` because that handler is "still a scaffold stub", and `tests/all/user_disassociation.rs:32-35` reads associations back through `DisassociateUser` itself because `ListUserAssociations` is "still a scaffolded 501".

## Evidence

Side 1 — `src/service.rs:155-157`, inside `override_operation`:
```
        // Only `AssociateUser` is gated. `DisassociateUser` answers the scaffold's 501 for
        // every request, so there is nothing yet for a gate in front of it to be
        // consistent with; its own issue owns it.
        if req.method() == "POST" && request_path(req.uri()) == ASSOCIATE_USER_URI {
```

Side 2 — `DisassociateUser` is implemented, not a 501 scaffold (`src/ops/disassociate_user.rs:3-7` and `:197-209`):
```
//! @op DisassociateUser
//!
//! Ends a user association: the disassociation is accepted and the association stays in
//! [`crate::state`] in the non-terminal `DISASSOCIATING` status until the teardown it
//! announces has completed.
...
        let record = state
            .begin_instance_user_disassociation(&arn, now)
...
        Ok(DisassociateUserOutput {
            instance_user_summary: crate::state::instance_user_summary(&record),
        })
```

Side 3 — the check the siblings run and this handler does not. `src/ops/associate_user.rs:191-196`:
```
        // The authorization gate, BEFORE any existence lookup of this service's own
        // resources: an instance the account's fleet does not hold cannot be shown to be
        // the caller's, so the request fails closed.
        if !state.authorizes_instance(&input.instance_id) {
            return Err(crate::scaffolded::Scaffolded::unauthorized());
        }
```
`src/ops/list_user_associations.rs:175-177`:
```
        if !state.authorizes_instance(&input.instance_id) {
            return Err(crate::scaffolded::Scaffolded::unauthorized());
        }
```
`grep -n authorizes_instance src/ops/disassociate_user.rs` → no match; the handler goes straight from the identifier rules to `resolve_instance_user` (`src/ops/disassociate_user.rs:185-195`).

Why the ordering matters, from `src/ops/associate_user.rs:8-16`: "this service refuses a call it will not authorize identically whether the request's `Tags` map is within the modelled `@length(max: 50)` or past it, and a handler cannot do that — the framework's centralized walk is deserialize -> validate -> handle".

Corroborating stale claims about now-implemented operations: `tests/all/list_instances.rs:21-25` ("whose `AssociateUser` handler is still a scaffold stub") and `tests/all/user_disassociation.rs:32-35` ("`ListUserAssociations` is still a scaffolded 501").

## Steps to Reproduce

1. Register an identity provider and associate a user so the user-association plane is live
2. POST /user/DisassociateUser with {"Username":"alice","InstanceId":"i-does-not-exist","IdentityProvider":{...},"InstanceUserArn":"not-an-arn"} — the instance is not in the account fleet AND the ARN violates the Arn @pattern
3. Observe a pattern ValidationException from the boundary validator, not the fixed UnauthorizedException this service answers for an instance it will not authorize; the same request shape against POST /user/AssociateUser is refused with UnauthorizedException by the pre-validation gate in src/service.rs:158-162
4. Read src/service.rs:155-157 and src/ops/disassociate_user.rs:3-7: the comment that justifies the missing gate states DisassociateUser "answers the scaffold's 501 for every request", while the handler is implemented and answers 200
