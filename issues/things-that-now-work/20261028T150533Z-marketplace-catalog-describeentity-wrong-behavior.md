Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8668e0a390f031d37f864cf13043544a41d29fed

# DescribeEntity can report a `Details` string longer than the modelled `len 2..16384`

_Filed: 2026-10-28T15:05:33Z_
_Operation: DescribeEntity | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8668e0a390f031d37f864cf13043544a41d29fed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeEntity` serialises the WHOLE accumulated entity details object into the output member `Details`, whose own modelled contract (quoted in the handler's header) is `len 2..16384`. Nothing on the write path or the read path keeps that value inside the modelled maximum:

* `StartChangeSet` accepts `Change$DetailsDocument`, a Smithy `document` with no length constraint at all; it checks only nesting DEPTH (`MAX_DETAILS_DEPTH`), that the value is an object, and the number of `DeliveryOptions` entries. A single modelled-valid request can therefore submit a details document of tens of kilobytes.
* `apply_changes` merges every submitted member into the stored entity details (`held.insert(key, value)`), so the stored object also GROWS across successive change sets even when each change's `Details` string stays inside `Change$Details`'s own `@length(max: 16384)`.
* `DescribeEntity` then answers `details: Some(entity.details_json())`, i.e. `self.details.to_string()` — the full object, untruncated.

The result is a 200 whose `Details` member is outside the length its own modelled shape declares, so a client that validates the response against the model cannot read it. This is the one reported value of the crate that is NOT kept inside its modelled length: the crate's other report paths explicitly cut a value for exactly this reason (`scope_text`, `quoted`), and the details path bounds only DEPTH, with the handler comment claiming only that "The conversion is bounded" (which is about depth, not size).

## Evidence

The modelled contract of the member, from the handler's own header — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/describe_entity.rs:56`:
```
//! * `Details` (Json, len 2..16384, pattern ^[\s]*\{[\s\S]*\}[\s]*$) — This stringified JSON
//!   object includes the details of the entity.
```
(same bound in `OPERATIONS.md:66`: "`Details` (string, len 2..16384, pattern ...)").

What the handler emits — `src/ops/describe_entity.rs:208`-`212`:
```
            // `Details` and `DetailsDocument` are the same details, as a JSON
            // string and as a document. The conversion is bounded, and `None`
            // is unreachable: nothing deeper than the stored bound was accepted.
            details: Some(entity.details_json()),
```
and `src/state.rs:829`-`832`:
```
    /// The accumulated details as the `Json` string `Details` is modelled as.
    pub fn details_json(&self) -> String {
        self.details.to_string()
    }
```

The only bound the crate puts on a details value is DEPTH — `src/state.rs:2791`-`2799`:
```
/// The deepest `Details` / `DetailsDocument` this service accepts.
///
/// Neither `Json` nor `JsonDocumentType` bounds nesting, but converting, storing
/// and reporting a details value walks it once per level, so a value nested
/// without limit would exhaust the stack ...
pub const MAX_DETAILS_DEPTH: usize = 64;
```

The accumulation that grows the stored object — `src/state.rs:1740`-`1744` (`apply_changes`):
```
                    if let Value::Object(held) = &mut entry.details {
                        for (key, value) in members {
                            held.insert(key, value);
                        }
                    }
```

The sibling rule this path skips — `src/state.rs:2131`-`2141`:
```
/// `text` as an `ErrorScope$Value` carries it: cut at [`SCOPE_VALUE_LIMIT`]
/// characters so it stays inside `ScopeValue`'s modelled length ...
fn scope_text(text: &str) -> String {
```
and `src/state.rs:2476`-`2478`: "cut at [`DETAIL_VALUE_QUOTE_LIMIT`] characters so the message built around it stays inside `ExceptionMessageContent`'s 2048."

Test coverage: no test pins the length of the reported `Details` — a grep for `16384` across `tests/` matches only `tests/all/assessment_production.rs:839`, which mentions `Change$Details`'s own input `@length(max: 16384)` while deliberately keeping the submitted document small ("The options here are empty objects, so a count far past the bound still fits inside `Change$Details`'s own `@length(max: 16384)`"). `tests/all/entity_reads.rs` and `tests/all/change_set_lifecycle.rs` only compare `Details`/`DetailsDocument` to what was submitted.

## Steps to Reproduce

1. POST /StartChangeSet with Catalog=AWSMarketplace and ChangeSet=[{ChangeType:"CreateProduct", Entity:{Type:"AmiProduct"}, DetailsDocument:{"ProductTitle":"big", "m0":"<30 chars>", "m1":"<30 chars>", ... "m999":"<30 chars>"}}] — a flat object of ~1000 members, which is modelled-valid (JsonDocumentType carries no @length, nesting depth is 1, no DeliveryOptions) — and observe 200.
2. Wait for the change set to reach SUCCEEDED (CHANGE_SET_PREPARING + CHANGE_SET_APPLYING).
3. Read the created entity id from ListChangeSets' EntityIdList, then GET /DescribeEntity?catalog=AWSMarketplace&entityId=<id>.
4. Measure the returned `/Details` string: it is ~30 KB, i.e. far beyond the `len 2..16384` the member's own modelled contract declares (describe_entity.rs:56).
5. Alternative path with no oversized single request: run three successive StartChangeSet calls against the SAME entity, each submitting ~8 KB of DISTINCT Details members (each inside Change$Details' @length(max: 16384)); after each succeeds, DescribeEntity's `Details` grows past 16384 because apply_changes merges the members into the stored object.
