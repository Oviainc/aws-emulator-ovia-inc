Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 45e969ed3c6a96399c623276452bb212f0ee3327

# The emulator's own store-ceiling refusal is worded two different ways: four creates hand-roll a message that reads as a real AWS quota, breaking the rule state::store_full states

_Filed: 2026-10-29T12:23:25Z_
_Operation: CreateQuote | Class: error-response_

> _Note: this issue was identified against the code base at commit `45e969ed3c6a96399c623276452bb212f0ee3327`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`MAX_RESOURCES` is the emulator's OWN in-memory entry ceiling, not an AWS quota, and this crate states exactly one rule for wording the refusal that ceiling produces: `crate::state::store_full`. Its doc fixes what the body must and must not say — it must name the emulator, the resource kind and the bound, "so the refusal cannot be mistaken for a statement about the caller's request or about a real AWS quota".

Three handlers obey it (`StartCapacityTask`, `StartConnection`, `CreateRenewal` all call `store_full(..)`). Four hand-roll their own `format!` instead — `CreateQuote`, `CreateSite`, `CreateOutpost`, `CreateOrder` — and the sentence they produce ("the number of quotes would exceed the limit of 5000") is precisely the thing `store_full`'s doc forbids: it reads as a statement about a real, published AWS quota on quotes, naming neither the emulator nor the fact that the bound is this process's memory. A caller (or a fixer reading the wire) cannot tell the emulator's own ceiling from a service quota.

The two handlers that face the identical premise — "models no quota error, so report the cap as a 400 `ValidationException`" — then disagree with each other, and the crate's own comment says they do not: `create_renewal.rs` asserts it reports the cap "exactly as `CreateQuote` reports its own", yet `CreateRenewal` emits the `store_full` sentence while `CreateQuote` emits the hand-rolled one. The same comment block in `create_quote.rs` has also drifted from the policy doc it points at: it says "the two creates that DO model the quota error", where `state.rs` says "the three creates that model `ServiceQuotaExceededException`" (CreateSite, CreateOutpost and CreateOrder all declare it).

So one rule has two implementations, the copies have already drifted, and four of them contradict the rule's own stated requirement.

## Evidence

The rule, services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:6193-6205:
```
/// How this service states that a store has reached [`MAX_RESOURCES`] and cannot
/// take another resource of that kind.
///
/// The ceiling is the emulator's own, so no AWS rule fixes this wording; what the
/// caller branches on is the modelled code and status. The message names the
/// emulator, the kind of resource and the bound, so the refusal cannot be mistaken
/// for a statement about the caller's request or about a real AWS quota.
pub fn store_full(resource: &str) -> String {
    format!(
        "this emulator holds the maximum of {MAX_RESOURCES} {resource} it can keep in \
         memory, so it cannot take another"
    )
}
```

The code that breaks it, src/ops/create_quote.rs:238-253:
```
// See `CreateSite`: the store's growth is capped. `CreateQuote` models no
// quota error, so the cap is reported as what it is from the caller's side —
// a request this service will not accept (400 `ValidationException`) — and
// never as a 501 or an invented error code. The wording is the one the two
// creates that DO model the quota error use.
if store.quotes.len() >= crate::state::MAX_RESOURCES {
    return Err(CreateQuoteErrorModeled::ValidationException(
        ValidationException {
            message: Some(format!(
                "the number of quotes would exceed the limit of {}",
                crate::state::MAX_RESOURCES
            )),
        },
    )
    .into());
}
```
The same hand-rolled sentence, three more times: src/ops/create_site.rs:167-177 (`"the number of sites would exceed the limit of {}"`), src/ops/create_outpost.rs:230-240 (`"the number of Outposts would exceed the limit of {}"`), src/ops/create_order.rs:347-357 (`"the number of orders would exceed the limit of {}"`).

The sibling that uses the helper, and whose comment claims the two agree — src/ops/create_renewal.rs:278-290:
```
// See `MAX_RESOURCES`: a renewal is never released, so what this contract
// grows is capped. `CreateRenewal` models no quota error, so the cap is
// reported as what it is from the caller's side — a request this service will
// not accept (400 `ValidationException`) — exactly as `CreateQuote` reports
// its own, and never as a 501 or an invented error code.
if store.renewals_held() >= crate::state::MAX_RESOURCES {
    return Err(CreateRenewalErrorModeled::ValidationException(
        ValidationException {
            message: Some(crate::state::store_full("renewal contracts")),
        },
    )
    .into());
}
```
and src/ops/start_capacity_task.rs:428-437 / src/ops/start_connection.rs:196-203, both `crate::state::store_full(..)`.

The miscount in the same CreateQuote comment, against src/state.rs:6180-6187:
```
/// Past the cap the creating operation refuses the request rather than growing:
/// the three creates that model `ServiceQuotaExceededException` (HTTP 402) answer it
```

Test coverage: no test pins any of these bodies — `grep -r store_full services/service-com-amazonaws-outposts-outpostsolafservice/tests` and a grep for "would exceed the limit" in `tests/` both return nothing, so the store-ceiling wording is untested on every path. (`tests/all/quote_store_bound.rs` exercises the CreateQuote ceiling's code and status, not this sentence.)

## Steps to Reproduce

1. Read src/state.rs:6193-6199: the crate's single statement of how the MAX_RESOURCES refusal must be worded, including that it must not be mistakable for "a real AWS quota".
2. Read src/ops/create_quote.rs:243-252, src/ops/create_site.rs:167-176, src/ops/create_outpost.rs:230-239 and src/ops/create_order.rs:347-356: each hand-rolls "the number of <kind> would exceed the limit of <N>" instead of calling store_full, producing exactly a published-quota-sounding sentence.
3. Read src/ops/create_renewal.rs:281-286: the comment says the refusal is worded "exactly as `CreateQuote` reports its own", but the code calls store_full("renewal contracts"), which renders a different sentence than CreateQuote's.
4. Drive it over the wire: fill the quote store to crate::state::MAX_RESOURCES with POST /quotes, then POST /quotes once more — the 400 ValidationException body reads "the number of quotes would exceed the limit of 5000"; fill the renewal store and POST /renewals once more — the 400 body reads "this emulator holds the maximum of 5000 renewal contracts it can keep in memory, so it cannot take another". Two bodies for one rule the crate says is stated once.
