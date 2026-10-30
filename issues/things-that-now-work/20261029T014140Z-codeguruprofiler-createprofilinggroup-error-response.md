Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 7b0476a706ac79cc0f7894740758ad8ab457f053

# Handlers build their 400 from the synthetic smithy.framework ValidationException although the operation models its own, against this crate's stated rule

_Filed: 2026-10-29T01:41:40Z_
_Operation: CreateProfilingGroup | Class: error-response_

> _Note: this issue was identified against the code base at commit `7b0476a706ac79cc0f7894740758ad8ab457f053`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::scaffolded` states, twice, that the synthetic `smithy.framework#ValidationException` is for operations whose model does NOT declare a client-input error, and that an operation which lists its own `ValidationException` must use the modelled variant on its generated error enum. Two handlers break that rule while declaring `ValidationException` in their own modelled error enum:

* `CreateProfilingGroup` refuses an out-of-catalogue `computePlatform` through `crate::scaffolded::Scaffolded::validation(..)` (the synthetic shape) and, nine lines later, refuses a bad `tags` map through `CreateProfilingGroupErrorModeled::ValidationException(..)` (the modelled shape) — one handler, one logical refusal class, two different error shapes.
* `GetProfile` builds all four of its input refusals (both companion-member refusals, the window read and the window-length rule) from the synthetic shape, although `GetProfileErrorModeled` declares `ValidationException` too.

Today the two paths render the same bytes — `Scaffolded::shape_name()` returns the literal `"ValidationException"`, the modelled shape carries only `message`, and restJson1 writes `__type` from that code — so this is a correctness-of-construction defect with no behavioural impact today. It is still a defect: the error travelling through the handler is a shape that is not in this service's model, so it cannot carry a member the modelled shape grows, and the choice is already inconsistent inside a single handler.

## Evidence

The rule, in this crate — src/scaffolded.rs:130-135:
```
/// Schema for the synthetic `ValidationException` error shape.
///
/// In the `smithy.framework` namespace for the same reason as `NotImplemented`: the
/// shape is not in any service's model, so it must not collide with a modeled one.
/// If THIS service's model declares its own `ValidationException`, prefer that —
/// see [`Scaffolded::validation`].
```
and src/scaffolded.rs:345-350:
```
    /// If this service's model declares its own `ValidationException` (or another
    /// client-input error) use THAT: the modeled shape may carry members beyond
    /// `message` — a `reason`, a field list — and a real client reads them. When the
    /// operation already lists it, it is a plain variant on the generated error enum;
```

The operation DOES declare it — src/ops/create_profiling_group.rs:99-105:
```
pub enum CreateProfilingGroupErrorModeled {
    ConflictException(ConflictException),
    ...
    ValidationException(ValidationException),
}
```

Both channels in ONE handler — src/ops/create_profiling_group.rs:171-188:
```
            Some(value) => {
                return Err(crate::scaffolded::Scaffolded::validation(format!(
                    "computePlatform must be one of {}, got {value}",
...
        if let Some(tags) = input.tags.as_ref() {
            crate::state::validate_tags("tags", tags).map_err(|message| {
                CreateProfilingGroupErrorModeled::ValidationException(ValidationException {
                    message,
                })
            })?;
```

The same pattern in GetProfile, whose enum also lists ValidationException (src/ops/get_profile.rs:738-743) — src/ops/get_profile.rs:196, 201, 213, 221:
```
            return Err(crate::scaffolded::Scaffolded::validation(
                companion_required_message("startTime", "endTime"),
...
    .map_err(crate::scaffolded::Scaffolded::validation)?;   // requested_window
    .map_err(crate::scaffolded::Scaffolded::validation)?;   // validate_window_length
```

Why no response changes: vendor/smithy-server-runtime/src/operation.rs:445 names the wire code `_ => e.shape_name()`, and src/scaffolded.rs:449 returns `"ValidationException"` for the synthetic variant, while the modelled shape has only `message` (`smithy_query(query="shape", shape="ValidationException")` -> `{"members": {"message": {"target": "String", "required": true}}, "httpError": 400}`). Tests that pin these refusals and must stay green: tests/all/profiling_group_lifecycle.rs, tests/all/get_profile_interval.rs:337, tests/all/get_profile_window_companion.rs:398, tests/all/get_profile_window_length.rs:420 (all assert code `ValidationException` with HTTP 400).

## Steps to Reproduce

1. Read src/scaffolded.rs:130-135 and :345-350 — the rule is that an operation listing its own ValidationException must use the modelled variant.
2. Open src/ops/create_profiling_group.rs:171-188: the computePlatform refusal is built with `Scaffolded::validation` (the `smithy.framework#ValidationException` shape, src/scaffolded.rs:136-144) while the tags refusal nine lines below is built with `CreateProfilingGroupErrorModeled::ValidationException`, the shape the model declares.
3. Open src/ops/get_profile.rs:196, 201, 213, 221 — all four input refusals use the synthetic shape although src/ops/get_profile.rs:742 declares the modelled one.
4. Replace the five `Scaffolded::validation` call sites with the operations' own modelled `ValidationException` variants and re-run tests/all/get_profile_interval.rs, get_profile_window_companion.rs, get_profile_window_length.rs and profiling_group_lifecycle.rs: the status, the `__type` and the message are unchanged.
