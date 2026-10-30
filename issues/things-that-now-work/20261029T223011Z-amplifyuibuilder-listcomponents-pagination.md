Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: cd3575e4f7f669abed45d33521966e31c07e4346

# A page token whose cursor is 0 — a cursor the service never hands out — is served as the first page instead of refused

_Filed: 2026-10-29T22:30:11Z_
_Operation: ListComponents | Class: pagination_

> _Note: this issue was identified against the code base at commit `cd3575e4f7f669abed45d33521966e31c07e4346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::parse_page_token` is the one token validator the three paginated handlers of this crate share (`ListComponents`, `ExportComponents`, `ExportThemes`). Its own documentation claims that a well-formed token nobody was ever given names no page, and `ListComponents`' handler comment says a token it never issued "is refused rather than silently served as a page of this list". But the only cursor bound the code applies is the UPPER one: `(cursor <= issued).then_some(cursor)`.

Cursors are handed out by `next_cursor`, which pre-increments (`self.cursor_seq = self.cursor_seq.saturating_add(1)`), so the lowest cursor any record ever holds — and therefore the lowest cursor any issued token ever carries — is 1. A token spelled `c-<scope>0000000000000000` is therefore a token the service never issued, yet it passes every check: the prefix matches, the 24 digits are hex, the app-environment digest matches (a client takes it from any real token of that app environment), and `0 <= issued` holds. `ListComponents` then takes the `cursor` branch and walks from `component.cursor < 0`, i.e. from the start, answering HTTP 200 with page one — turning the modelled `InvalidParameterException` (400) the handler is written to answer into a success with content. The same holds for `ExportComponents` and `ExportThemes`, which call the same helper.

This is a different hole from the already-filed ExportThemes token issue (`20261029T214706Z-amplifyuibuilder-exportthemes-pagination`, which is about the token carrying no COLLECTION discriminator so a cursor issued for another collection is accepted): adding a collection discriminator does not add the missing lower bound, and a cursor-0 token forged from a ListComponents token is accepted by ListComponents itself.

Refusing a cursor below the first one handed out changes no answer for any token the service actually issued, so no existing test's expectation moves.

## Evidence

The rule, stated by the helper the three handlers share:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1085-1089
    /// The cursor `token` resumes `key`'s list at, or `None` when it is not a token
    /// `ListComponents` issued for that app environment.
    ///
    /// A cursor beyond `issued` — the highest cursor the service has handed out — is
    /// refused too, so a well-formed token nobody was ever given names no page.

The code under it, which bounds the cursor only from ABOVE:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1105-1106
    let cursor = u64::from_str_radix(cursor, 16).ok()?;
    (cursor <= issued).then_some(cursor)

That 0 is never handed out — the counter pre-increments, so the first cursor is 1:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:704-707
    pub fn next_cursor(&mut self) -> u64 {
        self.cursor_seq = self.cursor_seq.saturating_add(1);
        self.cursor_seq
    }

The handler comment the behaviour breaks:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/list_components.rs:175-198
    // The token carries the app environment it was issued for and the cursor
    // the next page starts at. A token this operation never issued — a
    // malformed one, one issued while listing another app environment, or a
    // cursor past everything handed out — names no page, so it is refused
    // rather than silently served as a page of this list.
    let cursor = match input.next_token.as_deref() {
        None => 0,
        Some(token) => { match crate::state::parse_page_token(token, &key, store.cursor_seq) { ... None => return Err(... InvalidParameterException { message: Some(format!("nextToken {token:?} is not a token returned by ListComponents for this app environment")) } ...) } }
    };

(The same `None => 0` / `parse_page_token` pair is in export_components.rs:166-184 and export_themes.rs:163-181, so a cursor-0 token is accepted by the exports too.)

Test coverage: the token-refusal rows exercise only the ABOVE-issued direction, while their own comment states the broader rule —

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/component_lifecycle.rs:680-683
    // A token this operation never issued is a bad request, not an empty page. The rows
    // differ in how the token fails to name a page of this list: a shape it never emits,
    // a correctly shaped token issued while listing another app environment, and a
    // correctly scoped token whose cursor was never handed out.
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/component_lifecycle.rs:707-715
    let never_issued_cursor = format!("{}{}", &issued[..issued.len() - 16], "f".repeat(16));
    for token in ["not-a-token", "c-", "c-00000000000000zz", "0", &elsewhere, &never_issued_cursor] { ... assert_eq!(refused.status, 400, ...) }

`never_issued_cursor` substitutes `f`*16 (u64::MAX), i.e. a cursor ABOVE `issued`; no row substitutes `0`*16, so the lower end is unpinned even though the comment calls it the same case. tests/all/export_components.rs:555 and tests/all/export_themes.rs:551-562 carry the same six rows.

## Steps to Reproduce

1. Mount the service on one engine (as tests/all/component_lifecycle.rs does) and address app `d1234abcd`, environment `staging`.
2. POST /app/d1234abcd/environment/staging/components three times with valid `componentToCreate` payloads (distinct names).
3. GET /app/d1234abcd/environment/staging/components?maxResults=1 and read `nextToken` — a 24-digit token `c-<8 hex scope><16 hex cursor>`, whose cursor is 1 or more.
4. Forge a token the service never issued by keeping the prefix and the 8-hex app-environment digest and replacing the last 16 hex digits with zeros: `c-<same scope>0000000000000000`.
5. GET /app/d1234abcd/environment/staging/components?nextToken=<forged token>.
6. Expected per state.rs:1088-1089 and list_components.rs:175-179: HTTP 400 `InvalidParameterException` with "nextToken ... is not a token returned by ListComponents for this app environment" — the cursor was never handed out. Actual: HTTP 200 with the first page of components and a fresh `nextToken`.
7. Repeat step 5 against GET /export/app/d1234abcd/environment/staging/components and GET /export/app/d1234abcd/environment/staging/themes with the forged token: both also answer 200 with their first page.
