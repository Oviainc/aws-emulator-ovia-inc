Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 62bd53a2191a4112059965729a42c5aa0aaceb39

# The page-token guard admits one cursor offset the handler's own minting rule can never produce

_Filed: 2026-10-28T16:46:01Z_
_Operation: ListServices | Class: validation_

> _Note: this issue was identified against the code base at commit `62bd53a2191a4112059965729a42c5aa0aaceb39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListServices` serves pages from a self-describing cursor: the token carries a zero-padded offset, and the handler states (twice) that only a token this service MINTED is a valid resume point — anything else is answered with the pagination-token `ValidationException`.

But the two halves of that rule disagree at exactly one value. The MINT side issues a token only when there are items past the page just served (`end < catalog.len()`), so no token the service ever hands out can carry `offset == catalog.len()`. The VALIDATE side admits `offset <= catalog.len()`. The single value in the gap — `offset == catalog.len()` — is therefore a cursor that only a caller forging a token can produce, and the handler accepts it: it slices `catalog[len..len]`, and answers HTTP 200 with `services: []` and no `nextToken`, instead of the refusal every other never-minted token gets.

So a hand-built `nextToken` of `base64("uxc/service-catalog/offset=000000000226")` (226 = the seeded catalog's length) is served as a successful empty page, while the same forged token with any other out-of-catalog offset — e.g. `...offset=000000000227` — is refused 400. The inline comment directly above the guard claims the opposite: that a decoded position the catalog no longer has is "refused like any other token this service did not issue for this listing".

No existing test exercises `offset == catalog.len()`: the refusal table in `tests/all/account_customizations.rs` covers undecodable base64, a non-cursor plaintext, a cursor-shaped plaintext with the WRONG PREFIX, a tampered token and a truncated token — never a right-prefix token at the end offset. So the boundary is unpinned in either direction.

## Evidence

The mint side can never produce `offset == catalog.len()` — services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/list_services.rs:263-267:

        Ok(ListServicesOutput {
            // "This value is null when no more results are available": a token is
            // issued only while the catalog holds items past this page.
            next_token: (end < catalog.len()).then(|| encode_page_token(end)),

The validate side admits it anyway — src/ops/list_services.rs:238-250:

        let offset = match input.next_token.as_deref() {
            None => 0,
            Some(token) => match decode_page_token(token) {
                // A token that decodes but names a position past the catalog's end
                // no longer resolves to a resume point, so it is refused like any
                // other token this service did not issue for this listing.
                Some(offset) if offset <= catalog.len() => offset,
                // The token cleared the member's own constraints at the boundary,
                // so what failed here is the cursor decode: answer with the
                // pagination framework's message, not the member-constraint one.
                _ => return Err(pagination_token_exception(Self::NAME)),
            },
        };

and the rule the guard is supposed to enforce is stated on the token prefix itself — src/ops/list_services.rs:85-91:

    /// The plaintext prefix of an issued page token, before base64 encoding.
    ///
    /// A token is the emulator's own: it is MINTED here (never taken from the
    /// caller's imagination) and validated on the way back in, ...

and again on the decoder — src/ops/list_services.rs:112-114:

    /// The offset a token issued by [`encode_page_token`] resumes at, or `None` when
    /// the token is not one this service minted.

The offset really is reachable only by forgery: the catalog is a fixed, sorted, de-duplicated seed that no operation mutates (src/state.rs:418-431 `Default`, src/state.rs:236 `let catalog: Vec<String> = state.lock().service_catalog.clone();`), so there is no shrink that could leave a legitimately issued token pointing at the end.

Test coverage of the surrounding rule, none of it at this value — tests/all/account_customizations.rs:478-500 lists the refusal rows (`"base64 of a plaintext that is not a cursor"`, `"base64 of a cursor-shaped plaintext with the wrong prefix"` = `base64("not/a/catalog/offset=000000000042")`, `"an issued token with one character changed"`, `"an issued token truncated to the minimum length"`), and tests/all/account_customizations.rs:272-341 walks every page size without ever re-submitting an end-offset cursor. Fixing the boundary therefore changes no existing assertion.

## Steps to Reproduce

1. Start the emulator and call `GET /v1/services` with no query parameters; note the page holds the whole seeded catalog (226 entries from `src/state.rs:178-405`) and carries no `nextToken`.
2. Forge a token for the end position: `base64("uxc/service-catalog/offset=000000000226")` — the same prefix `PAGE_TOKEN_PREFIX` and the same 12-digit width `PAGE_TOKEN_OFFSET_DIGITS` the handler mints with, so it decodes.
3. Call `GET /v1/services?nextToken=<that token, percent-encoded>`: the response is HTTP 200 with `"services": []` and no `nextToken`, i.e. a cursor the service never minted is accepted as a resume point.
4. Repeat with `...offset=000000000227`: HTTP 400 `ValidationException` with the pagination-token message — the same class of forged token, answered differently.
5. Compare with the guard's own comment at src/ops/list_services.rs:240-243 ("refused like any other token this service did not issue for this listing") and with the mint at src/ops/list_services.rs:266 (`(end < catalog.len())`), which can never emit offset 226.
