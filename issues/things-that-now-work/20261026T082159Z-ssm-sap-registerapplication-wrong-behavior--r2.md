Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# RegisterApplication writes its Tags with a raw map insert instead of the crate's apply_tags helper

_Filed: 2026-10-26T08:21:59Z_
_Operation: RegisterApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other tag write in this crate goes through `RegionState::apply_tags`, which is documented as the one place tags are attached: it MERGES into what the resource already holds and refuses, with the modelled `ValidationException`, a write that would leave the resource holding more than `MAX_TAGS_PER_RESOURCE` (50) tags. `RegisterApplication` does not call it: it writes `region_state.tags_by_arn.insert(arn, tags)` directly, so the registration path replaces instead of merging and skips the ceiling entirely. `TagMap` carries no length bound in the model, so a registration naming 60 tags is input the model allows; it is persisted whole, leaving the store in a state the crate's own invariant says cannot exist — and from then on EVERY `TagResource` call on that application is refused 400, including one that only overwrites a key already held (`added == 0`, but `would_hold = 60 > 50`). The TagResource handler itself asserts the opposite is true, so one of the two sides is wrong about how the crate works.

## Evidence

Claim side — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/tag_resource.rs:188-196: "The merge, the ceiling on how many tags one resource is held for, and the refusal that ceiling is reported with all live on the state type, so the tags a registration carried and the tags this call attaches are written the same way." followed by `state.region_entry(&region).apply_tags(&input.resource_arn, &input.tags).map_err(TagResourceErrorModeled::ValidationException)?;`

Code side — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/register_application.rs:319-322: `if let Some(tags) = input.tags.filter(|tags| !tags.is_empty()) { // Keyed by ARN, so the tagging APIs join on the same place.\n            region_state.tags_by_arn.insert(arn, tags); }` — no `apply_tags`, no merge, no ceiling.

Helper that is skipped — services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:2413-2452: "Attach `tags` to the resource `arn` names, in the one place this service keeps tags ([`Self::tags_by_arn`]) ... Refused with [`validation`] when the resource would be left holding more than [`MAX_TAGS_PER_RESOURCE`] tags; a refused call writes nothing." with `if would_hold > MAX_TAGS_PER_RESOURCE { return Err(validation(...)) }`.

Model side — `smithy_query(query="shape", shape="TagMap")` returns a map with a `TagKey` pattern and a `TagValue` length only: no bound on the number of entries, so a 60-entry `Tags` is accepted by the boundary validator and reaches the handler.

Test coverage — tests/all/tag_resource.rs:487-568 (`the_per_resource_tag_ceiling_is_refused_and_persists_nothing`) exercises the ceiling ONLY through `TagResource` (it fills the resource via `tag(&emulator, &arn, &at_ceiling)`), and tests/all/resource_tags.rs:224-268 registers tag sets far below the ceiling, so no test pins the registration path's bypass.

## Steps to Reproduce

1. POST /register-application with ApplicationId "tag-bypass", ApplicationType "HANA", one instance, Sid/SapInstanceNumber/Credentials, and a `Tags` map holding 60 distinct keys — expect it to be treated like any other tag write (refused for exceeding the per-resource ceiling, or accepted only up to it). Actual: 200, and all 60 tags are persisted under the application ARN.
2. GET /tags/arn:aws:ssm-sap:<region>:<account>:application/tag-bypass — 60 tags are reported, i.e. 10 more than `MAX_TAGS_PER_RESOURCE`, a state `apply_tags` is documented to make impossible.
3. POST /tags/arn:aws:ssm-sap:<region>:<account>:application/tag-bypass with a single tag whose key is ALREADY held (an overwrite that adds nothing) — `apply_tags` computes `would_hold = 60 + 0 > 50` and the call is refused with 400 ValidationException, even though tests/all/tag_resource.rs:518-533 states an overwrite-only call is accepted at the ceiling.
