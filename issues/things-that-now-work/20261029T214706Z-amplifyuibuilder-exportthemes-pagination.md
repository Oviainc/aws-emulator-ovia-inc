Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# Page tokens carry no collection discriminator, so ExportThemes serves an empty page for a ListComponents token (and vice versa)

_Filed: 2026-10-29T21:47:06Z_
_Operation: ExportThemes | Class: pagination_

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All three paginated handlers of this crate (`ListComponents`, `ExportComponents`, `ExportThemes`) mint and parse their `nextToken` through the single pair `crate::state::page_token` / `crate::state::parse_page_token`. A token encodes only the fixed prefix `"c-"`, a digest of (appId, environmentName) and the cursor — nothing that says WHICH collection the page was served over — and `parse_page_token` accepts any cursor `<= cursor_seq`, the ONE counter components and themes both draw from. So a token issued by `ListComponents`/`ExportComponents` is indistinguishable from one issued by `ExportThemes` for the same app environment and is accepted by all of them.

That contradicts what each handler and the helper state. A `ListComponents` token fed to `ExportThemes` resolves to a COMPONENT cursor; because the component cursors are higher than the theme cursors in the walk, `ExportThemes` skips every stored theme and answers HTTP 200 with `entities: []` and no `nextToken` — exactly the "served as an empty page" outcome its own comment says must be refused — rather than the modelled `InvalidParameterException` (400). The reverse direction is the same defect: a token `ExportThemes` issued is accepted by `ListComponents`, which then serves a silently truncated page of components. In both cases a caller-supplied token the operation never issued turns a documented refusal into a success with wrong content.

## Evidence

The helper the three handlers share says a token is per-operation and per-app-environment:

- services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:193-195:
  `/// How a `ListComponents` page token is spelled, so a token this operation never`
  `/// issued can be told apart from one it did.`
  `const PAGE_TOKEN_PREFIX: &str = "c-";`
- state.rs:1085-1086: `/// The cursor `token` resumes `key`'s list at, or `None` when it is not a token`
  `/// `ListComponents` issued for that app environment.`

But the only things it checks are the prefix, the (appId, environmentName) digest and `cursor <= issued` (state.rs:1091-1107), and the scope digest hashes nothing but the two labels (state.rs:1114-1121: `key.0.hash(&mut hasher); key.1.hash(&mut hasher);`).

The same spelling is minted by another collection's export — export_themes.rs:192: `next_token = Some(crate::state::page_token(&key, theme.cursor));` — beside list_components.rs:210: `next_token = Some(crate::state::page_token(&key, component.cursor));` — and the cursors come from one shared counter: state.rs:510-512 `/// The counter [`StoredComponent::cursor`] and [`StoredTheme::cursor`] /// values are drawn from. pub cursor_seq: u64,` (create_component.rs:253 and create_theme.rs:198 both call `store.next_cursor()`).

The handler comment that the code breaks — export_themes.rs:159-162:
  `// The token carries the app environment it was issued for and the cursor`
  `// the next page starts at. A malformed token, one issued for another app`
  `// environment, or a cursor past everything handed out names no page of`
  `// this export, so it is refused rather than served as an empty page.`
and list_components.rs:176-179: `// A token this operation never issued — a malformed one, one issued while listing another app environment, or a cursor past everything handed out — names no page, so it is refused rather than silently served as a page of this list.` (refusal message: `"nextToken {token:?} is not a token returned by ListComponents for this app environment"`, list_components.rs:188-191).

Test coverage: the existing token-refusal suites only exercise malformed tokens, a token from another app environment and a never-handed-out cursor — tests/all/export_themes.rs:551-562 (`for token in ["not-a-token", "c-", "c-00000000000000zz", "0", &elsewhere, &never_issued_cursor]`) and tests/all/component_lifecycle.rs:708-715 (the same six rows). No row feeds a token issued by a DIFFERENT paginated operation, so this case is unpinned, even though the suite's own doc states the rule: tests/all/export_themes.rs:509-511 "A `nextToken` this service never issued for the addressed app environment names no page of the export, so it is refused with the modelled `InvalidParameterException` (HTTP 400) rather than served as an empty page."

## Steps to Reproduce

1. Mount the service on one engine (as tests/all/export_themes.rs does) and address app `d1234abcd`, environment `staging`.
2. POST /app/d1234abcd/environment/staging/themes twice (two valid CreateThemeData payloads) — these take cursors 1 and 2 from `cursor_seq`.
3. POST /app/d1234abcd/environment/staging/components twice (two valid CreateComponentData payloads) — these take cursors 3 and 4 from the SAME counter.
4. GET /app/d1234abcd/environment/staging/components?maxResults=1 and read `nextToken` from the body: it is `page_token(key, 4)`, a token only ListComponents issued.
5. GET /export/app/d1234abcd/environment/staging/themes?nextToken=<that token>.
6. Expected per export_themes.rs:159-162 and state.rs:1085-1086: HTTP 400 InvalidParameterException, because ExportThemes never issued that token. Actual: HTTP 200 with `entities: []` and no `nextToken` — an empty export of an app environment that holds two themes (every theme has cursor < 4, so the walk at export_themes.rs:185-196 skips them all).
7. The symmetric case: with >100 themes in the environment, take the `nextToken` ExportThemes issues and pass it to GET /app/{appId}/environment/{environmentName}/components?nextToken=… — it is accepted instead of refused, and serves a page of components resumed at a theme's cursor.
