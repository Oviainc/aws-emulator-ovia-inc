Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 0f3666ffc56d576d343f5b2efcf2fb2c8ab1775d

# decode_page_token honours hex spellings encode_page_token never mints (uppercase/mixed-case), so a NextToken this service never issued resumes a page instead of being refused

_Filed: 2026-10-29T15:14:58Z_
_Operation: ListExports | Class: pagination_

> _Note: this issue was identified against the code base at commit `0f3666ffc56d576d343f5b2efcf2fb2c8ab1775d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All four paginated listings of this crate (`ListExports`, `ListTables`, `ListTagsForResource`, `ListExecutions`) resume from the one shared helper `crate::state::page_or_token_refusal` -> `page` -> `decode_page_token`. The token is minted with `::hex::encode`, which emits LOWERCASE hex only, but it is read back with `::hex::decode`, which accepts upper-, lower- and mixed-case hex digits alike. An issued token with any of its hex digits upper-cased is therefore a string this service never handed out, yet it decodes to the same bytes, its integrity tag matches, and the listing serves a page for it (HTTP 200) instead of the modelled `ValidationException` the crate reserves for a token it never issued.

That contradicts the rule the helper's own documentation states — the token is "self-verifying, so a token this listing never issued is rejected instead of being read as some arbitrary position in it" — and the enumeration in `decode_page_token`'s doc of the only cases that answer `None`. The integrity tag is computed over the DECODED bytes, so it cannot distinguish the canonical spelling the service minted from a re-cased copy; nothing normalises or rejects a non-canonical spelling before the decode. The crate's own end-to-end suite states the same rule it is tested against but its forged-token table omits the case variant, which is why this slips through.

The defect is in one shared helper, so the fix belongs there (refuse a token whose spelling is not the one `encode_page_token` writes) and it covers all four listings at once; `ListExports` is named here as the first listing whose handler reaches it.

## Evidence

Minting is lowercase-only, reading is case-insensitive — `src/state.rs:941`-`:962`:

    /// A page token naming the key the NEXT page of `kind` starts at: an integrity tag
    /// followed by the key, hex-encoded. Opaque — a client cannot read a position out of it
    /// and must hand it back verbatim — and self-verifying, so a token this listing never
    /// issued is rejected instead of being read as some arbitrary position in it.
    fn encode_page_token(kind: PageTokenKind<'_>, key: &str) -> String {
        let mut bytes = page_token_tag(kind, key);
        bytes.extend_from_slice(key.as_bytes());
        ::hex::encode(bytes)        // <- lowercase hex, the only spelling ever issued
    }

    /// The key a token issued by [`encode_page_token`] for `kind` names, or `None` when this
    /// sequence never issued it: not hex, too short to carry a tag, not UTF-8, or carrying a
    /// tag that does not match the sequence and position it claims.
    fn decode_page_token(kind: PageTokenKind<'_>, token: &str) -> Option<String> {
        let bytes = ::hex::decode(token).ok()?;   // <- accepts A-F as well as a-f

The refusal this bypasses, `src/state.rs:964`-`:966` and `:980`-`:989`:

    /// The sentence every listing refuses a `NextToken` it never issued with.
    const UNISSUED_PAGE_TOKEN_MESSAGE: &str =
        "NextToken is not a pagination token issued by this service.";
    ...
    pub fn page_or_token_refusal<T: Clone, E>(... ) -> Result<(Vec<T>, Option<String>), E> {
        page(kind, items, max_results, token).ok_or_else(|| wrap(unissued_page_token(token_requirement)))

The callers that inherit it: `src/ops/list_exports.rs:179`-`:186` (`PageTokenKind::Exports`), `src/ops/list_executions.rs:217`-`:229`, `src/ops/list_tables.rs:153`-`:160`, `src/ops/list_tags_for_resource.rs:204`-`:215`.

Test coverage that states the rule but not this form — `tests/all/page_token_refusal.rs:6`-`:17`:

    //! The rule under test: a `NextToken` a listing did not itself issue is the modelled
    //! `ValidationException` (HTTP 400) naming the `NextToken` member with reason
    //! `fieldValidationFailed` ... crossed with a set of unissued tokens
    //! differing in FORM — not hex at all, too short to carry the integrity tag, hex of a
    //! plausible position with no tag, an issued token with its tag corrupted, an issued token
    //! truncated

and the table itself, `tests/all/page_token_refusal.rs:187`-`:200`, which builds exactly those five forms (`"not-a-token-this-service-issued"`, `"00"`, `hex::encode("exec-a")`, a tag-corrupted copy, a truncated copy) and no re-cased copy.

## Steps to Reproduce

1. Create two exports (`CreateExport` twice) so `ListExports` has two entries to page over.
2. Call `ListExports` with `MaxResults: 1` and record the `NextToken` it returns — a lowercase hex string (`encode_page_token` -> `hex::encode`).
3. Upper-case every hex digit of that token (e.g. `token.to_uppercase()`). This string was never issued by this service: `encode_page_token` can only emit lowercase.
4. Call `ListExports` with `NextToken` set to the upper-cased string.
5. Observed: HTTP 200 with the resumed page — `hex::decode` accepted the re-cased digits, the tag matched the same decoded bytes, and `page` resumed from the key. Expected per `src/state.rs:941`-`:944` and the suite's rule at `tests/all/page_token_refusal.rs:6`: HTTP 400 `ValidationException`, reason `fieldValidationFailed`, naming `NextToken` with the message "NextToken is not a pagination token issued by this service."
6. Repeat against `ListTables`, `ListTagsForResource` and `ListExecutions` — all four reach the same `decode_page_token`, so all four accept the re-cased token.
