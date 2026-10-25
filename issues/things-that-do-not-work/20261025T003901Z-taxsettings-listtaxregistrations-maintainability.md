Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# The listing page-build rule is implemented three times and the copies have drifted; two contradict their own "bounded by page_size" comment

_Filed: 2026-10-25T00:39:01Z_
_Operation: ListTaxRegistrations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All three listings in this crate implement the same page-building rule by hand — "skip the keys at or before the cursor, take `page_size` entries from the ordered store, peek to decide whether a further entry remains, and issue a token naming the last entry of the page" — once per handler. The three copies have already drifted:

* `ListTaxExemptions` performs the skip with the ordered map's own `range`, and states that as the rule: "Resuming uses the ordered map's own `range`, so a cursor's page costs a lookup rather than a scan of everything before it."
* `ListTaxRegistrations` and `ListSupplementalTaxRegistrations` perform the identical skip with `.iter().filter(|(key, _)| … *key > after)`, i.e. they walk every key before the cursor on every page, while each carries a comment claiming the opposite ("the work done under it is bounded by `page_size` records, not by the size of the store").

The same two handlers also call `store.settle_due(now)` under the lock immediately after that comment, and `settle_due` walks EVERY stored registration and EVERY stored supplemental registration — so the claim that the work under the lock is bounded by the page is false for a second, independent reason in the same block.

Nothing a caller sees changes: the three copies compute the same page and the same cursor. The defect is that one rule lives in three hand-rolled copies that have already diverged, and that two of the copies are annotated with a cost claim their own code breaks — so the next change to paging (for example a new listing, or a change to how a cursor is minted) has three places to touch and a misleading comment to trust. Folding the skip/take/peek/token sequence into one helper beside `encode_page_token`/`decode_page_token` in `state.rs` (which already own the token codec for all three) would leave every response and every existing test unchanged, and would let the comment be stated once, where it is true.

## Evidence

Copy 1 — `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/list_tax_registrations.rs:184`:
```
        // The page is built while the state lock is held, so the work done under
        // it is bounded by `page_size` records, not by the size of the store; the
        // guard is released before the response is assembled.
        let (account_details, more_remain) = {
            let mut store = state.lock();
            …
            store.settle_due(::std::time::Instant::now());
            let mut remaining = store
                .registrations
                .iter()
                .filter(|(account_id, _)| {
                    resume_after
                        .as_ref()
                        .is_none_or(|after| *account_id > after)
                })
                .peekable();
            let page: Vec<AccountDetails> = remaining
                .by_ref()
                .take(page_size)
```

Copy 2 — `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/list_tax_exemptions.rs:194` states the rule the other two break and implements the skip differently:
```
        // The page is built while the state lock is held, so the work done under
        // it is bounded by `page_size` accounts, not by the size of the store;
        // the guard is released before the response is assembled. Resuming uses
        // the ordered map's own `range`, so a cursor's page costs a lookup rather
        // than a scan of everything before it.
        …
            let mut remaining = store
                .exemptions
                .range::<str, _>((lower, ::std::ops::Bound::Unbounded))
```

Copy 3 — `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/list_supplemental_tax_registrations.rs:178`:
```
        // The page is built while the state lock is held, so the work done under
        // it is bounded by `page_size` records, not by the size of the store.
        let (tax_registrations, more_remain) = {
            let mut store = state.lock();
            store.settle_due(::std::time::Instant::now());
            let mut remaining = store
                .supplemental_registrations
                .iter()
                .filter(|(authority_id, _)| {
                    resume_after
                        .as_ref()
                        .is_none_or(|after| *authority_id > after)
                })
                .peekable();
```

The second contradiction of the same comment — `services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:333` walks the whole store under that lock:
```
    pub fn settle_due(&mut self, now: Instant) {
        for stored in self.registrations.values_mut() {
            stored.settle(now);
        }
        for stored in self.supplemental_registrations.values_mut() {
            stored.settle(now);
        }
    }
```

Test coverage that must stay unchanged by a de-duplication: `tests/all/listing_page_bounds.rs` — "The table spans all three listings (whose bounds differ) against page sizes either side of each listing's ceiling" (`listing_page_bounds.rs:13`), with `every_listing_pages_by_its_own_bounds` (`:199`), `list_tax_registrations_serves_the_page_bounds_it_documents` (`:292`) and `a_sibling_listing_keeps_its_own_default_page` (`:350`); plus the cursor assertions in `tests/all/tax_exemptions.rs` and `tests/all/supplemental_tax_registrations.rs`.

## Steps to Reproduce

1. Open src/ops/list_tax_registrations.rs:184-224, src/ops/list_tax_exemptions.rs:194-235 and src/ops/list_supplemental_tax_registrations.rs:178-214 side by side: each hand-rolls the same skip-to-cursor / take(page_size) / peek / encode_page_token(last) sequence.
2. Compare the skip in the three: list_tax_exemptions.rs:206-208 uses `store.exemptions.range::<str, _>((lower, Bound::Unbounded))`, while list_tax_registrations.rs:193-201 and list_supplemental_tax_registrations.rs:186-194 use `.iter().filter(|(key, _)| *key > after)` — the copies have drifted.
3. Read the comment at list_tax_registrations.rs:184-186 ("the work done under it is bounded by `page_size` records, not by the size of the store") against the two statements under it: `store.settle_due(...)` (state.rs:333-340, which iterates every stored registration and every stored supplemental registration) and the full-scan `.filter` skip. The same comment appears at list_supplemental_tax_registrations.rs:178-179 over the same two statements.
4. Confirm no response changes are at stake: run the pagination tests (tests/all/listing_page_bounds.rs) — they pin the page sizes and cursors all three listings answer with today, which a single shared page-builder must keep byte-identical.
