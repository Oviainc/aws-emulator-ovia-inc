Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: c0478c2dc62a949b47f12552e33c2facb0cc4b79

# RefreshToken mints a credential-store secret before the bound check its sibling states must come first

_Filed: 2026-11-04T14:28:24Z_
_Operation: RefreshToken | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c0478c2dc62a949b47f12552e33c2facb0cc4b79`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two token handlers of this crate order the same two steps — the live-session bound and the minting of a provider token from the emulator's credential store — in opposite ways, and `ExchangeCodeForToken` writes down the rule the other one breaks.

`ExchangeCodeForToken` checks the bound FIRST and says why: a request it is going to refuse must do no work on the credential store. `RefreshToken` mints the access token first (a `CredentialStore::create` followed by a `delete`, see `crate::state::mint_provider_token`) and only then takes the store lock, materialises expiry and consults the same bound — so every refresh that is refused for the bound has already created and destroyed a credential in the shared credential store, which is exactly what the sibling handler's comment says a refused token request must not do.

The fix is confined to the order of the two blocks inside `RefreshToken::handle` (take the lock, expire, check the bound, then mint), which leaves every response unchanged and is independent of which refusal class the bound answers with (that is the subject of the separate open issue `20261103T184956Z-amplifyuibuilder-refreshtoken-error-response`).

## Evidence

The rule, stated in the sibling handler of the same surface:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/exchange_code_for_token.rs:220-223
        // The bound, BEFORE anything is minted, so a refused exchange does no work
        // on the credential store: the session an exchange starts is named by a
        // refresh token minted below, so it is never one the store already holds.
        if !store.has_room_for_another_provider_token_session() {

…and only afterwards does it mint:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/exchange_code_for_token.rs:237-245
        // Minted by the emulator's credential store, once the exchange is settled
        // so a refused one mints nothing: the access token the caller uses now and
        // the refresh token that names the session. Neither is constructed here.
        ...
            crate::state::mint_provider_token(credentials),

The handler under review does the opposite — it mints first:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/refresh_token.rs:195-197
        // Minted by the emulator's credential store, outside the store lock: a service
        // never constructs secret material itself.
        let Some(access_token) = crate::state::mint_provider_token(ctx.credential_store())

…and consults the bound only after that, under the lock:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/refresh_token.rs:216-221
        let mut store = state.lock();
        // The expiry the clock already owes, materialised before the bound below is
        // read, so a caller is never refused over access tokens that have run out.
        store.expire_provider_token_sessions(now);

        if !store.has_room_for_provider_token_session(&key) {

That a mint is a create-and-delete against the shared credential store (so it is real work, not a local computation):

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1798-1807
    pub fn mint_provider_token(store: &::emulator_server::CredentialStore) -> Option<String> {
        let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
        ...
        let _ = store.delete(&minted.id);

The comment the refresh path does carry justifies only WHERE the mint happens ("outside the store lock") and WHO mints it, not that it happens before the bound — so nothing in the crate justifies the divergence.

Test coverage: the token suites exercise the success and the blank-token refusal (tests/all/provider_tokens.rs, tests/all/provider_token_refresh.rs); no test asserts anything about the order of the mint and the bound, so reordering the two blocks leaves them green.

## Steps to Reproduce

1. Read services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/exchange_code_for_token.rs:220-245 — the bound is consulted at line 223, the two mints happen at lines 244-245, with the comment stating that a refused exchange must do no work on the credential store.
2. Read services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/refresh_token.rs:195-231 — the mint happens at line 197, the lock at 216, the expiry at 219 and the same bound only at 221.
3. Drive the store to PROVIDER_TOKEN_SESSION_QUOTA live sessions (state.rs:1682) and POST /tokens/figma/refresh with a fresh refresh token: the request is refused for the bound, but mint_provider_token (state.rs:1798-1807) has already created and deleted a credential in the shared CredentialStore for it.
4. Repeat the refused call: each refused refresh does that credential-store work again, which the sibling handler's comment says a refused token request must not do.
