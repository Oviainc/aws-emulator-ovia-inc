Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# The ARN-type-to-store dispatch for tags is written twice, once per borrow kind, so the read and write halves can drift

_Filed: 2026-10-27T22:23:09Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Which family's store answers for an ARN's `<type>` segment is implemented twice in `state.rs`: once in `ComAmazonawsTranscribeTranscribeState::tags_of` (the read half, used by `ListTagsForResource` through `tags`, and by every job start through `holds`/`unheld_reference_error`) and once in `ComAmazonawsTranscribeTranscribeState::tags_mut` (the write half, used by `TagResource` and `UntagResource`). Both are nine-arm matches over `ResourceFamily` naming the same nine stores in the same order; only the borrow differs (`get` vs `get_mut`).

The crate's own doc on `tags` says the two "must agree about which ARN names which stored record", which is exactly the invariant two hand-written copies cannot enforce: a family added later (or a store renamed) has to be remembered in both, and forgetting the write half gives a resource whose tags can be read but not written — the shape of a defect this crate has already had filed against it twice (the fixed category-ARN and Call-Analytics-job tagging issues).

The crate already has the tool for this: `listed_records!` and `clocked_records!` generate per-family impls from one list of family names, so the same macro form can emit both halves of the dispatch from a single table of `ResourceFamily => store` pairs. Responses and messages stay byte-identical; this is purely a single-homing of one rule.

## Evidence

The invariant, stated in the crate — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:1800-1803:
  "/// The read-only half of [`Self::tags_mut`], so a reporting operation
   /// (`ListTagsForResource`) takes no mutable borrow for a read. The two must agree
   /// about which ARN names which stored record, so both read the same ARN qualifiers
   /// ([`ResourceRef::is_served_in`]) and match on the same resource types."

Copy 1 — state.rs:1817-1856 (`tags_of`):
  "        match family {
               ResourceFamily::TranscriptionJob => self
                   .transcription_jobs
                   .get(region, name)
                   .map(|record| &record.tags),
               ResourceFamily::MedicalTranscriptionJob => self
                   .medical_transcription_jobs
                   .get(region, name)
                   .map(|record| &record.tags),
   …   (nine arms, ending `ResourceFamily::CallAnalyticsJob`)"

Copy 2 — state.rs:1872-1913 (`tags_mut`):
  "        match ResourceFamily::from_resource_type(&resource.resource_type)? {
               ResourceFamily::TranscriptionJob => self
                   .transcription_jobs
                   .get_mut(region, &resource.name)
                   .map(|record| &mut record.tags),
               ResourceFamily::MedicalTranscriptionJob => self
                   .medical_transcription_jobs
                   .get_mut(region, &resource.name)
                   .map(|record| &mut record.tags),
   …   (the same nine arms, in the same order)"

The per-family macro this crate already uses for exactly this kind of repetition — state.rs:2961-2992 (`listed_records!`) and state.rs:192-216 (`clocked_records!`).

Test coverage that pins the behaviour a refactor must not change: tests/all/tagging_resource_families.rs ("The three tagging operations over EVERY resource family this service stores records of, end to end over the wire", lines 3-12) and tests/all/tagging_arn_qualifiers.rs, which drives "the SAME families through the same creates" (tagging_resource_families.rs:53-55).

## Steps to Reproduce

1. Open services/service-com-amazonaws-transcribe-transcribe/src/state.rs and read `tags_of` (lines 1817-1856) and `tags_mut` (lines 1872-1913) side by side: the same nine `ResourceFamily` arms name the same nine stores, differing only in `get` vs `get_mut`.
2. Note the doc at state.rs:1800-1803 requiring the two to agree about which ARN names which stored record — an invariant held only by hand.
3. Note that `listed_records!` (state.rs:2961) and `clocked_records!` (state.rs:192) already generate per-family impls from one list, so one table can emit both halves.
4. Run the tagging suites (tests/all/tagging_resource_families.rs, tests/all/tagging_arn_qualifiers.rs): they must pass unchanged after the two matches are single-homed.
