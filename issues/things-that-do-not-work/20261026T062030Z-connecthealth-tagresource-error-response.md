Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: adb8f986b66b684ed38690a8bd355d4f46d0ebf5

# Tag-entry refusal names an arbitrary offending entry and reports only one violation, so identical requests get different 400 bodies

_Filed: 2026-10-26T06:20:30Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `adb8f986b66b684ed38690a8bd355d4f46d0ebf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both operations that accept a `TagMap` (`TagResource` and `CreateDomain`) refuse an unacceptable entry through the shared `crate::service::tag_bounds::violation`, which walks the request's `tags` with `find_map` and returns the FIRST entry it finds unacceptable. The input type is a `std::collections::HashMap`, whose iteration order is unspecified and randomly seeded per process/map, so when a request carries more than one unacceptable entry (e.g. an empty key AND an over-long value) the handler picks an arbitrary one: the same request can be answered with two different `Message` texts naming two different entries on two different runs. The module also stops at the first violation and renders it through `envelope(&[violation])`, so the body always reads `1 validation error detected: ...` — while the module's own doc claims a refusal made here "reads exactly like one made at the boundary", and the boundary walk collects every violation and renders `N validation error(s) detected: ...` (as this crate's own test module documents). The defect is the non-determinism (the service answers one of several possible bodies for one byte-identical request) plus the divergence from the parity the helper documents.

## Evidence

The helper documents a deterministic choice: `services/service-com-amazonaws-connecthealth-connecthealth/src/service/tag_bounds.rs:154-157` — "/// The refusal text for the first entry of `tags` this service does not accept — one\n/// outside the modelled `TagKey` / `TagValue` lengths (BOTH ends of both bounds, so an\n/// empty key is refused and an empty value is not), or one whose key lies in the reserved\n/// [`RESERVED_TAG_KEY_PREFIX`] namespace — or `None` when every entry is acceptable." and implements it as `tag_bounds.rs:164` — "    tags.into_iter().find_map(|(key, value)| {".

But there IS no "first" entry: `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/types/tag_resource_types.rs:18` — "    pub tags: std::collections::HashMap<String, String>," and the handler hands that map straight in, `src/ops/tag_resource.rs:222` — "        if let Some(violation) = crate::service::tag_bounds::violation(input.tags.iter()) {". `CreateDomain` does the same with its optional map, `src/ops/create_domain.rs:283` — "            if let Some(violation) = crate::service::tag_bounds::violation(tags.iter()) {".

The claimed parity with the boundary refusal: `src/service/tag_bounds.rs:29-34` — "//! NOTHING about constraint checking or its wording is reimplemented: ... and the\n//! text is [`envelope`]'s. A refusal of a LENGTH made here therefore reads exactly like\n//! one made at the boundary". What the boundary renders for several violations, from this crate's own test module `tests/all/error_detail_member.rs:17` — "//!   `N validation error(s) detected: Value '<value>' at '<member>' failed to satisfy", whereas `tag_bounds.rs:168` / `tag_bounds.rs:185` always pass a one-element slice (`envelope(&[violation])`), so a two-violation request still reads `1 validation error detected`.

Test coverage: `tests/all/tag_map_bounds.rs` pins the rule but deliberately never sends two offending entries at once — `tag_map_bounds.rs:283-287` is the only multi-entry refusal row and it carries exactly one bad entry (`json!({ "env": "prod", "": "v" })`), and `tag_map_bounds.rs:104-119` asserts only the status/code/"failed to satisfy constraint" substring — so nothing pins which entry a multi-violation request names.

## Steps to Reproduce

1. POST /domain with {"name":"clinic"} and keep the returned `arn`.
2. POST /tags/<percent-encoded arn> with tags = {"": "v", "k": "<257-character value>"} — two entries, each unacceptable (empty key; value past the 256-character `TagValue` bound).
3. Observe HTTP 400 `ValidationException` whose `Message` is `1 validation error detected: ...` naming ONE of the two entries.
4. Repeat the byte-identical request against a fresh emulator process and observe the refusal can name the OTHER entry, because `TagResourceInput::tags` is a `HashMap` and `tag_bounds::violation` returns whichever entry iteration reaches first.
5. Compare with a boundary refusal of two members, which the crate documents as `N validation error(s) detected: ...` (tests/all/error_detail_member.rs:17): the handler never reports more than one.
