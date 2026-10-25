Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: dbe9ca7a8a80909c811e91f1b58fc3cbb14634ed

# Minted rule identifiers do not sort in creation order, so ListRules pages out of order and can skip a rule created mid-walk

_Filed: 2026-10-25T04:30:16Z_
_Operation: ListRules | Class: pagination_

> _Note: this issue was identified against the code base at commit `dbe9ca7a8a80909c811e91f1b58fc3cbb14634ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRules` walks the rule map in `BTreeMap` key order and uses the last identifier it returned as the page cursor (`Bound::Excluded`). The store documents that this is safe because minted identifiers sort in the same order as the monotonic seed they are rendered from. They do not: the base62 alphabet the renderer uses maps digit values 10..=35 to the LOWERCASE letters and 36..=61 to the UPPERCASE letters, while `BTreeMap<String, Rule>` compares keys by BYTES, where `'A'` (0x41) sorts before `'a'` (0x61). So the identifier minted for seed 36 (`"0000000000A"`) sorts BEFORE the identifier minted for seed 10 (`"0000000000a"`), even though it was created 26 rules later.

Two consequences, both of which the crate's own comments say cannot happen:

1. Once a Region holds more than 35 rules, the order `ListRules` reports them in is neither creation order nor seed order: the newest rules (seeds 36..61) are reported in the middle of the page, before the older seeds 10..35.
2. A rule created AFTER a page was handed back can sort before that page's cursor, so the resumed walk never reports it and no further `NextToken` is issued — the rule is silently skipped for the whole walk. With a genuinely order-preserving rendering a newly minted key always sorts after every existing key, which is exactly the property the module doc claims ("a page boundary stays stable while rules are added or removed").

The defect is in the alphabet ordering of `base62_identifier`, which contradicts its own doc comment.

## Evidence

The claimed invariant, `services/service-com-amazonaws-rbin-amazonrecyclebin/src/state.rs:688-690`:
```
/// Render `seed` as exactly [`IDENTIFIER_LEN`] base62 digits, most significant
/// first and left-padded with `0`, so the result satisfies `^[0-9a-zA-Z]{11}$` and
/// sorts in the same order as the seed.
```
and `src/state.rs:601-604`:
```
    /// Mint an identifier no stored rule holds, as 11 base62 digits.
    ///
    /// Monotonic, so the map's key order is also creation order; the loop skips a
```
and the module doc, `src/state.rs:7-9`:
```
//! ARN index. The ordering is load-bearing: `ListRules` pages through the map in
//! key order, and its `NextToken` is the last identifier it returned, so a page
//! boundary stays stable while rules are added or removed.
```

The code that breaks it, `src/state.rs:30-33`:
```
/// The alphabet `^[0-9a-zA-Z]{11}$` allows, in the digit order used to render a
/// minted identifier. 62^11 exceeds `u64::MAX`, so every seed renders in 11 digits.
const BASE62_DIGITS: &[u8; 62] =
    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
```
Index 10 is `'a'` (byte 0x61) and index 36 is `'A'` (byte 0x41), so a HIGHER digit value renders a LOWER byte — the mapping is not monotonic in byte order, and `src/state.rs:592` stores the rules in a byte-ordered map: `pub rules: BTreeMap<String, Rule>,`.

The walk and the cursor that rely on the broken invariant: `src/ops/list_rules.rs:203` `for (identifier, rule) in store.page_from(resume_after.as_deref()) {`, `src/ops/list_rules.rs:224` `next_token = last_reported.map(|id| crate::state::encode_page_token(id));`, and `src/state.rs:680-684`:
```
        let lower = match after {
            Some(identifier) => Bound::Excluded(identifier.to_string()),
            None => Bound::Unbounded,
        };
        self.rules.range((lower, Bound::Unbounded))
```

Test coverage: none pins the order or the add-during-paging case. `tests/all/retention_rule_lifecycle.rs:505-582` (`list_rules_pages_through_every_rule_for_each_page_size`) creates only 7 rules (seeds 1..7, all rendered as digits, where the alphabet happens to be order-preserving) and compares pages as a `BTreeSet` (`assert_eq!(unique, all, ...)`, line 577), so it is order-insensitive and never creates a rule between two pages. A grep for "creation order"/"key order" under `tests/` matches nothing.

## Steps to Reproduce

1. Create 35 retention rules of ResourceType EBS_SNAPSHOT (seeds 1..35 render as "00000000001".."00000000009" then "0000000000a".."0000000000z").
2. POST /list-rules with {"ResourceType":"EBS_SNAPSHOT","MaxResults":35} — note the returned NextToken is base64 of the last key in BYTE order, "0000000000z".
3. Create one more rule — its identifier is "0000000000A" (seed 36), which sorts BEFORE "0000000000z".
4. POST /list-rules again with the NextToken from step 2 — the page is empty and no further NextToken is issued, so the rule created in step 3 is never reported by the walk.
5. Alternatively create 36 rules from a fresh store and POST /list-rules with no MaxResults: the rule created 36th ("0000000000A") is reported 10th, before the rules created 10th through 35th, so the reported order is neither creation nor seed order.
