Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 2334cad17af0cf812822af0fde730e954dc43dac

# DeleteAccessor skips the malformed-AccessorId refusal GetAccessor makes, answering 404 for an id the service could never have issued

_Filed: 2026-10-27T15:43:10Z_
_Operation: DeleteAccessor | Class: error-response_

> _Note: this issue was identified against the code base at commit `2334cad17af0cf812822af0fde730e954dc43dac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate holds `AccessorId` to the accessor-id SPACE: a value outside it is refused with the modelled `InvalidRequestException` (400) naming the member and the value, BEFORE any lookup, and the rule is keyed on the INPUT MEMBER rather than on the operation that reads it. `GetAccessor` applies it. `DeleteAccessor` — the other operation that resolves that very member, as the member constant's own doc says — resolves the id straight against the store and answers the modelled `ResourceNotFoundException` (404) `Accessor <value> not found.` instead.

So the same value answers two different ways depending on which accessor operation reads it: `GET /accessors/not-a-real-id` -> 400 `InvalidRequestException` "AccessorId parameter not-a-real-id is malformed", while `DELETE /accessors/not-a-real-id` -> 404 `ResourceNotFoundException` "Accessor not-a-real-id not found." — which is exactly the distinction the crate says the two answers exist to draw ("you handed me a bad id" vs "that accessor is gone"). The boundary refusal does not cover it either: a value such as `not-a-real-id` sits inside `ResourceIdString`'s `@length(1, 32)`, so it raises no constraint violation and the handler is the only place that can refuse it.

## Evidence

The rule, and the member it is keyed on — src/state.rs:548-550:

```
/// The `GetAccessor` / `DeleteAccessor` input member that carries an accessor id, as the
/// model names it.
pub const MEMBER_ACCESSOR_ID: &str = "AccessorId";
```

and src/state.rs:2605-2609 (`id_space_of`): "Whether the values of input member `member` are drawn from a resource-id SPACE this service validates against ... `None` for a member with no space to test a value against, which is every member but [`MEMBER_ACCESSOR_ID`]"; src/state.rs:2632-2635: "The two places that can answer such a request — the service's boundary refusal and the handler that would look the id up — both word it from here, so they cannot disagree."

The sibling that applies it — src/ops/get_accessor.rs:157-168:

```
// An `AccessorId` outside the accessor-id space is an id this service could never
// have issued, so it is a request to FIX rather than a resource to look up: it is
// refused with the modelled `InvalidRequestException` (400) before any state is
// read, naming the member and the value the request held. A WELL-FORMED id
// nothing holds falls through to the `ResourceNotFoundException` below ...
if let Some(refusal) =
    crate::state::malformed_id(crate::state::MEMBER_ACCESSOR_ID, &input.accessor_id)
{
    return Err(GetAccessorErrorModeled::InvalidRequestException(refusal).into());
}
```

The handler that skips it — src/ops/delete_accessor.rs:164-173, which goes straight from `settle` to the store lookup with no `malformed_id` call anywhere in the file (grep for `malformed_id` in src/ matches only state.rs, service.rs and get_accessor.rs):

```
let Some(accessor) = guard.accessor_mut(&region, &input.accessor_id) else {
    return Err(DeleteAccessorErrorModeled::ResourceNotFoundException(
        crate::state::not_found(crate::state::RESOURCE_ACCESSOR, &input.accessor_id),
    )
    .into());
};
```

`DeleteAccessor` does model the shape the refusal needs: `InvalidRequestException (HTTP 400)` (OPERATIONS.md:66).

Test coverage: tests/all/not_found_and_malformed_ids.rs:16-21 states the rule as member-scoped ("An id outside the SPACE its input member's values are drawn from is refused with the modelled `InvalidRequestException` (400) naming that member and the value, before any lookup ... Only `AccessorId` is validated this way") and tests/all/not_found_and_malformed_ids.rs:407-409 states the generalisation for the sibling member ("`CreateMember` resolves an `InvitationId` of its own, and labels it the same way: the rule is about the member, not about the operation that reads it"). Every row of the malformed table at tests/all/not_found_and_malformed_ids.rs:295-338 is sent as `GET /accessors/{id}` only — no test sends a malformed id to `DELETE /accessors/{id}`, so nothing pins the current 404.

## Steps to Reproduce

1. Call `GET /accessors/not-a-real-id` -> 400 `InvalidRequestException`, Message "AccessorId parameter not-a-real-id is malformed" (the rule tests/all/not_found_and_malformed_ids.rs:287-338 pins).
2. Call `DELETE /accessors/not-a-real-id` -> 404 `ResourceNotFoundException`, Message "Accessor not-a-real-id not found."
3. Repeat with the other rows of the same table (a 36-character UUID is refused 400 at the boundary on both verbs; every value inside `@length(1, 32)` — `x`, `ac-`, `ac-` plus 25 characters, `ac-` lower-cased — diverges between the two verbs).
4. Expected (src/state.rs:548-550 and :2632-2635): both operations read the SAME input member through the same `malformed_id` wording, so an id outside the accessor-id space is the 400 on `DeleteAccessor` too, and only a WELL-FORMED id nothing holds is the 404.
