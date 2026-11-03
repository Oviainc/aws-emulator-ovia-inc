Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 600bb9247975905af81e2aa2cda8a015f9396423

# ExportComponents duplicates ListComponents' page-token parsing and cursor walk instead of sharing one helper

_Filed: 2026-10-29T18:56:27Z_
_Operation: ExportComponents | Class: maintainability_

> _Note: this issue was identified against the code base at commit `600bb9247975905af81e2aa2cda8a015f9396423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two component-listing handlers carry two copies of the same pagination logic: the `nextToken` parse-or-refuse block (identical except for the operation name interpolated into the message) and the cursor-resume walk that skips records below the cursor, stops at the page bound and issues the next token from the first unserved record's cursor. The only real differences are where the page bound comes from (`page_size` vs `crate::state::EXPORT_PAGE_LIMIT`) and the projection pushed (`to_summary()` vs `to_component()`). Because the rule lives twice, a change to one copy — the resume-past-a-deleted-record semantics, or the `cursor <= issued` refusal — silently leaves the other copy behind. A single generic helper in `crate::state` (alongside `page_token`/`parse_page_token`, which both already share) parameterised by page bound and projection removes the second copy with no change to any response.

## Evidence

services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/list_components.rs:180-214:
  `let cursor = match input.next_token.as_deref() { None => 0, Some(token) => { match crate::state::parse_page_token(token, &key, store.cursor_seq) { Some(cursor) => cursor, None => { return Err(ListComponentsErrorModeled::InvalidParameterException(...)) } } } };`
  `for component in store.components_in(&input.app_id, &input.environment_name) { if component.cursor < cursor { continue; } if entities.len() == page_size { next_token = Some(crate::state::page_token(&key, component.cursor)); break; } entities.push(component.to_summary()); }`

services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/export_components.rs:166-200 — the same two blocks again:
  `let cursor = match input.next_token.as_deref() { None => 0, Some(token) => { match crate::state::parse_page_token(token, &key, store.cursor_seq) { Some(cursor) => cursor, None => { return Err(ExportComponentsErrorModeled::InvalidParameterException(...)) } } } };`
  `for component in store.components_in(&input.app_id, &input.environment_name) { if component.cursor < cursor { continue; } if entities.len() == crate::state::EXPORT_PAGE_LIMIT { next_token = Some(crate::state::page_token(&key, component.cursor)); break; } entities.push(component.to_component()); }`

Even the resume comment is duplicated verbatim — list_components.rs:203-205 and export_components.rs:189-191:
  "// Resume at the first component at or after the cursor, so a page is\n// still served when the component the token named was deleted\n// meanwhile."

Tests that pin both copies and must stay green unchanged: tests/all/component_lifecycle.rs:581 `list_components_pages_over_one_stable_order` (and its token-refusal rows at component_lifecycle.rs:680-727), tests/all/export_components.rs:502 `export_components_pages_over_one_stable_order` and tests/all/export_components.rs:555 `export_components_refuses_a_token_it_never_issued`.

## Steps to Reproduce

1. Read services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/list_components.rs:180-214
2. Read services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/export_components.rs:166-200
3. Compare: the token parse-or-refuse block and the cursor-resume walk are the same logic twice, differing only in the page bound, the projection and the operation name in the refusal message
