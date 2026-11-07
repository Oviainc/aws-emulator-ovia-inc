Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b8b66cbc148b086d6ee99961aa0fb3de50cb6993

# A ListTunnels nextToken's cursor half is unauthenticated, so a hand-written canonical token is served as a position the listing never issued

_Filed: 2026-11-07T04:08:52Z_
_Operation: ListTunnels | Class: validation_

> _Note: this issue was identified against the code base at commit `b8b66cbc148b086d6ee99961aa0fb3de50cb6993`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`decode_next_token` authenticates only the SCOPE half of a page token (the listing fingerprint) and nothing at all about the POSITION half. A token is `tnl-` + the creation sequence in 16 lowercase hex digits + the listing fingerprint in 16 lowercase hex digits (`state::render_next_token`), and the only acceptance tests are (a) the string equals its own canonical re-rendering and (b) the fingerprint field equals the fingerprint of the listing being served. Both are things a caller can produce: a caller holding ONE token this listing minted can overwrite the first 16 digits with any `u64` it likes, leave the last 16 untouched, and the result is canonical by construction and carries the right fingerprint — so `decode_next_token` answers `PagePosition::ResumeAfter(<caller's number>)` and the handler applies it to the store. (The fingerprint itself is an UNKEYED SHA-256 over the Region and `thingName` the request already carries, so it is computable from public inputs too.)

That turns the refusal this crate documents for an unissued cursor into a 200 page: `nextToken: tnl-ffffffffffffffff<fingerprint>` restarts the listing from the newest tunnel, and `tnl-0000000000000000<fingerprint>` serves an empty page with no `nextToken`, both for a string the service never minted. The crate's own documentation says the opposite in three places (handler, state helper, test header), and the workspace already carries the single helper written for exactly this distinction — `service_common::page_tokens::PageTokenKey`, which HMACs the whole cursor under a per-store secret and is already a dependency of this crate (it uses `service_common::arn` and `service_common::uuid_from_random_bytes`).

This is not the hole the earlier canonical-form fix closed: that one was about strings whose suffix merely happened to parse as hex (re-cased, over-padded, truncated). A token whose cursor field was rewritten is perfectly canonical, so the re-render check passes it, and nothing downstream ties `seq` to a position this service ever handed out.

## Evidence

The state helper claims a hand-written token is refused — `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/state.rs:959-967`:

    /// Read `token` as a position in `listing`.
    ///
    /// Parsing the fields is not enough to accept one: `u64::from_str_radix` also
    /// accepts a digit count, a sign and a letter case that [`render_next_token`]
    /// never emits, so the parsed fields are re-rendered and the result compared
    /// with what arrived — only a token identical to its own canonical rendering
    /// was minted here, and every other string (truncated, re-cased, over-padded or
    /// hand-written) is [`PagePosition::Unissued`]. A token that IS canonical still
    /// names a position only in the listing whose fingerprint it carries.

and the code under it accepts whatever number the caller wrote in the cursor field — `src/state.rs:981-995`:

        let (Ok(seq), Ok(fingerprint)) = (
            u64::from_str_radix(sequence_digits, 16),
            u64::from_str_radix(fingerprint_digits, 16),
        ) else {
            return PagePosition::Unissued;
        };
        if render_next_token(seq, fingerprint) != token {
            return PagePosition::Unissued;
        }
        if fingerprint == listing.fingerprint() {
            PagePosition::ResumeAfter(seq)
        } else {
            PagePosition::OtherListing
        }

The rendering that makes a forged cursor canonical by construction — `src/state.rs:933-938`:

    fn render_next_token(seq: u64, fingerprint: u64) -> String {
        format!(
            "{NEXT_TOKEN_PREFIX}{seq:0width$x}{fingerprint:0width$x}",
            width = NEXT_TOKEN_FIELD_DIGITS
        )
    }

The scope half is an unkeyed digest of values the request itself carries — `src/state.rs:903-921`:

        fn fingerprint(&self) -> u64 {
            use sha2::{Digest, Sha256};

            let mut digest = Sha256::new();
            digest.update(self.region.as_bytes());
            digest.update([0]);

The handler states the rule it is enforcing, and the refusal text for the case that is being bypassed — `src/ops/list_tunnels.rs:151-169`:

        // A page token names a position in ONE listing. A token this service did
        // not mint names none at all, and a token another listing minted names
        // one only there — applying it here would silently restart the listing
        // or empty its page. The operation models no error for either, so both
        // are refused as invalid input.
        ...
                crate::state::PagePosition::Unissued => {
                    return Err(crate::scaffolded::Scaffolded::validation(
                        "nextToken is not a token this service issued",
                    ))

and the forged position is applied straight to the store — `src/ops/list_tunnels.rs:205-207`:

            // Resume strictly after the position the token names: descending
            // order, so the next page holds the tunnels created before it.
            .filter(|tunnel| resume_after.is_none_or(|seq| tunnel.creation_seq < seq))

The workspace's one helper for this exact distinction, already in Cargo.lock and already a dependency of this crate — `services/common/src/page_tokens.rs:1-17`:

    //! The ONE way a service tells a continuation token it ISSUED from one a caller built.
    //!
    //! A paginated read hands back an opaque `NextToken` and is handed one back on the next
    //! call. The position the token names travels INSIDE it — there is no page kept on the
    //! side to look up — so a read that only checks the token's SHAPE honours any value of
    //! that shape, and a caller can name a position the service never offered: the first
    //! page of a report it was paging past, a position far in the future, a position under
    //! another filter. What the services' APIs document for such a value is a refusal (an
    //! invalid-token error), not a page.
    //!
    //! So a token carries an AUTHENTICATOR over its own cursor, computed under a secret the
    //! issuing store draws from the system's randomness and never reveals ([`PageTokenKey`])

Test coverage: `tests/all/list_tunnels_next_token_form.rs:176-178` states the rule that the code breaks —

    /// A string the service cannot have minted is refused with a 400
    /// `ValidationException` and serves no page, however close to a real cursor it
    /// looks.

— but every row of its table (`:195-210`) garbles the token's FORM (re-cased, zero-stripped, over-padded, truncated, re-prefixed), so no row is a canonical token with a substituted cursor field; `:148-153` additionally pins the 32-hex-digit form. `tests/all/list_tunnels_page_token_scope.rs:1-24` covers only replays across different listings (a foreign fingerprint), not a rewritten cursor inside the same listing. So the behaviour reported here is pinned by nothing.

## Steps to Reproduce

1. Mount the service on an `InMemoryEngine` and sign every request for one Region, as `tests/all/list_tunnels_next_token_form.rs` does.
2. Call `OpenTunnel` eleven times (e.g. `{"destinationConfig":{"thingName":"thing-1","services":["SSH"]}}`).
3. Call `ListTunnels` with `{"maxResults": 1}` and keep the returned `nextToken`, of the form `tnl-<16 hex cursor digits><16 hex fingerprint digits>` (`state::render_next_token`, src/state.rs:933-938).
4. Build a token the service never minted by replacing only the FIRST 16 hex digits with `ffffffffffffffff` and keeping the last 16 as they came: `tnl-ffffffffffffffff<fingerprint>`. It is canonical by construction, so `render_next_token(seq, fingerprint) != token` at src/state.rs:987 is false and the fingerprint comparison at :990 still succeeds.
5. Call `ListTunnels` with `{"nextToken": "tnl-ffffffffffffffff<fingerprint>"}`: HTTP 200 and the listing restarts from the newest tunnel, because every stored `creation_seq` satisfies `creation_seq < 0xffffffffffffffff` at src/ops/list_tunnels.rs:207 — a position this listing never issued a token for. The crate's documented answer for a token it did not issue is HTTP 400 `ValidationException` with `nextToken is not a token this service issued` (src/ops/list_tunnels.rs:165-169).
6. Repeat with the cursor field set to `0000000000000000`: HTTP 200 with an empty `tunnelSummaries` and no `nextToken`, again instead of the documented refusal.
