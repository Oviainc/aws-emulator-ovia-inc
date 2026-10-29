Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sso-admin.application-access-scope-round-trip sso-admin.application-assignment-configuration-round-trip sso-admin.application-assignment-lifecycle sso-admin.application-authentication-method-round-trip sso-admin.application-grant-not-found-rejected sso-admin.application-grant-refresh-token-round-trip sso-admin.application-lifecycle-status-transition sso-admin.application-not-found-describe-rejected sso-admin.application-tag-quota-exceeded-rejected sso-admin.application-tag-resource-round-trip sso-admin.create-account-assignment-bad-permission-set-not-found-rejected sso-admin.create-application-assignment-duplicate-idempotent sso-admin.create-application-client-token-idempotency sso-admin.delete-application-cascades-assignment sso-admin.delete-trusted-token-issuer-while-referenced-by-grant sso-admin.trusted-token-issuer-client-token-conflict-rejected
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin.application-access-scope-round-trip|_pick_oauth_provider_arn#3|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.application-assignment-configuration-round-trip|_pick_application_provider_arn#3|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.application-assignment-lifecycle|_pick_application_provider_arn#3|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.application-authentication-method-round-trip|_pick_oauth_provider_arn#3|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.application-grant-not-found-rejected|_pick_oauth_provider_arn#3|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.application-grant-refresh-token-round-trip|_pick_oauth_provider_arn#3|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.application-lifecycle-status-transition|_pick_application_provider_arn#3|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.application-not-found-describe-rejected|_pick_oauth_provider_arn#3|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.application-tag-quota-exceeded-rejected|_pick_oauth_provider_arn#3|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.application-tag-resource-round-trip|_pick_oauth_provider_arn#3|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.create-account-assignment-bad-permission-set-not-found-rejected|_pick_oauth_provider_arn#7|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.create-application-assignment-duplicate-idempotent|_pick_oauth_provider_arn#4|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.create-application-client-token-idempotency|_pick_oauth_provider_arn#3|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.delete-application-cascades-assignment|_pick_application_provider_arn#3|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.delete-trusted-token-issuer-while-referenced-by-grant|_pick_oauth_provider_arn#3|AssertionError|raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
Issue-Key: sso-admin.trusted-token-issuer-client-token-conflict-rejected|run#12|AssertionError|assert message == "The request parameters are different from that of a previous request", message
Issue-Commit: 0ebbd81840eb1d8236e49c909db275b2251d47fa

# sso-admin: 2 net-new behavioral root causes — ListApplicationProviders' default page never surfaces the `custom` provider (blocks the whole application surface), and CreateTrustedTokenIssuer's reused-ClientToken ConflictException message is emulator-authored

> _Note: this issue was identified against the code base at commit `0ebbd81840eb1d8236e49c909db275b2251d47fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApplicationProviders

## Description
TWO distinct root causes in the sso-admin crate, both grounded in failing behavioral checks. A fixer must close BOTH before submitting.

=== Cause 1 of 2: ListApplicationProviders with no MaxResults never returns arn:aws:sso::aws:applicationProvider/custom, so no application can be created at all (15 failing checks) ===

AWS: ListApplicationProviders enumerates the IAM Identity Center application-provider catalog. Smithy: ListApplicationProviders input MaxResults (@range 1..100) and NextToken (Token, @length 0..2048, @pattern ^[-a-zA-Z0-9+=/_]*$); output ApplicationProviders (ApplicationProviderList) + NextToken; @paginated items=ApplicationProviders. The model carries NO default page size and NO ordering trait, so what a page contains is recorded behavior only. Recorded behavior (the wire corpus fixture sso-admin-catalog-list-application-providers, step task#1, which the open issue 20261029T184546Z-sso-admin-application-provider-catalog-is-wrong-describe-ignores-the.md quotes): a single ListApplicationProviders call with no MaxResults returns the catalog in ONE page — indices 0..393, i.e. 394 entries — in the service's own catalog order, which is not an ARN sort. That whole-catalog page is what makes the generic customer-managed OAuth 2.0 provider arn:aws:sso::aws:applicationProvider/custom reachable from an unparameterised list: `custom` is the ONLY provider CreateApplication accepts (Smithy: CreateApplication.ApplicationProviderArn @required, @pattern ^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$; the operation documents that it creates OAuth 2.0 customer managed applications only and "does not support creating SAML 2.0 customer managed applications or Amazon Web Services managed applications"), so a client that resolves a provider before CreateApplication finds it on the first page.

Emulator: the handler sorts the whole catalog ASCENDING BY ARN and then clamps the page to a fixed default of 100 when the request carries no MaxResults. Under an ARN sort the ARN segments order as `athena` < `catalog/...` < `custom`, and the emulator's catalog holds ~390 `catalog/<Name>` entries between them, so arn:aws:sso::aws:applicationProvider/custom lands several pages deep and is absent from every unparameterised ListApplicationProviders response. Every check that resolves a provider from a single list call therefore fails before it issues its first CreateApplication, with AssertionError "the generic customer-managed OAuth2 application provider is not available in this partition".

Why: a client cannot create an application against the emulator at all through the normal resolve-then-create flow — 15 of the 23 sso-admin behavioral checks never reach the operation they test (CreateApplication, PutApplicationAccessScope / GetApplicationAccessScope / ListApplicationAccessScopes / DeleteApplicationAccessScope, PutApplicationAssignmentConfiguration, CreateApplicationAssignment / DeleteApplicationAssignment, PutApplicationAuthenticationMethod, PutApplicationGrant / GetApplicationGrant, DeleteApplication, TagResource / UntagResource on an application, CreateAccountAssignment, DeleteTrustedTokenIssuer). The entire application and application-grant surface is unreachable, and a client that pages through to find `custom` is paging an order AWS does not serve either.

Fix: the ListApplicationProviders handler's paging of the provider catalog (the default page size it applies when MaxResults is absent, and the ARN sort it imposes on a catalog whose order is service data) together with the catalog seed those pages are drawn from. This is the SAME catalog that the already-open wire issue 20261029T184546Z-sso-admin-application-provider-catalog-is-wrong-describe-ignores-the.md faults for membership/order and for DescribeApplicationProvider's lookup; that issue carries no behavioral replay key and does not state that `custom` is unreachable from an unparameterised list, so this cause is filed separately and must be closed against these check ids.

Traces: sso-admin.application-access-scope-round-trip, sso-admin.application-assignment-configuration-round-trip, sso-admin.application-assignment-lifecycle, sso-admin.application-authentication-method-round-trip, sso-admin.application-grant-not-found-rejected, sso-admin.application-grant-refresh-token-round-trip, sso-admin.application-lifecycle-status-transition, sso-admin.application-not-found-describe-rejected, sso-admin.application-tag-quota-exceeded-rejected, sso-admin.application-tag-resource-round-trip, sso-admin.create-account-assignment-bad-permission-set-not-found-rejected, sso-admin.create-application-assignment-duplicate-idempotent, sso-admin.create-application-client-token-idempotency, sso-admin.delete-application-cascades-assignment, sso-admin.delete-trusted-token-issuer-while-referenced-by-grant. Code: services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/list_application_providers.rs:186-198 (the ARN sort and the crate::state::page call) and services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs:670 (MAX_PAGE_SIZE = 100, used as the no-MaxResults default) and :3998 (BUILT_IN_APPLICATION_PROVIDERS, whose first entry is the `custom` provider).

=== Cause 2 of 2: CreateTrustedTokenIssuer's reused-ClientToken ConflictException carries emulator-authored prose naming the stored ARN instead of AWS's fixed message (1 failing check) ===

AWS: ClientToken is CreateTrustedTokenIssuer's @idempotencyToken (Smithy: ClientToken, @length 1..64, @pattern ^[!-~]+$, @idempotencyToken) and its member documentation states the rule: "Passing the same value to a later call to an operation requires that you also pass the same value for all other parameters... If you retry the operation with the same ClientToken, but with different parameters, the retry fails". The refusal is the modelled ConflictException (Smithy: ConflictException, httpError 409). Recorded behavior for the message: the text is FIXED for this error class and carries no variable part at all — "The request parameters are different from that of a previous request". It names neither the ClientToken, nor the issuer that the first request created, nor any ARN.

Emulator: the HTTP status and the error code are right (409 ConflictException) but the message is emulator-authored and interpolates the stored issuer's ARN: "This ClientToken was already used for a different CreateTrustedTokenIssuer request, which created arn:aws:sso::<account-id>:trustedTokenIssuer/<instance-id>/tti-<uuid>". Breaking that down against AWS's requirement: the whole sentence is emulator prose (no part of it is AWS's fixed text), and its one variable part — the trustedTokenIssuer ARN — is a per-run, server-minted value taken from the emulator's state that AWS's message never contains in any form.

Why: a client that branches on, logs or asserts the conflict detail — the only member ConflictException carries — reads a different sentence from the emulator than from AWS for the same refusal, and one whose shape varies per run because a freshly minted issuer ARN is embedded in it. Any client or test that matches AWS's fixed conflict text fails against the emulator even though the status and code agree.

Fix: the CreateTrustedTokenIssuer handler's reused-ClientToken refusal — replace the hand-rolled format! with the fixed AWS text; it belongs beside the crate's other single-homed refusal-message helpers in state.rs rather than inline in the handler.

Traces: sso-admin.trusted-token-issuer-client-token-conflict-rejected. Code: services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_trusted_token_issuer.rs:262-271 (the ConflictException whose message is built with format! from existing.trusted_token_issuer_arn).

## Evidence
Behavioral suite simparity-behavioral-aws-v1: sso-admin 7/23 passed (30%), 16 failed, 0 skipped, 0 errored. Both causes below are mismatches; neither is marked ALREADY FILED nor OWNED by an open issue.

--- Cause 1 (15 checks) ---
Expected: a single ListApplicationProviders call with no MaxResults yields a page whose ApplicationProviders include an entry with ApplicationProviderArn == "arn:aws:sso::aws:applicationProvider/custom" (the recorded AWS page carries the whole 394-entry catalog in one response).
Actual: the served page carries at most 100 entries, ARN-sorted, so `custom` is absent. Each of the 15 checks aborts in its inlined provider resolver before any CreateApplication is issued:

  File "check_sso_admin_application_access_scope_round_trip.py", line 17, in run
    provider_arn = _pick_oauth_provider_arn(ctx)
  File "check_sso_admin_application_access_scope_round_trip.py", line 86, in _pick_oauth_provider_arn
    raise AssertionError("the generic customer-managed OAuth2 application provider is not available in this partition")
  AssertionError: the generic customer-managed OAuth2 application provider is not available in this partition

The resolver body, identical across all 15 checks (both the _pick_oauth_provider_arn and _pick_application_provider_arn spellings):
  providers = {p["ApplicationProviderArn"] for p in sso.list_application_providers().get("ApplicationProviders", [])}
  custom_arn = "arn:aws:sso::aws:applicationProvider/custom"
  if custom_arn not in providers: raise AssertionError(...)

The same assertion, at the same point, in all 15: application-access-scope-round-trip (line 86), application-assignment-configuration-round-trip (line 64), application-assignment-lifecycle (line 108), application-authentication-method-round-trip (line 106), application-grant-not-found-rejected (line 87), application-grant-refresh-token-round-trip (line 96), application-lifecycle-status-transition (line 80), application-not-found-describe-rejected (line 73), application-tag-quota-exceeded-rejected (line 84), application-tag-resource-round-trip (line 87), create-account-assignment-bad-permission-set-not-found-rejected (line 115), create-application-assignment-duplicate-idempotent (line 100), create-application-client-token-idempotency (line 107), delete-application-cascades-assignment (line 103), delete-trusted-token-issuer-while-referenced-by-grant (line 118).

Corroborating source: list_application_providers.rs sorts `ordered.sort_by(|(left,_),(right,_)| left.cmp(right))` over BUILT_IN_APPLICATION_PROVIDERS, then calls crate::state::page(..., input.max_results); state.rs page() does `let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE)` with MAX_PAGE_SIZE = 100. BUILT_IN_APPLICATION_PROVIDERS DOES contain the `custom` entry (state.rs:4000, customer_managed_provider("custom", "SSO Custom Application Provider", ...)) — it is simply never on the first page under an ARN sort, since ~390 `.../applicationProvider/catalog/<Name>` ARNs sort between `.../athena` and `.../custom`.

--- Cause 2 (1 check) ---
Check sso-admin.trusted-token-issuer-client-token-conflict-rejected. The idempotent replay and the ConflictException CODE both pass; only the message diverges:

  File "check_sso_admin_trusted_token_issuer_client_token_conflict_rejected.py", line 67, in run
    assert message == "The request parameters are different from that of a previous request", message
  AssertionError: This ClientToken was already used for a different CreateTrustedTokenIssuer request, which created arn:aws:sso::000000000000:trustedTokenIssuer/ssoins-7223b1d1ac5e1f0e/tti-f37ba10a-5143-4c61-b409-e66e3f8a3bb2

  expected Error.Code "ConflictException" (409) — MATCHES
  expected Error.Message "The request parameters are different from that of a previous request" (fixed text, no variable parts)
  actual   Error.Message "This ClientToken was already used for a different CreateTrustedTokenIssuer request, which created <trustedTokenIssuerArn>" — entirely emulator prose; the trailing ARN is a per-run, server-minted value read out of the emulator's store (the account id, instance id and tti-<uuid> above are one run's values, not part of any template).

The request sequence that earns it: CreateTrustedTokenIssuer(InstanceArn, Name, ClientToken=T, TrustedTokenIssuerType=OIDC_JWT, Configuration.OidcJwtConfiguration.IssuerUrl="https://example.com/oidc-first") succeeds; the same call with ClientToken=T and IssuerUrl="https://example.com/oidc-second" is the refusal under test.

Corroborating source: create_trusted_token_issuer.rs:262-271 builds the ConflictException message with format!("This ClientToken was already used for a different CreateTrustedTokenIssuer request, which created {}", existing.trusted_token_issuer_arn).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin behavioral checks (suite simparity-behavioral-aws-v1).

Cause 1:
2. Call ListInstances (or CreateInstance) and wait for Status == ACTIVE.
3. Call ListApplicationProviders with NO MaxResults and NO NextToken.
4. Collect the ApplicationProviderArn of every entry in ApplicationProviders. Observe the page carries 100 entries, ARN-sorted (first entries are .../applicationProvider/athena then .../applicationProvider/catalog/15Five, .../catalog/AbsorbLMS, ...), and that "arn:aws:sso::aws:applicationProvider/custom" is NOT among them. Against AWS the single page carries the full 394-entry catalog and does include it.
5. Re-replay the 15 check ids listed in trace_ids; each aborts in its provider resolver with AssertionError "the generic customer-managed OAuth2 application provider is not available in this partition" before issuing CreateApplication.

Cause 2:
6. Resolve an ACTIVE InstanceArn as above.
7. CreateTrustedTokenIssuer(InstanceArn=<arn>, Name=<n>, ClientToken="tok-1", TrustedTokenIssuerType="OIDC_JWT", TrustedTokenIssuerConfiguration={"OidcJwtConfiguration":{"IssuerUrl":"https://example.com/oidc-first","ClaimAttributePath":"email","IdentityStoreAttributePath":"emails.value","JwksRetrievalOption":"OPEN_ID_DISCOVERY"}}) — succeeds, returns a TrustedTokenIssuerArn.
8. Repeat the identical call with the SAME ClientToken "tok-1" but IssuerUrl="https://example.com/oidc-second".
9. Observe HTTP 409 / Error.Code "ConflictException" (correct) but Error.Message "This ClientToken was already used for a different CreateTrustedTokenIssuer request, which created arn:aws:sso::...:trustedTokenIssuer/.../tti-..." instead of AWS's fixed "The request parameters are different from that of a previous request".
10. Re-replay sso-admin.trusted-token-issuer-client-token-conflict-rejected to confirm.
