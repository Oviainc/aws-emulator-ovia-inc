Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9c3b4064a1735206e464a3fb30963f5d1ed2eddb

# Hoist the "mint a fresh counter id, holding the map to its capacity by releasing the oldest" rule into one place instead of three copies

_Filed: 2026-10-27T13:47:42Z_
_Operation: GetTermForReport | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9c3b4064a1735206e464a3fb30963f5d1ed2eddb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` implements the same two-part rule three times, once per store it keys:

1. hold a `BTreeMap` to a capacity constant by popping its first (oldest) entry, and
2. mint an id/token by advancing a `u64` counter with `wrapping_add(1)`, formatting it as a fixed prefix plus `{:016x}`, and re-rolling while the map already holds the candidate.

`ComAmazonawsArtifactArtifactState::mint_term_grant` (reached from the `GetTermForReport` handler) and `ComAmazonawsArtifactArtifactState::record_inquiry_export` (reached from the `ExportComplianceInquiry` handler) carry BOTH halves, line for line, differing only in the map, the capacity constant, the counter field and the id prefix. `ComAmazonawsArtifactArtifactState::mint_inquiry_id` (reached from the `CreateComplianceInquiry` handler) carries the minting half a third time.

Because the rule is written out per store, the copies can drift: a correction to the eviction order (`pop_first` is only "the oldest" because the fixed-width hex ids rise with the counter, an invariant each copy restates in its own doc comment), to the collision re-roll, or to the id width would have to be made three times, and a copy that was missed would silently key one store differently from the others. This crate has already hoisted two other rules written out per call site for exactly this reason (see the fixed issues `20261024T223115Z-artifact-listreports-maintainability.md`, the five-times page-walk, and `20261026T082605Z-artifact-exportcomplianceinquiry-maintainability.md`, the two-times query-ownership rule).

A fix is a single helper in `crate::state` taking the map, the capacity, the counter and the id prefix, with the three callers delegating to it. Every minted id, every eviction and every response stays byte-identical, so no existing test changes.

## Evidence

Copy 1 — `services/service-com-amazonaws-artifact-artifact/src/state.rs:1552-1569` (`mint_term_grant`, called from `src/ops/get_term_for_report.rs:204`, `let term_token = store.mint_term_grant(&report, resolved_version, now);`):

```rust
        while self.term_tokens.len() >= TERM_GRANT_CAPACITY {
            if self.term_tokens.pop_first().is_none() {
                break;
            }
        }
        let token = loop {
            self.term_token_seq = self.term_token_seq.wrapping_add(1);
            let candidate = format!("term-{:016x}", self.term_token_seq);
            if !self.term_tokens.contains_key(&candidate) {
                break candidate;
            }
        };
```

Copy 2 — `services/service-com-amazonaws-artifact-artifact/src/state.rs:1638-1657` (`record_inquiry_export`, called from `src/ops/export_compliance_inquiry.rs:240`, `let export_id = store.record_inquiry_export(`):

```rust
        while self.inquiry_exports.len() >= INQUIRY_EXPORT_CAPACITY {
            if self.inquiry_exports.pop_first().is_none() {
                break;
            }
        }
        let id = loop {
            self.inquiry_export_seq = self.inquiry_export_seq.wrapping_add(1);
            let candidate = format!("export-{:016x}", self.inquiry_export_seq);
            if !self.inquiry_exports.contains_key(&candidate) {
                break candidate;
            }
        };
```

Copy 3 (the minting half) — `services/service-com-amazonaws-artifact-artifact/src/state.rs:1490-1498` (`mint_inquiry_id`, called from `src/ops/create_compliance_inquiry.rs:289`, `let id = store.mint_inquiry_id();`):

```rust
        loop {
            self.inquiry_seq = self.inquiry_seq.wrapping_add(1);
            let candidate = format!("compliance-inquiry-{:016x}", self.inquiry_seq);
            if !self.inquiries.contains_key(&candidate) {
                return candidate;
            }
        }
```

The same invariant is restated in each copy's own doc comment, which is the drift surface: `state.rs:1544-1551` ("The counter advances on every call and the loop skips a token the store already holds … the tokens also rise lexicographically, so the store's first entry is its oldest grant. The store is held to [`TERM_GRANT_CAPACITY`] by releasing the oldest grants first"), `state.rs:1427-1431` ("the ids are minted from a monotonic counter in fixed-width hex, so the first entry is always the oldest export, which is what lets … `record_inquiry_export` hold the map to [`INQUIRY_EXPORT_CAPACITY`] without scanning it") and `state.rs:1486-1489` ("The counter advances on every call and the loop skips an id the store already holds").

Existing tests that pin the behaviour the fix must leave unchanged:
- `tests/all/report_workflow.rs:990` `the_term_grant_store_stays_bounded_however_many_terms_are_accepted` — issues `TERM_GRANT_CAPACITY + 1` grants, then asserts the newest still downloads (200) and the first was released (400), i.e. both halves of copy 1.
- `tests/all/compliance_inquiry_export.rs:722` `every_export_of_one_inquiry_is_its_own_document` and `:436` `an_export_answers_a_url_naming_the_inquiry_it_was_taken_from` — pin the minted export id as it appears in the announced URL path (copy 2).
- `tests/all/compliance_inquiry_lifecycle.rs:385` `every_submission_is_created_and_read_back_identically` and `tests/all/compliance_inquiry_feedback.rs:758` (`"the id a later create would mint", "compliance-inquiry-0000000000000009"`) — pin the minted inquiry id shape and sequence (copy 3).

## Steps to Reproduce

1. Open `services/service-com-amazonaws-artifact-artifact/src/state.rs` and read `mint_term_grant` (lines 1552-1581), `record_inquiry_export` (lines 1638-1670) and `mint_inquiry_id` (lines 1490-1498).
2. Observe that the capacity-release loop (`while <map>.len() >= <CAPACITY> { if <map>.pop_first().is_none() { break; } }`) appears verbatim in the first two, and the counter/format/collision-re-roll loop (`<seq> = <seq>.wrapping_add(1); let candidate = format!("<prefix>-{:016x}", <seq>); if !<map>.contains_key(&candidate)`) appears verbatim in all three, differing only in map, constant, counter and prefix.
3. Note that each copy restates the same invariant in its own doc comment (state.rs:1544-1551, 1427-1431, 1486-1489), so a change to the eviction order or the id width has to be made three times.
4. Confirm the behaviour a fix must preserve by running the crate's tests: `the_term_grant_store_stays_bounded_however_many_terms_are_accepted` (tests/all/report_workflow.rs:990), `every_export_of_one_inquiry_is_its_own_document` (tests/all/compliance_inquiry_export.rs:722) and `every_submission_is_created_and_read_back_identically` (tests/all/compliance_inquiry_lifecycle.rs:385).
5. Hoist the rule into one `crate::state` helper parameterised by map, capacity, counter and prefix, delegate the three call sites to it, and re-run the suite: every minted id, every eviction and every response must be unchanged.
