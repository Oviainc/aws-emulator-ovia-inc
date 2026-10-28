Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 21d30b53374928fd663edcc8ae486344ab510a4e

# The "this authorization is the oauth2 kind" branch is written twice, once against a bare literal instead of state.rs's AUTH_TYPE_OAUTH2

_Filed: 2026-10-28T04:28:53Z_
_Operation: ConnectAppAuthorization | Class: maintainability_

> _Note: this issue was identified against the code base at commit `21d30b53374928fd663edcc8ae486344ab510a4e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate declares one named constant for each modelled `AuthType` value in `state.rs` and routes every decision about an authorization's auth type through it: `AppAuthorizationRecord::auth_url` asks `self.auth_type == AUTH_TYPE_OAUTH2`, and `credential_kind_for_auth_type` asks `auth_type == AUTH_TYPE_API_KEY`. The `ConnectAppAuthorization` handler re-implements the same predicate — "is this stored authorization the OAuth2 kind?" — with a hand-written string literal `"oauth2"` instead of the constant, so the one rule now lives in two forms that can drift: a change to the constant (or to how the stored `auth_type` is spelled when written, which `create_app_authorization.rs` copies straight out of the validated input) silently stops matching in the connect handler, and the `authRequest`-required refusal the model mandates for an oauth2 authorization just stops firing. Replacing the literal with `crate::state::AUTH_TYPE_OAUTH2` leaves every response byte and every existing test untouched — the constant's value IS `"oauth2"` — and leaves exactly one place in the crate that spells the modelled enum value.

## Evidence

The constant, declared for this purpose — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs:115-118: "/// The other `AuthType` from the same enum: the one whose authorization is /// granted through an OAuth consent step, which is what /// [`AppAuthorizationRecord::auth_url`] reports a URL for." `pub const AUTH_TYPE_OAUTH2: &str = "oauth2";` (and the sibling at state.rs:109-113 `AUTH_TYPE_API_KEY`, whose doc notes the crate's rule of naming the modelled values: state.rs:38-43 — "Named here so the handlers that write the state and the ones that read it cannot drift apart over a bare literal.")

Copy 1 of the predicate, using the constant — src/state.rs:444-450: `pub fn auth_url(&self) -> Option<String> { if self.auth_type == AUTH_TYPE_OAUTH2 { Some(oauth_flow_url(...)) } else { None } }`

Copy 2 of the same predicate, using a bare literal — src/ops/connect_app_authorization.rs:199-208: "// `authRequest` is \"required if the app authorization for the request is // configured with an OAuth2 (oauth2) authorization type\" …" `if authorization.auth_type == "oauth2" && input.auth_request.is_none() {` followed by `crate::state::auth_request_required(&authorization.auth_type)`. A grep for `AUTH_TYPE_OAUTH2` in src/ops/ returns nothing, so this handler is the only site that does not use the constant.

Tests that exercise the affected branch and must keep passing unchanged: tests/all/app_authorization_auth_url.rs:28 (`const AUTH_TYPES: &[(&str, bool)] = &[("apiKey", false), ("oauth2", true)]`) and its connect calls at lines 247 and 332 that post `{"authRequest": {...}}` for an oauth2 authorization; tests/all/nested_resource_arn_region.rs:49-56 (the connect body "an `oauth2` …" authorization takes); tests/all/app_authorization_deletion.rs:114-125 (connect "carrying the OAuth2 `authRequest` an `oauth2` authorization requires and nothing for an `apiKey` one").

## Steps to Reproduce

1. grep -n 'AUTH_TYPE_OAUTH2' services/service-com-amazonaws-appfabric-fabricfrontendservice/src — only state.rs:118 (the declaration) and state.rs:445 (auth_url) match; no handler does
2. Open src/ops/connect_app_authorization.rs:203 — the same predicate is written `authorization.auth_type == "oauth2"`
3. Change the constant's spelling in state.rs (or imagine the stored value being normalised differently): auth_url stops reporting a URL while connect_app_authorization still takes its branch on the stale literal, and the two copies disagree about which authorizations are OAuth2
4. Substituting `crate::state::AUTH_TYPE_OAUTH2` at connect_app_authorization.rs:203 keeps every response and every test in tests/all/app_authorization_auth_url.rs, tests/all/nested_resource_arn_region.rs and tests/all/app_authorization_deletion.rs byte-identical
