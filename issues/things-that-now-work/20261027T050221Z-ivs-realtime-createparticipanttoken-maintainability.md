Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 53db56bf3158c7205a85c81c618ec0b3c4193a64

# The documented 1 KB attributes rule is implemented four times in four handlers instead of once in state.rs

_Filed: 2026-10-27T05:02:21Z_
_Operation: CreateParticipantToken | Class: maintainability_

> _Note: this issue was identified against the code base at commit `53db56bf3158c7205a85c81c618ec0b3c4193a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The documented-but-unmodelled `attributes` size limit is implemented as copy-pasted logic in four handlers: each compares `crate::state::attributes_size(..)` against `crate::state::MAX_ATTRIBUTES_BYTES` and formats the same refusal sentence itself. `src/state.rs` exports only the measure and the constant, while every other documented-but-unmodelled rule of this crate is centralised there as a `check_*` helper that returns the refusal sentence (`check_tag_count`, `check_tag_keys_unreserved`, `check_resource_arn`), which the handlers then merely map onto their own `ValidationException`. The four copies of the comparison-plus-wording can drift (the bound, the measure, or the sentence can be changed in one handler and not the others), which is the thing the crate's one-rule-one-place convention exists to prevent. A `state::check_attributes_size(operation, member, attributes)` beside the existing `check_*` helpers would leave every response byte-for-byte identical.

## Evidence

Four copies of one rule:
- src/ops/create_participant_token.rs:183-195: `let size = crate::state::attributes_size(&attributes); if size > crate::state::MAX_ATTRIBUTES_BYTES { ... format!("{}.attributes is required to have a maximum size of {} bytes, found {size}", Self::NAME, crate::state::MAX_ATTRIBUTES_BYTES,)`
- src/ops/create_ingest_configuration.rs:208-221: same two lines and the identical `format!("{}.attributes is required to have a maximum size of {} bytes, found {size}", ...)`
- src/ops/start_participant_replication.rs:206-216: same again
- src/ops/create_stage.rs:167-188: the same comparison over the list, wording only the member path differently (`"{}.participantTokenConfigurations.{}.member.attributes is required to have a maximum size of {} bytes, found {size}"`)

The convention the crate follows for every other unmodelled rule, src/state.rs:4666 `pub fn check_tag_count(operation: &str, found: usize) -> Result<(), String> {`, src/state.rs:4726 `pub fn check_tag_keys_unreserved<'a>(` and src/state.rs:4643 `pub fn check_resource_arn(operation: &str, resource_arn: &str) -> Result<(), String> {` — and src/state.rs:4696 which names the attributes limit as the same class of rule: "so the rule is documented-but-unmodelled and belongs to the handlers, exactly like the 1 KB `attributes` limit ([`MAX_ATTRIBUTES_BYTES`])".

Existing tests that pin the behaviour this refactor must not change: tests/all/stages.rs:392 `attributes_over_one_kilobyte_are_refused` (CreateParticipantToken and CreateStage, cases "exactly at the cap" 1024 → 200 and one byte over → 400), tests/all/ingest_configurations.rs:727 `an_oversized_attributes_map_is_refused_at_the_documented_bound` (asserting the message starts with "CreateIngestConfiguration.attributes ", line 766), and tests/all/participant_replication.rs:839-849 (asserting "StartParticipantReplication.attributes is required to have a maximum size …").

## Steps to Reproduce

1. Open src/ops/create_participant_token.rs:183-195, src/ops/create_ingest_configuration.rs:208-221, src/ops/start_participant_replication.rs:206-216 and src/ops/create_stage.rs:167-188: the same comparison against MAX_ATTRIBUTES_BYTES and the same refusal sentence appear in all four.
2. Compare with src/state.rs:4643/4666/4726, where the crate's other documented-but-unmodelled rules each live once as a `check_*` helper returning the refusal sentence.
3. Change the bound or the wording in one handler only: the other three keep the old rule, and the tests named above pass for whichever operation they cover — the drift the single helper would make impossible.
