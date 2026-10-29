Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0b0649dc1b525c86072f831fd9c0b574c3cd2f61

# An exemptionType the catalog does not publish makes a second exemption record when it differs only in surrounding whitespace

_Filed: 2026-10-29T01:26:04Z_
_Operation: PutTaxExemption | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0b0649dc1b525c86072f831fd9c0b574c3cd2f61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutTaxExemption` is documented by this crate as a `Put`: the exemption's identity is its type plus its jurisdiction, and a second submission of the same pair REPLACES the account's record. The crate states twice, in `state.rs`, that neither case NOR padding distinguishes two submissions. The code only honours that for the jurisdiction: `is_same_exemption` trims `authority.country` and `authority.state` before comparing them, but compares the recorded type's `description` with `eq_ignore_ascii_case` and no trim.

For a token the published catalog names this is harmless, because `find_exemption_type` trims the token before matching the catalog and the record is then held under the catalog entry's own (unpadded) description. For a token the catalog does NOT name, `submitted_exemption_type` records `description: Some(exemption_type.to_string())` exactly as submitted, padding included. So submitting the same unpublished type twice, once with surrounding whitespace, stores TWO exemption records for the account instead of replacing one — and the reads (`BatchGetTaxExemptions`, `ListTaxExemptions`) then report two exemptions whose `taxExemptionType.description` differ only by spaces. The duplicate also consumes a second slot of the resident exemption bound.

The fix is the trim the sibling comparisons in the same function already apply.

## Evidence

Stated rule, side 1 — `services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:863` (doc of `store_exemptions`): "append; case and padding in either do not make a second exemption" / ":864  ([`is_same_exemption`])."

Stated rule, side 2 — `src/state.rs:3316-3318` (doc of `is_same_exemption`): "Case and padding make no second exemption either: they are compared the way [`normalized_authority`] stores a jurisdiction, so what the store treats as one jurisdiction it treats as one record."

Code that breaks it — `src/state.rs:3329-3339`:
```
    stored
        .exemption_type
        .description
        .as_deref()
        .is_some_and(|held| held.eq_ignore_ascii_case(description))
        && stored
            .authority
            .country
            .trim()
            .eq_ignore_ascii_case(authority.country.trim())
        && same_state
```
The country is `.trim()`-ed and `same_state` (`src/state.rs:3324-3328`) trims both states; the `description` comparison has no trim. The `description` it is compared against is the untrimmed token — `src/state.rs:3262`: `description: Some(exemption_type.to_string()),` in `submitted_exemption_type`, while the crate's own `find_exemption_type` does trim (`src/state.rs:3228`: `let wanted = exemption_type.trim();`).

Writer — `src/ops/put_tax_exemption.rs:226-233` calls `store.store_exemptions(&input.account_ids, &input.exemption_type, &input.authority, …)`, and `src/state.rs:926-932` pushes a new record when `is_same_exemption` finds no match:
```
            match held
                .iter()
                .position(|stored| is_same_exemption(stored, &description, authority))
            {
                Some(index) => held[index] = submitted.clone(),
                None => held.push(submitted.clone()),
            }
```

Test coverage: `tests/all/exemption_type_admission.rs:231-262` (`an_unpublished_token_identifies_one_record_per_jurisdiction`) is the suite for exactly this rule and its header says the rows differ by "the token, and neither (a repeat, re-cased and padded)" — but every row spells the token identically (`"Exemption certificate on file"`); only `authority.state` is padded/re-cased (`" ohio "`, line 253). `tests/all/tax_exemptions.rs:1038-1091` likewise only pads the state (`" Ohio "`, line 1087) and re-cases a token that IS published. So a padded UNPUBLISHED token is unasserted.

## Steps to Reproduce

1. POST /PutTaxExemption with {"accountIds":["000000000011"],"authority":{"country":"US"},"exemptionType":"Exemption certificate on file","exemptionCertificate":{"documentName":"c.pdf","documentFile":"<base64>"}} — 200 with a caseId.
2. POST /BatchGetTaxExemptions with {"accountIds":["000000000011"]} and observe exactly one entry in taxExemptionDetailsMap["000000000011"].taxExemptions.
3. POST /PutTaxExemption again with the SAME body except "exemptionType":"Exemption certificate on file " (one trailing space) — 200.
4. POST /BatchGetTaxExemptions with {"accountIds":["000000000011"]} again. Observed: TWO taxExemptions, whose taxExemptionType.description are "Exemption certificate on file" and "Exemption certificate on file ". Expected per state.rs:863 and state.rs:3316-3318: one record, replaced in place.
